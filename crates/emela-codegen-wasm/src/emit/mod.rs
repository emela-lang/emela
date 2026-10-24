//! The emitter: Heap IR (stage `RefCounted`) to WebAssembly.
//!
//! The Heap IR already says everything the backend needs: the evaluation order (ANF), where
//! control goes (structured statements, failure and defect landings) and when references are
//! added and given up (`dup` and `drop`). The emitter follows it mechanically, like the C
//! emitter:
//!
//! - Every IR variable is a wasm local. A compound statement's `dst` is its local; `yield` sets
//!   it, `break` sets the block's and branches, `recur` sets the loop variables and branches to
//!   the loop head. Compound statements do not use wasm block result types: assigning the `dst`
//!   keeps the code in the same shape as the C emitter's.
//! - A call's defect landing is `if (em_defect != 0) { … }` after the call, and its failure
//!   landing `if (em_failure != 0) { … }`. `unwind` and `fail` return a dummy value.
//! - Heap objects have descriptors (`em_d…`) per shape: the tag, whether it is an error value and
//!   which slots hold references (for `drop`). Constructors without fields, string literals and
//!   closures without captures are static objects (rc == 0), which reference counting skips.
//! - The runtime (Perceus `dup`/`drop`, the allocator, strings, the numeric conversions and the
//!   WASI host calls) is emitted into the same module (see `runtime/`).
//!
//! The lowering is split by feature: this module has the entry point, the `Builder` skeleton and
//! the IR statements and values; `builtins` lowers `Builtin`; `external` handles the host
//! imports and their stubs; `runtimeop` lowers `RuntimeOp`; `suspend` emits the state machines
//! of suspending functions and begins a `suspend fn` operation call.

mod builtins;
mod external;
pub mod layout;
mod runtime;
mod runtimeop;
mod suspend;

use emela_core::{EnumId, FiberOp, OPTION_NONE, OPTION_SOME, Type, TypeId};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, Module, OnFail, Operand, PrimOp, PrimTy, Repr,
    SeqId, Stage, Stmt, StmtId, SwitchOn, Term, Value, VarId, enum_repr, repr_of,
};
use rustc_hash::{FxHashMap, FxHashSet};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, Function as WasmFunction, FunctionSection, GlobalSection,
    GlobalType, ImportSection, InstructionSink, MemArg, MemorySection, MemoryType,
    Module as WasmModule, RefType, TableSection, TableType, TypeSection, ValType,
};

use crate::check;
use crate::std_host::StdHost;
use crate::{WasmError, WasmErrorKind, WasmOutput};

/// What the generated `wasi:cli/run@0.3.0` export runs.
#[allow(dead_code)]
pub(crate) enum Run {
    /// The entry's `main` (its monomorphized copy).
    Main(Option<FnId>),
    /// Test functions, with their names.
    Tests(Vec<(String, FnId)>),
}

impl Run {
    /// The functions the output calls first.
    pub(crate) fn roots(&self) -> Vec<FnId> {
        match self {
            Run::Main(main) => main.iter().copied().collect(),
            Run::Tests(tests) => tests.iter().map(|&(_, id)| id).collect(),
        }
    }
}

/// The WASI 0.3 imports shared by the two output interfaces.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputImports {
    pub(crate) write: u32,
    pub(crate) stream_new: u32,
    pub(crate) stream_write: u32,
    pub(crate) stream_drop_writable: u32,
    pub(crate) future_read: u32,
    pub(crate) future_drop_readable: u32,
}

pub(crate) fn emit(m: &Module, entry: &Run) -> Result<WasmOutput, Vec<WasmError>> {
    assert_eq!(
        m.stage,
        Stage::RefCounted,
        "the WASM emitter reads the Heap IR"
    );
    let mut b = Builder::new(m);
    b.build(entry);
    if !b.errors.is_empty() {
        return Err(b.errors);
    }
    let (core, layout) = b.finish();
    let optional = crate::component::OptionalImports {
        environment: b.uses_env(),
        random: b.uses_random(),
        clock: b.uses_clock(),
        monotonic_clock: b.uses_std_host(StdHost::is_monotonic),
        filesystem: b.uses_std_host(StdHost::is_fs),
    };
    let module = crate::component::wrap(&core, optional).map_err(|e| {
        vec![WasmError {
            kind: WasmErrorKind::Internal,
            message: format!("internal error in the WASM component: {e}"),
        }]
    })?;
    Ok(WasmOutput { module, layout })
}

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

/// The shape of a heap object made by the program.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DescKey {
    /// 0 = fields, 1 = string (variable size).
    pub kind: u32,
    pub tag: u32,
    /// The enum index of an error value, or -1.
    pub enum_id: i32,
    pub name: String,
    pub show: Option<u32>,
    pub refs: Vec<bool>,
}

/// The wasm value type of a representation.
pub(crate) fn val(r: Repr) -> ValType {
    match r {
        Repr::I32 => ValType::I32,
        Repr::I64 => ValType::I64,
        Repr::F64 => ValType::F64,
        Repr::Ref => ValType::I32,
        Repr::Void => unreachable!("no value of the void representation"),
    }
}

fn repr_val(r: Repr) -> Option<ValType> {
    (r != Repr::Void).then(|| val(r))
}

// ---- global indices ----
const G_HEAP: u32 = 0;
const G_FREE: u32 = 1;
const G_DEFECT: u32 = 2;
const G_FAILURE: u32 = 3;
/// Set while a suspending function unwinds (used by the state-machine passes; not yet).
#[allow(dead_code)]
const G_SUSPENDING: u32 = 4;
const G_STRDESC: u32 = 5;
/// The output streams and the waitable set of the WASI 0.3 host calls (`runtime/host.rs`).
const G_STDOUT: u32 = 6;
const G_STDOUT_FUTURE: u32 = 7;
const G_STDERR: u32 = 8;
const G_STDERR_FUTURE: u32 = 9;
const G_WAITSET: u32 = 10;
/// The name of the handler retired last (spec 14.4: the release note names it).
pub(crate) const G_LAST_RETIRED: u32 = 11;
/// Whether `Std.Env` has fetched the environment list, and the list's pointer and length. The
/// WIT contract makes `get-environment` a value import, so the list is fetched once per command.
const G_ENV_LOADED: u32 = 12;
const G_ENV_PTR: u32 = 13;
const G_ENV_LEN: u32 = 14;
const GLOBALS: u32 = 15;

/// Object header: rc at 0, `aux` at 4, the descriptor pointer at 8, the size of a heap object's
/// block at 12, slots at 16. A `rc` of 0 marks static data (never counted).
const HDR: u32 = 16;

// A const's cell (spec 5.6; see `runtime/constant.rs`). The state is 0 (not evaluated), 1 (being
// evaluated), 2 (the value is in the cell) or 3 (the defect is in the cell). The last two words
// hold the static strings the runtime needs: the self-reference message and the prefix of the
// wrapped defect.
pub(crate) const CONST_STATE: u32 = 0;
pub(crate) const CONST_VALUE: u32 = 8;
pub(crate) const CONST_DEFECT: u32 = 16;
pub(crate) const CONST_SELF: u32 = 20;
pub(crate) const CONST_PREFIX: u32 = 24;
pub(crate) const CONST_CELL_SIZE: u32 = 32;
const DESC_KIND: u32 = 0;
const DESC_NSLOTS: u32 = 4;
const DESC_TAG: u32 = 8;
const DESC_ENUM: u32 = 12;
const DESC_REFS: u32 = 16;
const DESC_SHOW: u32 = 20;
const DESC_NAME: u32 = 24;
const DESC_NAMELEN: u32 = 28;
const DESC_SIZE: u32 = 32;

pub(crate) struct Builder<'m> {
    pub(crate) m: &'m Module,
    types: Vec<(Vec<ValType>, Vec<ValType>)>,
    type_map: FxHashMap<(Vec<ValType>, Vec<ValType>), u32>,
    pub(crate) imports: Vec<(String, String, u32)>,
    import_map: FxHashMap<(String, String), u32>,
    pub(crate) defs: Vec<u32>,
    fn_index: Vec<u32>,
    fn_table: Vec<u32>,
    table_funcs: Vec<u32>,
    pub(crate) rt: FxHashMap<&'static str, u32>,
    /// The runtime functions the IR bodies call. A runtime module's `setup` reads it to leave
    /// out static data the program never reaches.
    pub(crate) rt_used: FxHashSet<&'static str>,
    /// The `show` function of each error enum (spec 10.6), as a table index for the descriptor.
    pub(crate) error_shows: FxHashMap<EnumId, u32>,
    pub(crate) entry_index: u32,
    pub(crate) callback_index: u32,
    pub(crate) str_desc: u32,
    /// The descriptors of a list cell whose head is a reference, and one whose head is a word
    /// (the runtime's `String.split` / `String.code_points` build lists).
    pub(crate) cons_ref_desc: u32,
    pub(crate) cons_val_desc: u32,
    /// The static buffers of the Float runtime (allocated only when the program uses Float).
    pub(crate) float_area: u32,
    pub(crate) data: Vec<u8>,
    /// The static data of the runtime modules, by name (written by their `setup`).
    pub(crate) rt_data: FxHashMap<&'static str, u32>,
    descs: FxHashMap<DescKey, u32>,
    strs: FxHashMap<Vec<u8>, u32>,
    nullary: FxHashMap<u32, u32>,
    static_closures: FxHashMap<FnId, u32>,
    /// The function index of a const's body (the wrapper has `fn_index`).
    const_inits: FxHashMap<FnId, u32>,
    /// The static cell of a const.
    const_cells: FxHashMap<FnId, u32>,
    pub(crate) errors: Vec<WasmError>,
    pub(crate) bodies: Vec<WasmFunction>,
    /// The external functions resolved to a supported WASI 0.3 host, with their runtime wrapper.
    std_hosts: Vec<(FnId, StdHost)>,
    /// The suspend runtime's cells and per-function entries, when the program suspends.
    pub(crate) sus: Option<suspend::Plan>,
    /// The asynchronous host bridge's cells, when the program sleeps.
    pub(crate) async_plan: Option<runtime::async_bridge::Plan>,
    /// The file system runtime's cells, when the program uses `Std.FileSystem`.
    pub(crate) fs_plan: Option<runtime::fs::Plan>,
}

impl<'m> Builder<'m> {
    fn new(m: &'m Module) -> Self {
        let mut b = Builder {
            m,
            types: Vec::new(),
            type_map: FxHashMap::default(),
            imports: Vec::new(),
            import_map: FxHashMap::default(),
            defs: Vec::new(),
            fn_index: vec![0; m.functions.len()],
            fn_table: vec![0; m.functions.len()],
            table_funcs: Vec::new(),
            rt: FxHashMap::default(),
            rt_used: FxHashSet::default(),
            error_shows: FxHashMap::default(),
            entry_index: 0,
            callback_index: 0,
            str_desc: 0,
            cons_ref_desc: 0,
            cons_val_desc: 0,
            float_area: 0,
            data: vec![0; layout::STATIC_START as usize],
            rt_data: FxHashMap::default(),
            descs: FxHashMap::default(),
            strs: FxHashMap::default(),
            nullary: FxHashMap::default(),
            static_closures: FxHashMap::default(),
            const_inits: FxHashMap::default(),
            const_cells: FxHashMap::default(),
            errors: Vec::new(),
            bodies: Vec::new(),
            std_hosts: Vec::new(),
            sus: None,
            async_plan: None,
            fs_plan: None,
        };
        b.str_desc = b.string_desc();
        b.cons_ref_desc = b.list_desc(true);
        b.cons_val_desc = b.list_desc(false);
        b
    }

    pub(crate) fn core(&self) -> &'m emela_core::Module {
        &self.m.core
    }

    pub(crate) fn repr(&self, ty: TypeId) -> Repr {
        repr_of(self.core(), ty)
    }

    pub(crate) fn internal(&mut self, message: impl Into<String>) {
        self.errors.push(WasmError::new(
            WasmErrorKind::Internal,
            format!("internal error in the WASM emitter: {}", message.into()),
        ));
    }

    fn ty(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        if let Some(&i) = self.type_map.get(&(params.clone(), results.clone())) {
            return i;
        }
        let i = self.types.len() as u32;
        self.types.push((params.clone(), results.clone()));
        self.type_map.insert((params, results), i);
        i
    }

    /// Appends a WASI 0.3 import, or returns the index of the one already there.
    ///
    /// Imports are assigned before the IR functions get their indices. A new host import is
    /// added by calling this in the same early phase (`build`), never in the middle of the
    /// function index space.
    fn host_import(
        &mut self,
        module: &str,
        name: &str,
        params: Vec<ValType>,
        results: Vec<ValType>,
    ) -> u32 {
        let key = (module.to_owned(), name.to_owned());
        if let Some(&i) = self.import_map.get(&key) {
            return i;
        }
        let ty = self.ty(params, results);
        let i = self.imports.len() as u32;
        self.imports.push((key.0.clone(), key.1.clone(), ty));
        self.import_map.insert(key, i);
        i
    }

    /// Whether a supported WASI host is used (and so its runtime wrapper and import are needed).
    pub(crate) fn uses_std_host(&self, which: fn(StdHost) -> bool) -> bool {
        self.std_hosts.iter().any(|&(_, host)| which(host))
    }

    pub(crate) fn uses_console(&self) -> bool {
        self.uses_std_host(StdHost::is_console)
    }

    pub(crate) fn uses_env(&self) -> bool {
        self.uses_std_host(StdHost::is_env)
    }

    pub(crate) fn uses_random(&self) -> bool {
        self.uses_std_host(StdHost::is_random)
    }

    pub(crate) fn uses_clock(&self) -> bool {
        self.uses_std_host(StdHost::is_clock)
    }

    pub(crate) fn uses_sleep(&self) -> bool {
        self.uses_std_host(StdHost::is_sleep)
    }

    // ---- WASI 0.3 imports ----

    pub(crate) fn environment_get(&mut self) -> u32 {
        self.host_import(
            "wasi:cli/environment@0.3.0",
            "get-environment",
            // The result is a `list<tuple<string, string>>`, returned through a pointer.
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn random_u64(&mut self) -> u32 {
        self.host_import(
            "wasi:random/random@0.3.0",
            "get-random-u64",
            Vec::new(),
            vec![ValType::I64],
        )
    }

    pub(crate) fn system_clock_now(&mut self) -> u32 {
        self.host_import(
            "wasi:clocks/system-clock@0.3.0",
            "now",
            // The `instant` record is returned through a pointer.
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn monotonic_wait_for(&mut self) -> u32 {
        self.host_import(
            "wasi:clocks/monotonic-clock@0.3.0",
            "[async-lower]wait-for",
            vec![ValType::I64],
            vec![ValType::I32],
        )
    }

    pub(crate) fn subtask_drop(&mut self) -> u32 {
        self.host_import("$root", "[subtask-drop]", vec![ValType::I32], Vec::new())
    }

    pub(crate) fn exit_with_code(&mut self) -> u32 {
        self.host_import(
            "wasi:cli/exit@0.3.0",
            "exit-with-code",
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn task_return(&mut self) -> u32 {
        self.host_import(
            "[export]wasi:cli/run@0.3.0",
            "[task-return]run",
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn waitable_set_new(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-set-new]",
            Vec::new(),
            vec![ValType::I32],
        )
    }

    pub(crate) fn waitable_set_wait(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-set-wait]",
            vec![ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    pub(crate) fn waitable_set_drop(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-set-drop]",
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn waitable_join(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-join]",
            vec![ValType::I32, ValType::I32],
            Vec::new(),
        )
    }

    /// `write-via-stream` and the stream/future intrinsics it introduces, for one of the two
    /// output interfaces (`wasi:cli/stdout@0.3.0` or `wasi:cli/stderr@0.3.0`).
    pub(crate) fn output_imports(&mut self, interface: &'static str) -> OutputImports {
        OutputImports {
            write: self.host_import(
                interface,
                "write-via-stream",
                vec![ValType::I32],
                vec![ValType::I32],
            ),
            stream_new: self.host_import(
                interface,
                "[stream-new-0]write-via-stream",
                Vec::new(),
                vec![ValType::I64],
            ),
            stream_write: self.host_import(
                interface,
                "[async-lower][stream-write-0]write-via-stream",
                vec![ValType::I32, ValType::I32, ValType::I32],
                vec![ValType::I32],
            ),
            stream_drop_writable: self.host_import(
                interface,
                "[stream-drop-writable-0]write-via-stream",
                vec![ValType::I32],
                Vec::new(),
            ),
            future_read: self.host_import(
                interface,
                "[async-lower][future-read-1]write-via-stream",
                vec![ValType::I32, ValType::I32],
                vec![ValType::I32],
            ),
            future_drop_readable: self.host_import(
                interface,
                "[future-drop-readable-1]write-via-stream",
                vec![ValType::I32],
                Vec::new(),
            ),
        }
    }

    fn fn_ty(&mut self, f: &Function) -> u32 {
        let params: Vec<ValType> = f.params.iter().map(|&p| val(f.vars[p].repr)).collect();
        let results: Vec<ValType> = repr_val(self.repr(f.ret)).into_iter().collect();
        self.ty(params, results)
    }

    fn imported(&self, id: FnId) -> bool {
        let f = &self.m.functions[id];
        f.external
            .as_ref()
            .is_some_and(|e| e.binding("wasm").is_some())
    }

    fn build(&mut self, entry: &Run) {
        // The imports (the WASI host and the externals with a wasm binding) take the lowest
        // indices. A supported `Std` host is a runtime wrapper instead; it is resolved after the
        // runtime functions have their indices.
        for (id, f) in self.m.functions.iter() {
            if let Some(ext) = &f.external
                && let Some(binding) = ext.binding("wasm")
            {
                match crate::std_host::classify(f) {
                    Some(host) => self.std_hosts.push((id, host)),
                    None => self.external_import(id, f, binding),
                }
            }
        }
        // The WASI 0.3 host calls the runtime always uses (their indices must be fixed before the
        // function index space is assigned). The environment, random and clock imports are only
        // added when the program uses them.
        if self.uses_env() {
            self.environment_get();
        }
        if self.uses_random() {
            self.random_u64();
        }
        if self.uses_clock() {
            self.system_clock_now();
        }
        if self.uses_std_host(StdHost::is_monotonic) {
            self.monotonic_wait_for();
            // A `$root` intrinsic, not a WIT import: used to drop a completed or cancelled
            // subtask. Only the real monotonic clock starts one.
            self.subtask_drop();
        }
        if self.uses_std_host(StdHost::is_fs) {
            // The file system runtime waits on its own subtasks and stream operations.
            self.subtask_drop();
            runtime::fs::imports(self);
        }
        self.exit_with_code();
        self.output_imports("wasi:cli/stdout@0.3.0");
        self.output_imports("wasi:cli/stderr@0.3.0");
        self.waitable_set_new();
        self.waitable_set_wait();
        self.waitable_set_drop();
        self.waitable_join();
        self.task_return();
        // The IR definitions, in arena order (the entries of the table are the IR functions). A
        // const gets two: the wrapper callers see, then its body (`_init`).
        for (id, f) in self.m.functions.iter() {
            if self.imported(id) {
                continue;
            }
            let ty = self.fn_ty(f);
            let index = self.imports.len() as u32 + self.defs.len() as u32;
            self.fn_index[ix(id)] = index;
            self.fn_table[ix(id)] = self.table_funcs.len() as u32;
            self.table_funcs.push(index);
            self.defs.push(ty);
            if f.constant {
                self.const_cell(id, f);
                let init = self.imports.len() as u32 + self.defs.len() as u32;
                self.const_inits.insert(id, init);
                self.defs.push(ty);
            }
        }
        // The error `show` functions, as table indices (their descriptors point at them).
        for (enum_id, function) in self.m.error_shows.iter().copied() {
            let slot = if self.imported(function) {
                u32::MAX
            } else {
                self.fn_table[ix(function)]
            };
            self.error_shows.insert(enum_id, slot);
        }
        // The state machines of suspending functions: their extra definitions come right after
        // the IR functions, before the runtime (so the runtime indices do not move).
        if suspend::used(self) {
            suspend::reserve(self);
        }
        runtime::register(self);
        // A supported `Std` host calls its runtime wrapper directly.
        for i in 0..self.std_hosts.len() {
            let (id, host) = self.std_hosts[i];
            self.fn_index[ix(id)] = *self.rt.get(host.runtime_name()).unwrap_or_else(|| {
                self.errors.push(WasmError::new(
                    WasmErrorKind::Internal,
                    format!(
                        "internal error: missing runtime function `{}`",
                        host.runtime_name()
                    ),
                ));
                &0
            });
        }
        if suspend::used(self) {
            suspend::prepare(self);
        }
        // The async-lifted `wasi:cli/run@0.3.0#run` export and its callback. Both return the
        // component-model status code.
        let ty = self.ty(vec![], vec![ValType::I32]);
        self.entry_index = self.imports.len() as u32 + self.defs.len() as u32;
        self.defs.push(ty);
        let ty = self.ty(
            vec![ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        );
        self.callback_index = self.imports.len() as u32 + self.defs.len() as u32;
        self.defs.push(ty);

        self.emit_ir_bodies();
        suspend::emit_machines(self);
        runtime::emit(self);
        self.emit_entry(entry);
        self.emit_callback();
    }

    // ---- statics ----
    //
    // Descriptors are created on demand by shape (`DescKey`). A new lowering builds the
    // descriptor it needs with `desc` (and a static value with `nullary` or `string_literal`);
    // there is no central table to extend.

    fn alloc_static(&mut self, size: u32) -> u32 {
        let align = (8 - (self.data.len() as u32 % 8)) % 8;
        self.data
            .extend(std::iter::repeat_n(0u8, (align + size) as usize));
        let off = self.data.len() as u32 - size;
        debug_assert!(
            off >= layout::STATIC_START,
            "static data below the scratch area"
        );
        off
    }

    fn write_u32(&mut self, off: u32, v: u32) {
        self.data[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// The offset of a static string object: rc == 0, no slots, the length in the first word and
    /// the utf-8 bytes after it.
    pub(crate) fn string_literal(&mut self, bytes: &[u8]) -> u32 {
        self.string_object(bytes, true)
    }

    /// A string literal that no other use shares, so that the runtime can mark its `aux` word
    /// (the self-reference message of a const, spec 5.6).
    fn unique_string_literal(&mut self, bytes: &[u8]) -> u32 {
        self.string_object(bytes, false)
    }

    fn string_object(&mut self, bytes: &[u8], shared: bool) -> u32 {
        if shared && let Some(&o) = self.strs.get(bytes) {
            return o;
        }
        let o = self.alloc_static(HDR + 4 + bytes.len() as u32 + 1);
        let desc = self.string_desc();
        self.write_u32(o + 8, desc);
        self.write_u32(o + HDR, bytes.len() as u32);
        self.data[(o + HDR + 4) as usize..(o + HDR + 4) as usize + bytes.len()]
            .copy_from_slice(bytes);
        if shared {
            self.strs.insert(bytes.to_vec(), o);
        }
        o
    }

    /// The static cell of a const: its state, its value and defect, and the two messages the
    /// runtime wraps with (spec 5.6).
    fn const_cell(&mut self, id: FnId, f: &Function) {
        let name = check::display_name(&f.name);
        let prefix = self.string_literal(format!("while evaluating const {name}: ").as_bytes());
        // The marker in `aux` travels with the defect (through `defect_take`, notes and
        // `defect_resume`). The string is unique to this const, so marking it cannot affect a
        // user string with the same bytes.
        let self_ref = self.unique_string_literal(
            format!("const {name} refers to itself while being evaluated").as_bytes(),
        );
        self.write_u32(self_ref + 4, 1);
        let cell = self.alloc_static(CONST_CELL_SIZE);
        self.write_u32(cell + CONST_SELF, self_ref);
        self.write_u32(cell + CONST_PREFIX, prefix);
        self.const_cells.insert(id, cell);
    }

    fn string_desc(&mut self) -> u32 {
        self.desc(DescKey {
            kind: 1,
            tag: 0,
            enum_id: -1,
            name: "String".to_owned(),
            show: None,
            refs: Vec::new(),
        })
    }

    /// The descriptor of a list cell (`{head, tail}`), whose head is a reference or a word.
    fn list_desc(&mut self, head_ref: bool) -> u32 {
        self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name: "list".to_owned(),
            show: None,
            refs: vec![head_ref, true],
        })
    }

    pub(crate) fn desc(&mut self, key: DescKey) -> u32 {
        if let Some(&o) = self.descs.get(&key) {
            return o;
        }
        let o = self.alloc_static(DESC_SIZE);
        let refs = if key.refs.is_empty() {
            0
        } else {
            let r = self.alloc_static(key.refs.len() as u32);
            for (i, &b) in key.refs.iter().enumerate() {
                self.data[(r + i as u32) as usize] = u8::from(b);
            }
            r
        };
        let name = key.name.as_bytes().to_vec();
        let (name_ptr, name_len) = if name.is_empty() {
            (0, 0)
        } else {
            let p = self.alloc_static(name.len() as u32);
            self.data[p as usize..p as usize + name.len()].copy_from_slice(&name);
            (p, name.len() as u32)
        };
        self.write_u32(o + DESC_KIND, key.kind);
        self.write_u32(o + DESC_NSLOTS, key.refs.len() as u32);
        self.write_u32(o + DESC_TAG, key.tag);
        self.write_u32(o + DESC_ENUM, key.enum_id as u32);
        self.write_u32(o + DESC_REFS, refs);
        self.write_u32(o + DESC_SHOW, key.show.unwrap_or(u32::MAX));
        self.write_u32(o + DESC_NAME, name_ptr);
        self.write_u32(o + DESC_NAMELEN, name_len);
        self.descs.insert(key, o);
        o
    }

    /// If `ty` is Prelude's `Option[A]`, the descriptor of `Some` (its field as a word), the
    /// static `None` object, and the representation of the `Some` field. The runtime's
    /// `Int.parse` and friends build the option with these.
    pub(crate) fn option_parts(&mut self, ty: TypeId) -> Option<(u32, u32, Repr)> {
        let core = self.core();
        let option = core.option?;
        let field = match core.ty(ty) {
            Type::Enum(id, args) if *id == option && args.len() == 1 => args[0],
            _ => return None,
        };
        let repr = self.repr(field);
        let some = self.desc(DescKey {
            kind: 0,
            tag: OPTION_SOME as u32,
            enum_id: -1,
            name: "Some".to_owned(),
            show: None,
            refs: vec![repr == Repr::Ref],
        });
        let none_desc = self.desc(DescKey {
            kind: 0,
            tag: OPTION_NONE as u32,
            enum_id: -1,
            name: "None".to_owned(),
            show: None,
            refs: Vec::new(),
        });
        let none = self.nullary(none_desc);
        Some((some, none, repr))
    }

    /// A static object (rc == 0) of the descriptor, with no fields.
    pub(crate) fn nullary(&mut self, desc: u32) -> u32 {
        if let Some(&o) = self.nullary.get(&desc) {
            return o;
        }
        let o = self.alloc_static(HDR);
        self.write_u32(o + 8, desc);
        self.nullary.insert(desc, o);
        o
    }

    /// A static closure record (rc == 0) without captures.
    pub(crate) fn static_closure(&mut self, function: FnId) -> u32 {
        if let Some(&o) = self.static_closures.get(&function) {
            return o;
        }
        let desc = self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name: check::display_name(&self.m.functions[function].name).to_owned(),
            show: None,
            refs: vec![false],
        });
        let o = self.alloc_static(HDR + 8);
        self.write_u32(o + 8, desc);
        let table = self.fn_table[ix(function)];
        self.write_u32(o + HDR, table);
        self.static_closures.insert(function, o);
        o
    }

    pub(crate) fn fn_index(&self, id: FnId) -> u32 {
        self.fn_index[ix(id)]
    }

    /// The type index of a call with the given parameter and result representations.
    pub(crate) fn call_ty(&mut self, params: &[Repr], ret: Repr) -> u32 {
        let params: Vec<ValType> = params.iter().map(|&r| val(r)).collect();
        let results: Vec<ValType> = repr_val(ret).into_iter().collect();
        self.ty(params, results)
    }

    // ---- assembly ----

    fn finish(&mut self) -> (Vec<u8>, layout::Layout) {
        let mut module = WasmModule::new();

        let mut types = TypeSection::new();
        for (params, results) in &self.types {
            types.ty().function(params.clone(), results.clone());
        }
        module.section(&types);

        if !self.imports.is_empty() {
            let mut imports = ImportSection::new();
            for (m, n, ty) in &self.imports {
                imports.import(m, n, EntityType::Function(*ty));
            }
            module.section(&imports);
        }

        let mut functions = FunctionSection::new();
        for ty in &self.defs {
            functions.function(*ty);
        }
        module.section(&functions);

        let mut table = TableSection::new();
        table.table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: self.table_funcs.len() as u64,
            maximum: Some(self.table_funcs.len() as u64),
            shared: false,
        });
        module.section(&table);

        let layout = layout::Layout::new(self.data.len() as u32);
        let heap_base = layout.heap_base;
        let pages = (heap_base as u64).div_ceil(layout::PAGE as u64).max(1);
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        let mut globals = GlobalSection::new();
        for i in 0..GLOBALS {
            let init = match i {
                G_HEAP => ConstExpr::i32_const(heap_base as i32),
                G_STRDESC => ConstExpr::i32_const(self.str_desc as i32),
                _ => ConstExpr::i32_const(0),
            };
            globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &init,
            );
        }
        module.section(&globals);

        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export(
            "[async-lift]wasi:cli/run@0.3.0#run",
            ExportKind::Func,
            self.entry_index,
        );
        exports.export(
            "[callback][async-lift]wasi:cli/run@0.3.0#run",
            ExportKind::Func,
            self.callback_index,
        );
        // An imported function returning a list (`get-environment`, `get-directories`) makes the
        // host allocate the result in our linear memory through `cabi_realloc` (the canonical
        // ABI's realloc).
        if let Some(&realloc) = self.rt.get("cabi_realloc") {
            exports.export("cabi_realloc", ExportKind::Func, realloc);
        }
        module.section(&exports);

        if !self.table_funcs.is_empty() {
            let funcs = self.table_funcs.clone();
            let mut elements = ElementSection::new();
            elements.segment(wasm_encoder::ElementSegment {
                mode: wasm_encoder::ElementMode::Active {
                    table: Some(0),
                    offset: &ConstExpr::i32_const(0),
                },
                elements: Elements::Functions(std::borrow::Cow::Owned(funcs)),
            });
            module.section(&elements);
        }

        let mut code = CodeSection::new();
        for body in &self.bodies {
            code.function(body);
        }
        module.section(&code);

        let mut data = DataSection::new();
        data.active(0, &ConstExpr::i32_const(0), self.data.iter().copied());
        module.section(&data);

        (module.finish(), layout)
    }

    // ---- function bodies ----

    fn emit_ir_bodies(&mut self) {
        let m = self.m;
        let mut bodies = Vec::new();
        for (id, f) in m.functions.iter() {
            if self.imported(id) {
                continue;
            }
            if f.constant {
                // A const's body is evaluated eagerly, so it never suspends (spec 5.6).
                bodies.push(self.const_wrapper(id, f));
                bodies.push(self.emit_fn(id, f));
                continue;
            }
            let body = if suspend::is_machine(f) {
                self.machine_wrapper(id, f)
            } else if f.external.is_some() {
                self.external_stub(id, f)
            } else {
                self.emit_fn(id, f)
            };
            bodies.push(body);
        }
        self.bodies = bodies;
    }

    /// The function callers of a const call: returns the remembered value or defect, and runs the
    /// body (`_init`) on the first reference only (spec 5.6).
    fn const_wrapper(&mut self, id: FnId, f: &Function) -> WasmFunction {
        let ret = self.repr(f.ret);
        let cell = self.const_cells[&id];
        let init = self.const_inits[&id];
        let mut locals = Vec::new();
        if ret != Repr::Void {
            locals.push((1, val(ret)));
        }
        let mut func = WasmFunction::new(locals);
        const TEMP: u32 = 0;
        {
            let mut ins = func.instructions();
            // The cell holds the value: return it without calling the runtime.
            ins.i32_const(cell as i32);
            ins.i32_load(mem(CONST_STATE, 4));
            ins.i32_const(2);
            ins.i32_eq();
            ins.if_(BlockType::Empty);
            self.load_const_value(&mut ins, cell, ret);
            ins.return_();
            ins.end();
            // 1: the cell holds the defect, which `const_enter` raised.
            ins.i32_const(cell as i32);
            ins.call(self.rt("const_enter"));
            ins.if_(BlockType::Empty);
            dummy_return(&mut ins, ret);
            ins.end();
            // 0: evaluate the body once.
            ins.call(init);
            if ret != Repr::Void {
                ins.local_set(TEMP);
            }
            ins.global_get(G_DEFECT);
            ins.if_(BlockType::Empty);
            // A failing evaluation owns nothing to keep: the IR returns a dummy value.
            if ret == Repr::Ref {
                ins.local_get(TEMP);
                let drop = self.rt("drop");
                ins.call(drop);
            }
            ins.i32_const(cell as i32);
            let leave = self.rt("const_leave");
            ins.call(leave);
            dummy_return(&mut ins, ret);
            ins.end();
            // The value is now in the cell; the caller gets its own reference.
            if ret != Repr::Void {
                ins.i32_const(cell as i32);
                ins.local_get(TEMP);
            }
            match ret {
                Repr::Void => {}
                Repr::I32 | Repr::Ref => {
                    ins.i32_store(mem(CONST_VALUE, 4));
                }
                Repr::I64 => {
                    ins.i64_store(mem(CONST_VALUE, 8));
                }
                Repr::F64 => {
                    ins.f64_store(mem(CONST_VALUE, 8));
                }
            }
            if ret == Repr::Ref {
                ins.local_get(TEMP);
                let dup = self.rt("dup");
                ins.call(dup);
            }
            ins.i32_const(cell as i32);
            let leave = self.rt("const_leave");
            ins.call(leave);
            if ret != Repr::Void {
                ins.local_get(TEMP);
            }
            ins.return_();
            ins.end();
        }
        func
    }

    /// The ordinary entry of a state machine: the IR, closures and direct calls see this
    /// signature; it enters the body without a frame.
    fn machine_wrapper(&mut self, id: FnId, f: &Function) -> WasmFunction {
        let Some(body) = self
            .sus
            .as_ref()
            .and_then(|p| p.machine(id))
            .map(|m| m.body)
        else {
            self.internal(format!("no state machine for {}", f.name));
            return WasmFunction::new(Vec::<(u32, ValType)>::new());
        };
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let mut ins = func.instructions();
            for i in 0..f.params.len() as u32 {
                ins.local_get(i);
            }
            ins.i32_const(0);
            ins.call(body);
            ins.end();
        }
        func
    }

    /// Loads a const's remembered value. A reference is duplicated: the cell keeps its own.
    fn load_const_value(&mut self, ins: &mut InstructionSink, cell: u32, ret: Repr) {
        match ret {
            Repr::Void => {}
            Repr::I32 => {
                ins.i32_const(cell as i32);
                ins.i32_load(mem(CONST_VALUE, 4));
            }
            Repr::Ref => {
                // `dup` takes the pointer by value and counts it; load it again for the caller.
                ins.i32_const(cell as i32);
                ins.i32_load(mem(CONST_VALUE, 4));
                let dup = self.rt("dup");
                ins.call(dup);
                ins.i32_const(cell as i32);
                ins.i32_load(mem(CONST_VALUE, 4));
            }
            Repr::I64 => {
                ins.i32_const(cell as i32);
                ins.i64_load(mem(CONST_VALUE, 8));
            }
            Repr::F64 => {
                ins.i32_const(cell as i32);
                ins.f64_load(mem(CONST_VALUE, 8));
            }
        }
    }
    fn emit_fn(&mut self, id: FnId, f: &Function) -> WasmFunction {
        let mut cx = FnCtx::new(f, self);
        let Some(body) = f.body else {
            self.internal(format!("fn {} has no body", f.name));
            return WasmFunction::new(Vec::<(u32, ValType)>::new());
        };
        let mut func = WasmFunction::new(cx.decls.clone());
        {
            let mut ins = func.instructions();
            self.seq(&mut cx, &mut ins, body, None);
            // A body that falls off its end must still return the right type.
            let ret = cx.ret;
            dummy_return(&mut ins, ret);
            ins.end();
        }
        let _ = id;
        func
    }

    fn emit_entry(&mut self, entry: &Run) {
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let ins = &mut func.instructions();
            match entry {
                Run::Main(Some(id)) => {
                    let index = self.fn_index(*id);
                    let ret = self.repr(self.m.functions[*id].ret);
                    ins.call(index);
                    self.drop_result(ins, ret);
                    if self.sus.is_some() {
                        let root = self.rt("root_run");
                        ins.call(root);
                    }
                }
                Run::Main(None) => {}
                Run::Tests(tests) => {
                    for (name, id) in tests {
                        // Each test is a run of its own: the previous one may have stalled.
                        if let Some(plan) = &self.sus
                            && plan.unwinding != 0
                        {
                            ins.i32_const(plan.unwinding as i32);
                            ins.i32_const(0);
                            ins.i32_store(mem(0, 4));
                        }
                        let index = self.fn_index(*id);
                        let ret = self.repr(self.m.functions[*id].ret);
                        ins.call(index);
                        self.drop_result(ins, ret);
                        if self.sus.is_some() {
                            let root = self.rt("root_run");
                            ins.call(root);
                        }
                        let name = self.string_literal(name.as_bytes());
                        ins.i32_const(name as i32);
                        ins.call(self.rt("test_result"));
                    }
                    ins.call(self.rt("test_done"));
                }
            }
            ins.call(self.rt("finish"));
            ins.call(self.rt("close_output"));
            ins.i32_const(0);
            ins.call(self.task_return());
            // `0` is the component model's EXIT status: the async-lifted run finished
            // synchronously.
            ins.i32_const(0);
            ins.end();
        }
        self.bodies.push(func);
    }

    /// The callback of the async-lifted `run`. The generated run never yields (its waits are
    /// synchronous), so the host never calls this; it must exist for the async ABI.
    fn emit_callback(&mut self) {
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let ins = &mut func.instructions();
            ins.i32_const(0);
            ins.end();
        }
        self.bodies.push(func);
    }

    /// Gives up a call's result: `drop` for a reference, `drop` for a plain value, nothing for
    /// `()`.
    fn drop_result(&mut self, ins: &mut InstructionSink, ret: Repr) {
        match ret {
            Repr::Ref => {
                ins.call(self.rt("drop"));
            }
            Repr::Void => {}
            _ => {
                ins.drop();
            }
        }
    }

    fn rt(&mut self, name: &'static str) -> u32 {
        self.rt_used.insert(name);
        *self.rt.get(name).unwrap_or_else(|| {
            self.errors.push(WasmError::new(
                WasmErrorKind::Internal,
                format!("internal error: missing runtime function `{name}`"),
            ));
            &0
        })
    }

    // ---- IR emission ----

    fn assign(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        repr: Repr,
    ) {
        match dst {
            Some(d) => {
                if repr != Repr::Void {
                    ins.local_set(cx.local(d));
                }
            }
            None => {
                if repr != Repr::Void {
                    ins.drop();
                }
            }
        }
    }

    fn operand(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, o: &Operand) {
        match o {
            Operand::Var(v) => {
                ins.local_get(cx.local(*v));
            }
            Operand::Const(c) => self.const_val(ins, c),
        }
    }

    fn const_val(&mut self, ins: &mut InstructionSink, c: &Const) {
        match c {
            Const::Int(n) => {
                ins.i32_const(*n);
            }
            Const::Int64(n) => {
                ins.i64_const(*n);
            }
            Const::Float(x) => {
                ins.f64_const((*x).into());
            }
            Const::Bool(b) => {
                ins.i32_const(i32::from(*b));
            }
            Const::String(s) => {
                let o = self.string_literal(s.as_bytes());
                ins.i32_const(o as i32);
            }
            Const::Unit => {}
        }
    }

    fn operand_repr(f: &Function, o: &Operand) -> Repr {
        match o {
            Operand::Var(v) => f.vars[*v].repr,
            Operand::Const(c) => c.repr(),
        }
    }

    /// Stores the operand into slot `index` of the object in local `base` (a word: any word for a
    /// value without a representation).
    fn store_slot(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        base: VarId,
        index: usize,
        o: &Operand,
    ) {
        let off = HDR + index as u32 * 8;
        let r = Self::operand_repr(cx.f, o);
        ins.local_get(cx.local(base));
        match r {
            Repr::Void => {
                ins.i32_const(0);
                ins.i32_store(mem(off, 4));
            }
            Repr::I32 | Repr::Ref => {
                self.operand(cx, ins, o);
                ins.i32_store(mem(off, 4));
            }
            Repr::I64 => {
                self.operand(cx, ins, o);
                ins.i64_store(mem(off, 8));
            }
            Repr::F64 => {
                self.operand(cx, ins, o);
                ins.f64_store(mem(off, 8));
            }
        }
    }

    fn alloc(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: VarId,
        desc: u32,
        nslots: usize,
    ) {
        ins.i32_const(nslots as i32);
        ins.i32_const(desc as i32);
        let alloc = self.rt("alloc");
        ins.call(alloc);
        ins.local_set(cx.local(dst));
    }

    fn seq(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        id: SeqId,
        yield_dst: Option<VarId>,
    ) {
        let stmts = cx.f.seqs[id].stmts.clone();
        for s in stmts {
            self.stmt(cx, ins, s);
        }
        let term = cx.f.seqs[id].term.clone();
        self.term(cx, ins, &term, yield_dst);
    }

    fn term(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        term: &Term,
        yield_dst: Option<VarId>,
    ) {
        match term {
            Term::Yield(v) => {
                if let (Some(d), Some(v)) = (yield_dst, v) {
                    self.operand(cx, ins, v);
                    ins.local_set(cx.local(d));
                } else if let (None, Some(_)) = (yield_dst, v) {
                    self.internal(format!("yield without a target in {}", cx.f.name));
                }
            }
            Term::Return(v) => {
                if let Some(v) = v {
                    self.operand(cx, ins, v);
                }
                ins.return_();
            }
            Term::Fail(v) => {
                self.operand(cx, ins, v);
                ins.global_set(G_FAILURE);
                dummy_return(ins, cx.ret);
            }
            Term::Break { label, value } => {
                let Some((depth, dst)) = cx.find_block(*label) else {
                    self.internal(format!("break to an unknown label in {}", cx.f.name));
                    return;
                };
                if let (Some(d), Some(v)) = (dst, value) {
                    self.operand(cx, ins, v);
                    ins.local_set(cx.local(d));
                }
                ins.br(depth);
            }
            Term::Recur(args) => {
                let Some((depth, vars)) = cx.find_loop() else {
                    self.internal(format!("recur outside a loop in {}", cx.f.name));
                    return;
                };
                // Parallel assignment through scratch locals.
                for (i, (o, &v)) in args.iter().zip(&vars).enumerate() {
                    self.operand(cx, ins, o);
                    let r = cx.f.vars[v].repr;
                    ins.local_set(cx.scratch(r, i));
                }
                for (i, &v) in vars.iter().enumerate() {
                    let r = cx.f.vars[v].repr;
                    ins.local_get(cx.scratch(r, i));
                    ins.local_set(cx.local(v));
                }
                ins.br(depth);
            }
            Term::Unwind => dummy_return(ins, cx.ret),
            Term::Unreachable => {
                ins.unreachable();
            }
            Term::Defect(..) => self.internal(format!("defect left in {}", cx.f.name)),
        }
    }

    fn stmt(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, id: StmtId) {
        let stmt = cx.f.stmts[id].clone();
        match stmt {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                if on_suspend.is_some() {
                    // State machines have their own emitter (see `suspend`).
                    self.internal(format!("suspension edge left in {}", cx.f.name));
                }
                // Suspending code called where nothing may suspend runs in place (spec 8.7);
                // what comes back suspended is unwound as interrupted (spec 14.5).
                let sync = !cx.f.suspends && emela_lir::may_suspend(&value);
                if sync {
                    ins.call(self.rt("sync_enter"));
                }
                self.value(cx, ins, dst, &value);
                if sync {
                    ins.call(self.rt("sync_leave"));
                }
                if let Some(landing) = on_defect {
                    ins.global_get(G_DEFECT);
                    ins.if_(BlockType::Empty);
                    cx.push(Lbl::Other);
                    self.seq(cx, ins, landing, None);
                    cx.pop();
                    ins.end();
                }
                if let Some(on_fail) = value_on_fail(&value) {
                    match on_fail {
                        OnFail::Handle { error, body } => {
                            ins.global_get(G_FAILURE);
                            ins.if_(BlockType::Empty);
                            cx.push(Lbl::Other);
                            ins.global_get(G_FAILURE);
                            ins.local_set(cx.local(*error));
                            ins.i32_const(0);
                            ins.global_set(G_FAILURE);
                            self.seq(cx, ins, *body, None);
                            cx.pop();
                            ins.end();
                        }
                        _ => self.internal(format!("Flow IR failure edge in {}", cx.f.name)),
                    }
                }
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                self.operand(cx, ins, &cond);
                ins.if_(BlockType::Empty);
                cx.push(Lbl::Other);
                self.seq(cx, ins, then, dst);
                ins.else_();
                self.seq(cx, ins, else_, dst);
                cx.pop();
                ins.end();
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let scrutinee = match on {
                    SwitchOn::Tag(v) => {
                        if cx.f.vars[v].repr == Repr::Ref {
                            Scrutinee::Tag(v)
                        } else {
                            Scrutinee::Lit(v)
                        }
                    }
                    SwitchOn::Lit(v) => Scrutinee::Lit(v),
                };
                self.switch_chain(cx, ins, &cases, 0, &default, dst, scrutinee);
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                for (&v, init) in vars.iter().zip(&inits) {
                    self.operand(cx, ins, init);
                    ins.local_set(cx.local(v));
                }
                ins.loop_(BlockType::Empty);
                cx.push(Lbl::Loop(vars.clone()));
                self.seq(cx, ins, body, dst);
                cx.pop();
                ins.end();
            }
            Stmt::Block { dst, label, body } => {
                ins.block(BlockType::Empty);
                cx.push(Lbl::Block(label, dst));
                self.seq(cx, ins, body, dst);
                cx.pop();
                ins.end();
            }
            Stmt::Protect { .. } => self.internal(format!("protect left in {}", cx.f.name)),
            Stmt::Dup(v) => {
                ins.local_get(cx.local(v));
                let dup = self.rt("dup");
                ins.call(dup);
            }
            Stmt::Drop(v) => {
                ins.local_get(cx.local(v));
                let drop = self.rt("drop");
                ins.call(drop);
            }
        }
    }

    /// A switch as a chain of comparing `if` / `else`, nested once per case. Not `br_table`: the
    /// chain is simpler, and the depths of the branches out of it are counted the same way as
    /// for any other `if`.
    #[allow(clippy::too_many_arguments)]
    fn switch_chain(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        cases: &[emela_lir::Case],
        i: usize,
        default: &Option<SeqId>,
        dst: Option<VarId>,
        scrutinee: Scrutinee,
    ) {
        if i >= cases.len() {
            match default {
                Some(d) => {
                    self.seq(cx, ins, *d, dst);
                }
                None => {
                    ins.unreachable();
                }
            }
            return;
        }
        let case = &cases[i];
        self.push_scrutinee(cx, ins, scrutinee);
        match &case.key {
            CaseKey::Variant(k) => {
                ins.i32_const(*k as i32);
            }
            CaseKey::Lit(c) => {
                self.const_val(ins, c);
            }
        }
        match scrutinee {
            Scrutinee::Tag(_) => ins.i32_eq(),
            Scrutinee::Lit(v) => match cx.f.vars[v].repr {
                Repr::I64 => ins.i64_eq(),
                _ => ins.i32_eq(),
            },
        };
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        let body = case.body;
        self.seq(cx, ins, body, dst);
        ins.else_();
        self.switch_chain(cx, ins, cases, i + 1, default, dst, scrutinee);
        cx.pop();
        ins.end();
    }

    fn push_scrutinee(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, s: Scrutinee) {
        match s {
            Scrutinee::Tag(v) => {
                ins.local_get(cx.local(v));
                let tag = self.rt("tag");
                ins.call(tag);
            }
            Scrutinee::Lit(v) => {
                ins.local_get(cx.local(v));
            }
        }
    }

    fn value(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        value: &Value,
    ) {
        match value {
            Value::Const(c) => {
                self.const_val(ins, c);
                self.assign(cx, ins, dst, c.repr());
            }
            Value::Prim { op, ty, args } => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                self.prim(ins, *op, *ty);
                self.assign(cx, ins, dst, ty.repr());
            }
            Value::Call { callee, args, .. } => self.call(cx, ins, dst, callee, args),
            Value::Builtin { op, args } => self.builtin(cx, ins, dst, *op, args),
            Value::Make { ctor, args } => {
                let core = self.core();
                let def = &core.enums[ctor.enum_id];
                if enum_repr(core, ctor.enum_id) == Repr::I32 {
                    if let Some(d) = dst {
                        ins.i32_const(ctor.variant as i32);
                        ins.local_set(cx.local(d));
                    }
                    return;
                }
                let refs: Vec<bool> = args
                    .iter()
                    .map(|a| Self::operand_repr(cx.f, a) == Repr::Ref)
                    .collect();
                let key = DescKey {
                    kind: 0,
                    tag: ctor.variant as u32,
                    enum_id: if def.error {
                        ix(ctor.enum_id) as i32
                    } else {
                        -1
                    },
                    name: def.variants[ctor.variant].name.clone(),
                    show: if def.error {
                        self.error_shows.get(&ctor.enum_id).copied()
                    } else {
                        None
                    },
                    refs,
                };
                let desc = self.desc(key);
                if args.is_empty() {
                    let o = self.nullary(desc);
                    if let Some(d) = dst {
                        ins.i32_const(o as i32);
                        ins.local_set(cx.local(d));
                    }
                } else {
                    self.alloc_value(cx, ins, dst, desc, args);
                }
            }
            Value::Field { base, index, .. } | Value::TupleGet { base, index } => {
                if let Some(d) = dst {
                    let base = *base;
                    let repr = cx.f.vars[d].repr;
                    self.load_field(cx, ins, base, *index, repr);
                    ins.local_set(cx.local(d));
                }
            }
            Value::Tag(v) => {
                if cx.f.vars[*v].repr == Repr::Ref {
                    ins.local_get(cx.local(*v));
                    let tag = self.rt("tag");
                    ins.call(tag);
                } else {
                    ins.local_get(cx.local(*v));
                }
                self.assign(cx, ins, dst, Repr::I32);
            }
            Value::Tuple(elems) => {
                let refs: Vec<bool> = elems
                    .iter()
                    .map(|a| Self::operand_repr(cx.f, a) == Repr::Ref)
                    .collect();
                let desc = self.desc(DescKey {
                    kind: 0,
                    tag: 0,
                    enum_id: -1,
                    name: "tuple".to_owned(),
                    show: None,
                    refs,
                });
                let elems = elems.clone();
                self.alloc_value(cx, ins, dst, desc, &elems);
            }
            Value::List { elems, tail } => {
                let Some(d) = dst else {
                    self.internal(format!("list without a destination in {}", cx.f.name));
                    return;
                };
                // The tail lives in a scratch local while the cells are built; `List` and the
                // operations below are ANF, so one temporary is enough.
                let tmp = cx.scratch(Repr::Ref, 0);
                match tail {
                    Some(t) => self.operand(cx, ins, t),
                    None => {
                        ins.i32_const(0);
                    }
                }
                ins.local_set(tmp);
                let elems = elems.clone();
                for e in elems.iter().rev() {
                    let head_ref = Self::operand_repr(cx.f, e) == Repr::Ref;
                    let desc = self.list_desc(head_ref);
                    self.alloc(cx, ins, d, desc, 2);
                    self.store_slot(cx, ins, d, 0, e);
                    ins.local_get(cx.local(d));
                    ins.local_get(tmp);
                    ins.i32_store(mem(HDR + 8, 4));
                    ins.local_get(cx.local(d));
                    ins.local_set(tmp);
                }
                ins.local_get(tmp);
                ins.local_set(cx.local(d));
            }
            Value::ListIsEmpty(v) => {
                ins.local_get(cx.local(*v));
                ins.i32_eqz();
                self.assign(cx, ins, dst, Repr::I32);
            }
            Value::ListHead(v) => {
                if let Some(d) = dst {
                    let base = *v;
                    let repr = cx.f.vars[d].repr;
                    self.load_field(cx, ins, base, 0, repr);
                    ins.local_set(cx.local(d));
                }
            }
            Value::ListTail(v) => {
                let base = *v;
                ins.local_get(cx.local(base));
                ins.i32_load(mem(HDR + 8, 4));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            Value::FnRef(_) | Value::Closure(_) => {
                self.internal(format!("unconverted closure in {}", cx.f.name));
            }
            Value::MakeClosure { function, captures } => {
                let function = *function;
                if captures.is_empty() {
                    let o = self.static_closure(function);
                    if let Some(d) = dst {
                        ins.i32_const(o as i32);
                        ins.local_set(cx.local(d));
                    }
                    return;
                }
                let mut refs = vec![false];
                refs.extend(captures.iter().map(|&c| cx.f.vars[c].repr == Repr::Ref));
                let desc = self.desc(DescKey {
                    kind: 0,
                    tag: 0,
                    enum_id: -1,
                    name: check::display_name(&self.m.functions[function].name).to_owned(),
                    show: None,
                    refs,
                });
                let Some(d) = dst else {
                    self.internal(format!("closure without a destination in {}", cx.f.name));
                    return;
                };
                self.alloc(cx, ins, d, desc, captures.len() + 1);
                // The function's table index in slot 0.
                ins.local_get(cx.local(d));
                ins.i32_const(self.fn_table[ix(function)] as i32);
                ins.i32_store(mem(HDR, 4));
                let captures = captures.clone();
                for (i, c) in captures.iter().enumerate() {
                    self.store_slot(cx, ins, d, i + 1, &Operand::Var(*c));
                }
            }
            Value::EnvGet { env, index } => {
                if let Some(d) = dst {
                    let base = *env;
                    let repr = cx.f.vars[d].repr;
                    self.load_field(cx, ins, base, index + 1, repr);
                    ins.local_set(cx.local(d));
                }
            }
            Value::EvLookup { ev, effect } => {
                ins.local_get(cx.local(*ev));
                ins.i32_const(ix(*effect) as i32);
                let f = self.rt("ev_lookup");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            Value::EvExtend { base, bindings } => {
                let Some(d) = dst else {
                    self.internal(format!("evidence without a destination in {}", cx.f.name));
                    return;
                };
                let mut refs = vec![true, false];
                for _ in bindings {
                    refs.push(false);
                    refs.push(true);
                }
                let desc = self.desc(DescKey {
                    kind: 0,
                    tag: 0,
                    enum_id: -1,
                    name: "evidence".to_owned(),
                    show: None,
                    refs,
                });
                let bindings = bindings.clone();
                self.alloc(cx, ins, d, desc, 2 + 2 * bindings.len());
                // slot 0: the base table (or 0).
                ins.local_get(cx.local(d));
                match base {
                    Some(b) => {
                        ins.local_get(cx.local(*b));
                    }
                    None => {
                        ins.i32_const(0);
                    }
                }
                ins.i32_store(mem(HDR, 4));
                // slot 1: the number of bindings.
                ins.local_get(cx.local(d));
                ins.i32_const(bindings.len() as i32);
                ins.i32_store(mem(HDR + 8, 4));
                // slots 2..: the (effect, handler) pairs, most recent last.
                for (j, (effect, h)) in bindings.iter().enumerate() {
                    let off = HDR + (2 + 2 * j as u32) * 8;
                    ins.local_get(cx.local(d));
                    ins.i32_const(ix(*effect) as i32);
                    ins.i32_store(mem(off, 4));
                    ins.local_get(cx.local(d));
                    ins.local_get(cx.local(*h));
                    ins.i32_store(mem(off + 8, 4));
                }
            }
            Value::Handler { name, ops, .. } => {
                let mut refs = vec![false, true];
                refs.extend(std::iter::repeat_n(true, ops.len()));
                let defect = self
                    .string_literal(format!("handler {name} was used after release").as_bytes());
                let desc = self.desc(DescKey {
                    kind: 0,
                    tag: 0,
                    enum_id: -1,
                    name: name.clone(),
                    show: None,
                    refs,
                });
                let Some(d) = dst else {
                    self.internal(format!("handler without a destination in {}", cx.f.name));
                    return;
                };
                self.alloc(cx, ins, d, desc, ops.len() + 2);
                ins.local_get(cx.local(d));
                ins.i32_const(0);
                ins.i32_store(mem(HDR, 4));
                ins.local_get(cx.local(d));
                ins.i32_const(defect as i32);
                ins.i32_store(mem(HDR + 8, 4));
                let ops = ops.clone();
                for (i, o) in ops.iter().enumerate() {
                    self.store_slot(cx, ins, d, i + 2, &Operand::Var(*o));
                }
            }
            Value::Fiber { op, args, .. } => self.fiber(cx, ins, dst, *op, args),
            Value::IsError { error, enum_id } => {
                ins.local_get(cx.local(*error));
                ins.i32_const(ix(*enum_id) as i32);
                let f = self.rt("is_error");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            Value::AsError { error, .. } => {
                ins.local_get(cx.local(*error));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            Value::Runtime { op, args } => self.runtime_op(cx, ins, dst, *op, args),
            Value::Coerce(v) => {
                let Some(d) = dst else { return };
                let from = cx.f.vars[*v].repr;
                let to = cx.f.vars[d].repr;
                if from == to {
                    ins.local_get(cx.local(*v));
                } else if to == Repr::Ref {
                    // Box the value.
                    let desc = self.desc(DescKey {
                        kind: 0,
                        tag: 0,
                        enum_id: -1,
                        name: "box".to_owned(),
                        show: None,
                        refs: vec![from == Repr::Ref],
                    });
                    ins.i32_const(1);
                    ins.i32_const(desc as i32);
                    let alloc = self.rt("alloc");
                    ins.call(alloc);
                    ins.local_set(cx.local(d));
                    self.store_slot(cx, ins, d, 0, &Operand::Var(*v));
                    return;
                } else {
                    // Unbox.
                    ins.local_get(cx.local(*v));
                    match to {
                        Repr::I32 | Repr::Ref => ins.i32_load(mem(HDR, 4)),
                        Repr::I64 => ins.i64_load(mem(HDR, 8)),
                        Repr::F64 => ins.f64_load(mem(HDR, 8)),
                        Repr::Void => unreachable!(),
                    };
                }
                ins.local_set(cx.local(d));
            }
        }
    }

    /// Loads slot `index` of the object in local `base`, as `repr`.
    fn load_field(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        base: VarId,
        index: usize,
        repr: Repr,
    ) {
        let off = HDR + index as u32 * 8;
        ins.local_get(cx.local(base));
        match repr {
            Repr::I32 | Repr::Ref => {
                ins.i32_load(mem(off, 4));
            }
            Repr::I64 => {
                ins.i64_load(mem(off, 8));
            }
            Repr::F64 => {
                ins.f64_load(mem(off, 8));
            }
            Repr::Void => {}
        }
    }

    fn alloc_value(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        desc: u32,
        args: &[Operand],
    ) {
        let Some(d) = dst else {
            self.internal(format!("allocation without a destination in {}", cx.f.name));
            return;
        };
        self.alloc(cx, ins, d, desc, args.len());
        for (i, a) in args.iter().enumerate() {
            self.store_slot(cx, ins, d, i, a);
        }
    }

    fn prim(&mut self, ins: &mut InstructionSink, op: PrimOp, ty: PrimTy) {
        use PrimOp::*;
        match ty {
            PrimTy::I32 => match op {
                Add => {
                    ins.i32_add();
                }
                Sub => {
                    ins.i32_sub();
                }
                Mul => {
                    ins.i32_mul();
                }
                Div => {
                    let f = self.rt("idiv");
                    ins.call(f);
                }
                Rem => {
                    let f = self.rt("irem");
                    ins.call(f);
                }
                Neg => {
                    let f = self.rt("ineg");
                    ins.call(f);
                }
                Not => {
                    ins.i32_eqz();
                }
                Eq => {
                    ins.i32_eq();
                }
                Ne => {
                    ins.i32_ne();
                }
                Lt => {
                    ins.i32_lt_s();
                }
                Le => {
                    ins.i32_le_s();
                }
                Gt => {
                    ins.i32_gt_s();
                }
                Ge => {
                    ins.i32_ge_s();
                }
            },
            PrimTy::I64 => match op {
                Add => {
                    ins.i64_add();
                }
                Sub => {
                    ins.i64_sub();
                }
                Mul => {
                    ins.i64_mul();
                }
                Div => {
                    let f = self.rt("ldiv");
                    ins.call(f);
                }
                Rem => {
                    let f = self.rt("lrem");
                    ins.call(f);
                }
                Neg => {
                    let f = self.rt("lneg");
                    ins.call(f);
                }
                Not => {
                    ins.i64_eqz();
                }
                Eq => {
                    ins.i64_eq();
                }
                Ne => {
                    ins.i64_ne();
                }
                Lt => {
                    ins.i64_lt_s();
                }
                Le => {
                    ins.i64_le_s();
                }
                Gt => {
                    ins.i64_gt_s();
                }
                Ge => {
                    ins.i64_ge_s();
                }
            },
            PrimTy::F64 => match op {
                Add => {
                    ins.f64_add();
                }
                Sub => {
                    ins.f64_sub();
                }
                Mul => {
                    ins.f64_mul();
                }
                Div => {
                    ins.f64_div();
                }
                Rem => {
                    let f = self.rt("fmod");
                    ins.call(f);
                }
                Neg => {
                    ins.f64_neg();
                }
                Not => {
                    ins.f64_const(0.0.into());
                    ins.f64_eq();
                }
                Eq => {
                    ins.f64_eq();
                }
                Ne => {
                    ins.f64_ne();
                }
                Lt => {
                    ins.f64_lt();
                }
                Le => {
                    ins.f64_le();
                }
                Gt => {
                    ins.f64_gt();
                }
                Ge => {
                    ins.f64_ge();
                }
            },
            PrimTy::String => match op {
                Eq => {
                    let f = self.rt("str_eq");
                    ins.call(f);
                }
                Ne => {
                    let f = self.rt("str_eq");
                    ins.call(f);
                    ins.i32_eqz();
                }
                _ => {
                    // Compare by code point (utf-8 byte order is the same).
                    let f = self.rt("str_cmp");
                    ins.call(f);
                    ins.i32_const(0);
                    match op {
                        Lt => {
                            ins.i32_lt_s();
                        }
                        Le => {
                            ins.i32_le_s();
                        }
                        Gt => {
                            ins.i32_gt_s();
                        }
                        Ge => {
                            ins.i32_ge_s();
                        }
                        _ => {}
                    }
                }
            },
        }
    }

    fn call(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        callee: &Callee,
        args: &[Operand],
    ) {
        match callee {
            Callee::Direct(g) => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let index = self.fn_index(*g);
                ins.call(index);
                let ret = self.repr(self.m.functions[*g].ret);
                self.assign(cx, ins, dst, ret);
            }
            Callee::Indirect(c) => {
                let fty = match self.core().ty(cx.f.vars[*c].ty) {
                    Type::Fn(fty) => fty.clone(),
                    _ => {
                        self.internal(format!(
                            "calling a value that is not a function in {}",
                            cx.f.name
                        ));
                        return;
                    }
                };
                let ret = self.repr(fty.ret);
                self.indirect(cx, ins, dst, *c, args, ret);
            }
            Callee::Op {
                handler,
                effect,
                op,
            } => {
                let def = self.core().effects[*effect].ops[*op].clone();
                let ret = self.repr(def.ret);
                self.op_call(cx, ins, dst, *handler, *op, args, ret, &def);
            }
        }
    }

    fn indirect(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        closure: VarId,
        args: &[Operand],
        ret: Repr,
    ) {
        // The closure record is the environment: it goes first, then the arguments, then the
        // table index.
        let mut params = vec![Repr::Ref];
        params.extend(args.iter().map(|a| Self::operand_repr(cx.f, a)));
        let ty = self.call_ty(&params, ret);
        ins.local_get(cx.local(closure));
        for a in args {
            self.operand(cx, ins, a);
        }
        ins.local_get(cx.local(closure));
        ins.i32_load(mem(HDR, 4));
        ins.call_indirect(0, ty);
        self.assign(cx, ins, dst, ret);
    }

    #[allow(clippy::too_many_arguments)]
    fn op_call(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        handler: VarId,
        op: usize,
        args: &[Operand],
        ret: Repr,
        def: &emela_core::OpDef,
    ) {
        let suspend = def.suspend;
        // A `suspend fn` implementation takes the resume function last and returns nothing; the
        // value to continue with comes from `perform_end` (spec 8.4, 14.5).
        let mut params = vec![Repr::Ref];
        params.extend(args.iter().map(|a| Self::operand_repr(cx.f, a)));
        if suspend {
            params.push(Repr::Ref);
        }
        let call_ret = if suspend { Repr::Void } else { ret };
        let ty = self.call_ty(&params, call_ret);
        let op_off = HDR + 8 * (op as u32 + 2);
        // If the handler was released, its operations are a defect (spec 8.6).
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(HDR, 4));
        ins.if_(BlockType::Empty);
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(HDR + 8, 4));
        let raise = self.rt("raise");
        ins.call(raise);
        dummy_return(ins, cx.ret);
        ins.else_();
        let rs = if suspend {
            let rs = cx.scratch(Repr::Ref, 0);
            let prefix =
                self.string_literal(format!("operation `{}` of handler `", def.name).as_bytes());
            let resume = self.resume_table(ret);
            ins.i32_const(prefix as i32);
            ins.local_get(cx.local(handler));
            ins.i32_const(resume as i32);
            let f = self.rt("perform_new");
            ins.call(f);
            ins.local_set(rs);
            Some(rs)
        } else {
            None
        };
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        for a in args {
            self.operand(cx, ins, a);
        }
        if let Some(rs) = rs {
            ins.local_get(rs);
        }
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        ins.i32_load(mem(HDR, 4));
        ins.call_indirect(0, ty);
        if let Some(rs) = rs {
            let end = self.rt("perform_end");
            ins.local_get(rs);
            ins.call(end);
            self.perform_result(ins, ret);
        }
        self.assign(cx, ins, dst, ret);
        ins.end();
    }

    /// `Fiber.attempt` and the operations of `Std.Cause` (spec 11.5), with the runtime of
    /// `runtime::attempt`. The other concurrency operations are refused by `check` before this.
    fn fiber(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        op: FiberOp,
        args: &[Operand],
    ) {
        match op {
            FiberOp::Attempt { suspends, .. } => {
                let Some(Operand::Var(f)) = args.first() else {
                    self.internal(format!("attempt without a function in {}", cx.f.name));
                    return;
                };
                let ret = match self.core().ty(cx.f.vars[*f].ty) {
                    Type::Fn(fty) => self.repr(fty.ret),
                    _ => {
                        self.internal(format!(
                            "attempt of a value that is not a function in {}",
                            cx.f.name
                        ));
                        return;
                    }
                };
                // Call the function in place; its value goes to a word, as a slot stores it.
                let word = cx.scratch(Repr::I64, 0);
                let exit = cx.scratch(Repr::Ref, 0);
                let ty = self.call_ty(&[Repr::Ref], ret);
                ins.local_get(cx.local(*f));
                ins.local_get(cx.local(*f));
                ins.i32_load(mem(HDR, 4));
                ins.call_indirect(0, ty);
                match ret {
                    Repr::I32 | Repr::Ref => {
                        ins.i64_extend_i32_u();
                    }
                    Repr::I64 => {}
                    Repr::F64 => {
                        ins.i64_reinterpret_f64();
                    }
                    Repr::Void => {
                        ins.i64_const(0);
                    }
                }
                ins.local_set(word);
                let success = runtime::attempt::success_desc(self, ret);
                if suspends {
                    // The function suspended: the attempt's frame goes after the callee's, and
                    // makes the `Exit` when the continuation is resumed. The suspension edge of
                    // the statement pushes the caller's frame next.
                    let frame = runtime::attempt::frame_desc(self);
                    let table = self.sus.as_ref().map_or(0, |p| p.attempt_table);
                    ins.global_get(G_SUSPENDING);
                    ins.if_(BlockType::Result(ValType::I32));
                    ins.i32_const(frame as i32);
                    ins.i32_const(table as i32);
                    ins.i32_const(success as i32);
                    ins.i32_const(0);
                    let new = self.rt("frame_new");
                    ins.call(new);
                    let push = self.rt("frame_push");
                    ins.call(push);
                    ins.i32_const(0);
                    ins.else_();
                }
                let caught = self.rt("attempt_caught");
                ins.call(caught);
                ins.local_tee(exit);
                ins.if_(BlockType::Result(ValType::I32));
                ins.local_get(exit);
                ins.else_();
                // Nothing caught: a value, or a defect that is not caught (then no value).
                ins.global_get(G_DEFECT);
                ins.if_(BlockType::Result(ValType::I32));
                ins.i32_const(0);
                ins.else_();
                ins.i32_const(success as i32);
                ins.local_get(word);
                let make = self.rt("exit_success");
                ins.call(make);
                ins.end();
                ins.end();
                if suspends {
                    ins.end();
                }
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::CauseKind { kind } => {
                if enum_repr(self.core(), kind) != Repr::I32 {
                    self.internal("`CauseKind` is not an enum of plain variants");
                    return;
                }
                self.operand(cx, ins, &args[0]);
                ins.i32_load(mem(4, 4));
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::CauseToString => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("cause_to_string");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::CauseRaise { .. } => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("cause_raise");
                ins.call(f);
            }
            _ => self.internal(format!(
                "fiber operation `{}` left in {}",
                op.name(),
                cx.f.name
            )),
        }
    }

    /// The resume function table index for an operation returning `ret`.
    fn resume_table(&mut self, ret: Repr) -> u32 {
        match &self.sus {
            Some(plan) => plan.resume_table(ret),
            None => {
                self.internal("a `suspend fn` operation without the suspend runtime");
                0
            }
        }
    }

    /// Leaves the value `perform_end` produced (or a dummy when it suspended or a pending
    /// failure or defect is set).
    fn perform_result(&mut self, ins: &mut InstructionSink, ret: Repr) {
        if ret == Repr::Void {
            return;
        }
        let Some(plan) = &self.sus else { return };
        let out_mode = plan.out_mode;
        ins.i32_const(out_mode as i32);
        ins.i32_load8_u(mem(0, 1));
        ins.i32_eqz();
        ins.if_(BlockType::Result(val(ret)));
        ins.i32_const(plan.out_word as i32);
        match ret {
            Repr::I32 | Repr::Ref => {
                ins.i32_load(mem(0, 4));
            }
            Repr::I64 => {
                ins.i64_load(mem(0, 8));
            }
            Repr::F64 => {
                ins.f64_load(mem(0, 8));
            }
            Repr::Void => unreachable!(),
        }
        ins.else_();
        match ret {
            Repr::I32 | Repr::Ref => {
                ins.i32_const(0);
            }
            Repr::I64 => {
                ins.i64_const(0);
            }
            Repr::F64 => {
                ins.f64_const(0.0.into());
            }
            Repr::Void => unreachable!(),
        }
        ins.end();
    }
}

/// A switch's scrutinee.
#[derive(Clone, Copy)]
enum Scrutinee {
    Tag(VarId),
    Lit(VarId),
}

/// The representation of a value that may suspend. The suspension edge needs it when the value is
/// unused (`dst` is `None`) and the resumed reference must be released.
fn value_repr(b: &Builder<'_>, cx: &FnCtx<'_>, value: &Value) -> Repr {
    let Value::Call { callee, .. } = value else {
        return Repr::Void;
    };
    match callee {
        Callee::Direct(g) => b.repr(b.m.functions[*g].ret),
        Callee::Op { effect, op, .. } => b.repr(b.core().effects[*effect].ops[*op].ret),
        Callee::Indirect(c) => match b.core().ty(cx.f.vars[*c].ty) {
            Type::Fn(fty) => b.repr(fty.ret),
            _ => Repr::Void,
        },
    }
}

fn value_on_fail(v: &Value) -> Option<&OnFail> {
    match v {
        Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } => on_fail.as_ref(),
        _ => None,
    }
}

fn mem(offset: u32, align_bytes: u32) -> MemArg {
    MemArg {
        offset: offset as u64,
        align: align_bytes.trailing_zeros(),
        memory_index: 0,
    }
}

fn dummy_return(ins: &mut InstructionSink, ret: Repr) {
    match ret {
        Repr::Void => {
            ins.return_();
        }
        Repr::I32 | Repr::Ref => {
            ins.i32_const(0);
            ins.return_();
        }
        Repr::I64 => {
            ins.i64_const(0);
            ins.return_();
        }
        Repr::F64 => {
            ins.f64_const(0.0.into());
            ins.return_();
        }
    }
}

/// A wasm label on the stack, for computing `br` depths.
enum Lbl {
    Block(LabelId, Option<VarId>),
    Loop(Vec<VarId>),
    Other,
}

struct FnCtx<'a> {
    f: &'a Function,
    ret: Repr,
    local: Vec<u32>,
    scratch_base: [u32; 4],
    decls: Vec<(u32, ValType)>,
    labels: Vec<Lbl>,
}

impl<'a> FnCtx<'a> {
    fn new(f: &'a Function, b: &Builder<'_>) -> Self {
        Self::with_extra_params(f, b, 0)
    }

    /// The context of a function whose wasm signature has `extra` parameters after the IR ones
    /// (a state machine's frame pointer). They occupy the locals right after the parameters.
    fn with_extra_params(f: &'a Function, b: &Builder<'_>, extra: u32) -> Self {
        let nparams = f.params.len() as u32;
        let mut local = vec![0u32; f.vars.len()];
        let mut decls: Vec<(u32, ValType)> = Vec::new();
        for (i, &p) in f.params.iter().enumerate() {
            local[ix(p)] = i as u32;
        }
        let mut next = nparams + extra;
        for (v, data) in f.vars.iter() {
            if !f.params.contains(&v) {
                local[ix(v)] = next;
                next += 1;
                decls.push((1, val(data.repr)));
            }
        }
        // Scratch locals, one group per representation: for the `recur` parallel assignment and
        // the general temporaries. At least one of each so a temporary always exists.
        let count = max_recur(f).max(1);
        let mut scratch_base = [0u32; 4];
        for base in scratch_base.iter_mut() {
            *base = next;
            next += count;
        }
        decls.push((count, ValType::I32));
        decls.push((count, ValType::I64));
        decls.push((count, ValType::F64));
        decls.push((count, ValType::I32));
        let ret = b.repr(f.ret);
        FnCtx {
            f,
            ret,
            local,
            scratch_base,
            decls,
            labels: Vec::new(),
        }
    }

    fn local(&self, v: VarId) -> u32 {
        self.local[ix(v)]
    }

    fn scratch(&self, r: Repr, i: usize) -> u32 {
        let base = match r {
            Repr::I32 => self.scratch_base[0],
            Repr::I64 => self.scratch_base[1],
            Repr::F64 => self.scratch_base[2],
            Repr::Ref => self.scratch_base[3],
            Repr::Void => self.scratch_base[0],
        };
        base + i as u32
    }

    fn push(&mut self, l: Lbl) {
        self.labels.push(l);
    }

    fn pop(&mut self) {
        self.labels.pop();
    }

    fn find_block(&self, label: LabelId) -> Option<(u32, Option<VarId>)> {
        for (i, l) in self.labels.iter().enumerate().rev() {
            if let Lbl::Block(id, dst) = l
                && *id == label
            {
                let depth = (self.labels.len() - 1 - i) as u32;
                return Some((depth, *dst));
            }
        }
        None
    }

    /// The nearest enclosing loop: its branch depth and its variables.
    fn find_loop(&self) -> Option<(u32, Vec<VarId>)> {
        for (i, l) in self.labels.iter().enumerate().rev() {
            if let Lbl::Loop(vars) = l {
                return Some(((self.labels.len() - 1 - i) as u32, vars.clone()));
            }
        }
        None
    }
}

fn max_recur(f: &Function) -> u32 {
    let mut max = 0;
    for (_, seq) in f.seqs.iter() {
        if let Term::Recur(args) = &seq.term {
            max = max.max(args.len() as u32);
        }
    }
    max
}
