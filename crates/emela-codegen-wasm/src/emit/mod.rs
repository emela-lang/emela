//! The emitter: Heap IR (stage `RefCounted`) to WebAssembly.
//!
//! The Heap IR already says everything the backend needs: the evaluation order (ANF), where
//! control goes (structured statements, failure and defect landings) and when references are
//! added and given up (`dup` and `drop`). The emitter follows it mechanically, like the C
//! emitter:
//!
//! - Every IR variable is a wasm local. A compound statement's `dst` is its local; `yield` sets
//!   it, `break` sets the block's and branches, `recur` sets the loop variables and branches to
//!   the loop head.
//! - A call's defect landing is `if (em_defect != 0) { … }` after the call, and its failure
//!   landing `if (em_failure != 0) { … }`. `unwind` and `fail` return a dummy value.
//! - Heap objects have descriptors (`em_d…`) per shape: the tag, whether it is an error value and
//!   which slots hold references (for `drop`). Constructors without fields, string literals and
//!   closures without captures are static objects (rc == 0), which reference counting skips.
//! - The runtime (Perceus `dup`/`drop`, the allocator, strings, the numeric conversions and the
//!   WASI host calls) is emitted into the same module (see `runtime`).

mod builtins;
pub(crate) mod external;
mod fiber;
mod handle;
pub mod layout;
mod runtime;
mod runtimeop;
mod suspend;

use emela_core::{Builtin, EnumId, HostModule, HostRef, OPTION_NONE, OPTION_SOME, Type, TypeId};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, Module, OnFail, Operand, PrimOp, PrimTy, Repr,
    RuntimeOp, SeqId, Stage, Stmt, StmtId, SwitchOn, Term, Value, VarId, enum_repr, repr_of,
};
use rustc_hash::FxHashMap;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, Function as WasmFunction, FunctionSection, GlobalSection,
    GlobalType, ImportSection, InstructionSink, MemArg, MemorySection, MemoryType,
    Module as WasmModule, RefType, TableSection, TableType, TypeSection, ValType,
};

use crate::check;
use crate::{WasmError, WasmErrorKind, WasmOutput};

/// What the generated `_start` runs.
#[allow(dead_code)]
pub(crate) enum Run {
    /// main, as its monomorphic copy (when the program has one).
    Main(Option<FnId>),
    /// Test functions, with their names.
    Tests(Vec<(String, FnId)>),
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
    let (module, layout) = b.finish();
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
/// Set while a suspending function unwinds (the state machines push their frames and return).
const G_SUSPENDING: u32 = 4;
const G_STRDESC: u32 = 5;
/// The globals every program has; the suspend runtime appends its own after these.
const BASE_GLOBALS: u32 = 6;
/// The `i64` resume input of a suspended frame (a `resume`/`yield` delivered it as raw bits).
const G_RESUME: u32 = 6;
/// The head and tail of the captured continuation (a list of frames, innermost first).
const G_SUSP_HEAD: u32 = 7;
const G_SUSP_TAIL: u32 = 8;
/// The request the current suspension is waiting on (a `perform`, or 0), or 0.
const G_SUSP_WAIT: u32 = 9;
/// How many `sync` calls (a suspending call in a non-suspending function) are under way.
const G_SYNC_DEPTH: u32 = 10;
/// The address of the `interrupted` descriptor (spec 14.5): a pending defect that is it ends the
/// run as interrupted (exit 130).
const G_INTERRUPTED: u32 = 11;
/// A defect that ended a `sync` call (a failure after resume, spec 8.7).
const G_SYNC_DEFECT: u32 = 12;
/// The run under way (the `em_root`), or 0 (spec 14.5).
const G_ROOT: u32 = 13;
/// The fiber being stepped (the `em_fiber`), or 0.
const G_CUR: u32 = 14;
/// Whether the stepped fiber runs its main continuation (not an operation implementation's).
const G_CUR_MAIN: u32 = 15;
/// The identity of the run under way (`em_run_id`): a resume after the run is ignored.
const G_RUN_ID: u32 = 16;
/// The first defect of a continuation unwound when the run stalled that was not a fiber's
/// (kept in the root's `orphan` slot).
/// The note tables of spec 11.2 (`em_note_errors`/`em_note_lines`), as vector pointers.
const G_NOTE_ERRORS: u32 = 17;
const G_NOTE_LINES: u32 = 18;
/// The scheduler's timers (a vector of timer records; see `runtime::sched`) and the sequence
/// number that orders equal firing times, for the driver loop (spec 12.6).
const G_TIMERS: u32 = 19;
const G_TIMER_SEQ: u32 = 20;
/// The number of globals of a program that suspends.
const SUSPEND_GLOBALS: u32 = 21;

/// The object header (see [`HDR`]) fields a frame uses: state, next, and the first saved slot.
const FRAME_STATE: u32 = HDR + 8;
const FRAME_NEXT: u32 = HDR + 16;
const FRAME_SAVED: u32 = HDR + 24;

/// The static cell of a const (spec 5.6): 0 = the state (0 not evaluated, 1 being evaluated,
/// 2 the value, 3 the defect), 8 = the remembered value (a word), 16 = the remembered defect,
/// 24 = the message of a self-reference, 32 = the prefix of a defect raised while it is evaluated.
const CELL_STATE: u32 = 0;
const CELL_VALUE: u32 = 8;
const CELL_DEFECT: u32 = 16;
const CELL_SELF_MSG: u32 = 24;
const CELL_PREFIX: u32 = 32;
const CELL_SIZE: u32 = 40;

/// Object header: rc at 0, `aux` at 4, the descriptor pointer at 8, slots at 16. A `rc` of 0
/// marks static data (never counted).
const HDR: u32 = 16;
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
    /// Runtime functions requested by an internal host binding (`emit::external::INTERNAL_HOSTS`).
    /// They are registered on demand so a program that uses no internal host keeps the exact
    /// bytes it had before the function existed.
    pub(crate) requested_hosts: Vec<&'static str>,
    /// Internal host bindings seen before the runtime index space was fixed: `(fn_index slot,
    /// module, name)`, resolved by `external::bind_internal_hosts`.
    pub(crate) deferred_hosts: Vec<(usize, String, String)>,
    /// The `show` function of each error enum (spec 10.6), as a table index for the descriptor.
    pub(crate) error_shows: FxHashMap<EnumId, u32>,
    pub(crate) entry_index: u32,
    pub(crate) str_desc: u32,
    /// The descriptors of a list cell whose head is a reference, and one whose head is a word
    /// (the runtime's `String.split` / `String.code_points` build lists).
    pub(crate) cons_ref_desc: u32,
    pub(crate) cons_val_desc: u32,
    pub(crate) data: Vec<u8>,
    descs: FxHashMap<DescKey, u32>,
    strs: FxHashMap<Vec<u8>, u32>,
    nullary: FxHashMap<u32, u32>,
    static_closures: FxHashMap<FnId, u32>,
    pub(crate) errors: Vec<WasmError>,
    pub(crate) bodies: Vec<WasmFunction>,
    /// Whether the module reaches the suspend machinery (a suspending function, a suspending call
    /// or a `yield`/`hold`/`unhold`): the scheduler/state-machine runtime is then linked, and the
    /// suspend globals are emitted. A program without it keeps the exact bytes it had before.
    pub(crate) uses_suspend: bool,
    /// Whether a host that uses the scheduler's timers is requested: the driver's timer functions
    /// are linked and WASI's `clock_time_get` is imported. A program without one keeps its exact
    /// bytes.
    pub(crate) uses_timers: bool,
    /// The function index of the WASI `clock_time_get` import (0 when `uses_timers` is false).
    pub(crate) clock_time_get_fn: u32,
    /// The number of globals the module emits (see [`BASE_GLOBALS`]).
    globals: u32,
    /// The state-machine functions: their body (`f_sm`), resume wrapper and table index.
    machine: FxHashMap<usize, MachineInfo>,
    /// The table index of the resume closures, by [`repr_code`].
    resume_closure: [u32; 5],
    /// The table indices of the fiber runtime's frame resume functions (`scope_body`,
    /// `scope_close`, `attempt`), in that order.
    pub(crate) fiber_resume: [u32; 3],
    /// The table index of the root-entry thunk of the run (`_start`'s argument to `run`).
    ///
    /// Set from [`thunk_tables`](Builder::thunk_tables) but not consumed yet: `_start` reads
    /// the entry thunk from the table directly. Kept, like the C layout, until the run path
    /// uses it (W5-7).
    pub(crate) entry_thunk: u32,
    /// The entry functions of the run (main, or the tests), each with a thunk in the table.
    pub(crate) entry_ids: Vec<FnId>,
    /// The table indices of the entry thunks, one per [`entry_ids`](Builder::entry_ids).
    pub(crate) thunk_tables: Vec<u32>,
    /// The descriptor of the `interrupted` object (spec 14.5).
    pub(crate) interrupted_desc: u32,
    /// Whether the module has a const (`Function::constant`): the const helpers and the
    /// self-reference global (`self_reference`) are emitted only then, so other programs keep the
    /// bytes they had before (see `runtime::constant`).
    uses_const: bool,
    /// Whether the module can record a release-failure note (a handler is retired, spec 14.4): the
    /// name global (`last_retired`), the note strings and the note code are emitted only then.
    uses_notes: bool,
    /// The global of the const self-reference flag (spec 5.6), when [`Self::uses_const`].
    self_reference: u32,
    /// The global holding the name of the handler retired most recently (spec 14.4), when
    /// [`Self::uses_notes`].
    last_retired: u32,
    /// The static cell of each const function, by its index (the state, value and defect).
    const_cell: FxHashMap<usize, u32>,
    /// The init body of each const function (the wrapper takes its [`Self::fn_index`] slot).
    const_init: FxHashMap<usize, u32>,
}

/// A suspending function lowered as a state machine: its body is `f_sm` (entered with a `frame`),
/// its resume wrapper `f_resume` is called by the driver loop (via the table), and the wrapper `f`
/// enters the body without a frame.
#[derive(Clone, Copy)]
pub(crate) struct MachineInfo {
    /// The function index of the body (`f_sm`).
    pub(crate) body: u32,
    /// The function index of the resume wrapper (`f_resume`).
    #[allow(dead_code)]
    pub(crate) resume: u32,
    /// The table index of the resume wrapper (stored in the frames).
    pub(crate) table: u32,
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
            requested_hosts: Vec::new(),
            deferred_hosts: Vec::new(),
            error_shows: FxHashMap::default(),
            entry_index: 0,
            str_desc: 0,
            cons_ref_desc: 0,
            cons_val_desc: 0,
            data: vec![0; layout::STATIC_START as usize],
            descs: FxHashMap::default(),
            strs: FxHashMap::default(),
            nullary: FxHashMap::default(),
            static_closures: FxHashMap::default(),
            errors: Vec::new(),
            bodies: Vec::new(),
            uses_suspend: uses_suspend(m),
            uses_timers: false,
            clock_time_get_fn: 0,
            globals: BASE_GLOBALS,
            machine: FxHashMap::default(),
            resume_closure: [0; 5],
            fiber_resume: [0; 3],
            entry_thunk: 0,
            entry_ids: Vec::new(),
            thunk_tables: Vec::new(),
            interrupted_desc: 0,
            uses_const: has_const(m),
            uses_notes: has_release(m),
            self_reference: 0,
            last_retired: 0,
            const_cell: FxHashMap::default(),
            const_init: FxHashMap::default(),
        };
        b.str_desc = b.string_desc();
        b.cons_ref_desc = b.list_desc(true);
        b.cons_val_desc = b.list_desc(false);
        if b.uses_suspend {
            b.globals = SUSPEND_GLOBALS;
            b.interrupted_desc = b.desc(DescKey {
                kind: 0,
                tag: 0,
                enum_id: -1,
                name: "interrupted".to_owned(),
                show: None,
                refs: vec![true],
            });
        }
        // The globals the note and const machinery need, appended after the base and suspend ones
        // so that programs without them keep the exact bytes they had.
        if b.uses_const {
            b.self_reference = b.globals;
            b.globals += 1;
        }
        if b.uses_notes {
            b.last_retired = b.globals;
            b.globals += 1;
        }
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

    fn wasi_import(&mut self, name: &str, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        let module = "wasi_snapshot_preview1".to_owned();
        if let Some(&i) = self.import_map.get(&(module.clone(), name.to_owned())) {
            return i;
        }
        let ty = self.ty(params, results);
        let i = self.imports.len() as u32;
        self.imports.push((module.clone(), name.to_owned(), ty));
        self.import_map.insert((module, name.to_owned()), i);
        i
    }

    pub(crate) fn fd_write(&mut self) -> u32 {
        self.wasi_import(
            "fd_write",
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    pub(crate) fn proc_exit(&mut self) -> u32 {
        self.wasi_import("proc_exit", vec![ValType::I32], vec![])
    }

    /// `environ_sizes_get(count_ptr, buf_size_ptr)`: the number of environment variables and the
    /// total byte size of their `NAME=VALUE\0` entries. Used by `Std.Env`'s internal hosts.
    pub(crate) fn environ_sizes_get(&mut self) -> u32 {
        self.wasi_import(
            "environ_sizes_get",
            vec![ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// `environ_get(env_ptr, buf_ptr)`: the pointers of the `NAME=VALUE\0` entries into the
    /// buffer. Used by `Std.Env`'s internal hosts.
    pub(crate) fn environ_get(&mut self) -> u32 {
        self.wasi_import(
            "environ_get",
            vec![ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// `random_get(buf, buf_len)`: cryptographically secure random bytes. Used by `Std.Random`'s
    /// internal hosts.
    pub(crate) fn random_get(&mut self) -> u32 {
        self.wasi_import(
            "random_get",
            vec![ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// `clock_time_get(clock_id, precision, time_ptr)`: the time of `clock_id` (0 = monotonic) in
    /// nanoseconds, written to `time_ptr`. Used by the scheduler's timers (the `now_ms` runtime
    /// function); registered on demand when a host that uses them is requested.
    pub(crate) fn clock_time_get(&mut self) -> u32 {
        self.wasi_import(
            "clock_time_get",
            vec![ValType::I32, ValType::I64, ValType::I32],
            vec![ValType::I32],
        )
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
        // indices.
        for (id, f) in self.m.functions.iter() {
            if let Some(ext) = &f.external
                && let Some(binding) = ext.binding("wasm")
            {
                self.external_import(id, f, binding);
            }
        }
        // The WASI host calls the runtime always uses (their indices must be fixed before the
        // function index space is assigned).
        self.fd_write();
        self.proc_exit();
        // The WASI calls only some internal hosts need. They are added here, before the index
        // space is fixed, but only when the host that uses them was requested, so programs
        // without it keep their exact bytes (`emit::external::INTERNAL_HOSTS`).
        if self.requested_hosts.contains(&"env_has") || self.requested_hosts.contains(&"env_get") {
            self.environ_sizes_get();
            self.environ_get();
        }
        if self.requested_hosts.contains(&"random_int")
            || self.requested_hosts.contains(&"random_float")
        {
            self.random_get();
        }
        // The clock hosts need WASI's clock (the `clock_time_get` import): added before the index
        // space is fixed, but only when a host that reads it is requested, so a program without
        // one keeps its exact bytes (see `emit::external::CLOCK_HOSTS`). The separate
        // `TIMER_HOSTS` list is the subset that also uses the scheduler's timers, whose globals
        // exist only in a suspending module: requesting one therefore implies `uses_suspend`
        // (a `suspend fn` host; `check` rejects every other binding), which `register` relies on.
        self.uses_timers = self
            .requested_hosts
            .iter()
            .any(|h| external::CLOCK_HOSTS.contains(h));
        debug_assert!(
            !self
                .requested_hosts
                .iter()
                .any(|h| external::TIMER_HOSTS.contains(h))
                || self.uses_suspend,
            "a host that uses the scheduler's timers is requested without the suspend globals"
        );
        if self.uses_timers {
            self.clock_time_get_fn = self.clock_time_get();
        }
        // The WASI calls the `Std.FileSystem` internal hosts need (only when one is requested).
        self.fs_imports();
        // The IR definitions, in arena order (the entries of the table are the IR functions).
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
            // A const gets a second slot for its body, which the wrapper (`fn_index`) evaluates
            // once and remembers (spec 5.6). Callers and the table use the wrapper.
            if f.constant && !is_machine(f) {
                let init = self.imports.len() as u32 + self.defs.len() as u32;
                self.defs.push(ty);
                self.const_init.insert(ix(id), init);
            }
        }
        // The static cells of the consts (their state, value and defect).
        for (id, f) in self.m.functions.iter() {
            if !f.constant || is_machine(f) {
                continue;
            }
            let cell = self.const_cell_for(f);
            self.const_cell.insert(ix(id), cell);
        }
        // The state machines: each suspending function with suspension points gets a body entered
        // with a frame (`f_sm`) and a resume wrapper (`f_resume`, in the table for the driver).
        if self.uses_suspend {
            for (id, f) in self.m.functions.iter() {
                if self.imported(id) || !is_machine(f) {
                    continue;
                }
                let mut params: Vec<ValType> =
                    f.params.iter().map(|&p| val(f.vars[p].repr)).collect();
                params.push(ValType::I32);
                let results: Vec<ValType> = repr_val(self.repr(f.ret)).into_iter().collect();
                let body_ty = self.ty(params, results);
                let body = self.imports.len() as u32 + self.defs.len() as u32;
                self.defs.push(body_ty);
                let resume_ty = self.ty(vec![ValType::I32], vec![ValType::I64]);
                let resume = self.imports.len() as u32 + self.defs.len() as u32;
                self.defs.push(resume_ty);
                let table = self.table_funcs.len() as u32;
                self.table_funcs.push(resume);
                self.machine.insert(
                    ix(id),
                    MachineInfo {
                        body,
                        resume,
                        table,
                    },
                );
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
        runtime::register(self);
        // The resume closures are runtime functions the resume values call through the table.
        if self.uses_suspend {
            for (code, name) in [
                (0, "resume_i32"),
                (1, "resume_i64"),
                (2, "resume_f64"),
                (3, "resume_ref"),
                (4, "resume_void"),
            ] {
                let index = self.rt(name);
                self.resume_closure[code] = self.table_funcs.len() as u32;
                self.table_funcs.push(index);
            }
            // The frames of the fiber runtime resume through these (`fiber_scope` and
            // `fiber_attempt`).
            for (slot, name) in [
                (0, "scope_body_resume"),
                (1, "scope_close_resume"),
                (2, "attempt_resume"),
            ] {
                let index = self.rt(name);
                self.fiber_resume[slot] = self.table_funcs.len() as u32;
                self.table_funcs.push(index);
            }
        }
        // The internal host bindings are runtime functions whose index is fixed now.
        self.bind_internal_hosts();
        self.entry_ids = match entry {
            Run::Main(main) => main.iter().copied().collect(),
            Run::Tests(tests) => tests.iter().map(|(_, id)| *id).collect(),
        };
        // The entry thunks (the run calls one through the table; see `emit_entry`). They come
        // before `_start` in the function index space.
        if self.uses_suspend {
            let thunk_ty = self.ty(vec![], vec![ValType::I64]);
            for _ in &self.entry_ids {
                let func = self.imports.len() as u32 + self.defs.len() as u32;
                self.thunk_tables.push(self.table_funcs.len() as u32);
                self.table_funcs.push(func);
                self.defs.push(thunk_ty);
            }
        }
        // One entry (`_start`).
        let ty = self.ty(vec![], vec![]);
        self.entry_index = self.imports.len() as u32 + self.defs.len() as u32;
        self.defs.push(ty);

        self.emit_ir_bodies();
        runtime::emit(self);
        self.emit_entry(entry);
    }

    // ---- statics ----

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
        if let Some(&o) = self.strs.get(bytes) {
            return o;
        }
        let o = self.alloc_static(HDR + 4 + bytes.len() as u32 + 1);
        let desc = self.string_desc();
        self.write_u32(o + 8, desc);
        self.write_u32(o + HDR, bytes.len() as u32);
        self.data[(o + HDR + 4) as usize..(o + HDR + 4) as usize + bytes.len()]
            .copy_from_slice(bytes);
        self.strs.insert(bytes.to_vec(), o);
        o
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

    /// The static cell of a const (spec 5.6): the state, the remembered value and defect, and the
    /// messages that name the const in a self-reference and in a defect raised while it evaluates.
    fn const_cell_for(&mut self, f: &Function) -> u32 {
        let name = check::display_name(&f.name);
        let self_msg = self.string_literal(
            format!("const {name} refers to itself while being evaluated").as_bytes(),
        );
        let prefix = self.string_literal(format!("while evaluating const {name}: ").as_bytes());
        let cell = self.alloc_static(CELL_SIZE);
        self.write_u32(cell + CELL_SELF_MSG, self_msg);
        self.write_u32(cell + CELL_PREFIX, prefix);
        cell
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
        for i in 0..self.globals {
            let ty = if i == G_RESUME && self.uses_suspend {
                GlobalType {
                    val_type: ValType::I64,
                    mutable: true,
                    shared: false,
                }
            } else {
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                }
            };
            let init = match i {
                G_HEAP => ConstExpr::i32_const(heap_base as i32),
                G_STRDESC => ConstExpr::i32_const(self.str_desc as i32),
                G_INTERRUPTED if self.uses_suspend => {
                    ConstExpr::i32_const(self.interrupted_desc as i32)
                }
                G_RESUME if self.uses_suspend => ConstExpr::i64_const(0),
                _ => ConstExpr::i32_const(0),
            };
            globals.global(ty, &init);
        }
        module.section(&globals);

        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export("_start", ExportKind::Func, self.entry_index);
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
            if f.external.is_some() {
                bodies.push(self.external_stub(id, f));
                continue;
            }
            if f.constant && !is_machine(f) {
                // The slots were reserved as [wrapper, init]: the wrapper evaluates and remembers,
                // the init is the body the source declared (spec 5.6). Every const takes this path
                // (`is_machine` is always false for a const), so the wrapper is unconditional, as
                // the C target's.
                let init = self.emit_fn(id, f);
                let wrapper = self.emit_const_wrapper(id, f);
                bodies.push(wrapper);
                bodies.push(init);
                continue;
            }
            bodies.push(self.emit_fn(id, f));
        }
        // The state-machine bodies and resume wrappers come after every IR function (their slots
        // were reserved in the same order in `build`).
        if self.uses_suspend {
            for (id, f) in m.functions.iter() {
                if self.imported(id) || !is_machine(f) {
                    continue;
                }
                let info = self.machine[&ix(id)];
                bodies.push(self.emit_machine_body(id, f, info));
                let ret = self.repr(f.ret);
                bodies.push(emit_machine_resume(f, info, ret));
            }
        }
        self.bodies = bodies;
    }

    fn emit_fn(&mut self, id: FnId, f: &Function) -> WasmFunction {
        if let Some(info) = self.machine.get(&ix(id)).copied() {
            return emit_machine_wrapper(f, info);
        }
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

    /// The wrapper a const's callers call: it evaluates the body (the init function) on the first
    /// reference and remembers the value or the defect (spec 5.6). The init is only reached when
    /// `const_enter` says the const has not been evaluated yet; otherwise the remembered value (or
    /// the pending defect it raised) comes from the cell.
    fn emit_const_wrapper(&mut self, id: FnId, f: &Function) -> WasmFunction {
        let cell = self.const_cell[&ix(id)];
        let init = self.const_init[&ix(id)];
        let ret = self.repr(f.ret);
        let is_ref = i32::from(ret == Repr::Ref);
        let enter = self.rt("const_enter");
        let record = self.rt("const_record");
        let locals = if ret == Repr::Void {
            Vec::new()
        } else {
            vec![(1, val(ret))]
        };
        let mut func = WasmFunction::new(locals);
        {
            let mut ins = func.instructions();
            ins.i32_const(cell as i32);
            ins.i32_const(is_ref);
            ins.call(enter);
            ins.i32_eqz();
            ins.if_(BlockType::Empty);
            // Already evaluated (or a defect is pending): hand out the remembered value, which
            // `const_enter` has already counted if it is a reference.
            if ret != Repr::Void {
                ins.i32_const(cell as i32);
                load_cell_value(&mut ins, ret);
            }
            ins.return_();
            ins.end();
            // First reference: evaluate the body, then remember what it came back with.
            ins.call(init);
            if ret == Repr::Void {
                ins.i32_const(cell as i32);
                ins.i64_const(0);
                ins.i32_const(0);
                ins.call(record);
            } else {
                ins.local_set(0);
                ins.i32_const(cell as i32);
                match ret {
                    Repr::I32 | Repr::Ref => {
                        ins.local_get(0);
                        ins.i64_extend_i32_u();
                    }
                    Repr::I64 => {
                        ins.local_get(0);
                    }
                    Repr::F64 => {
                        ins.local_get(0);
                        ins.i64_reinterpret_f64();
                    }
                    Repr::Void => unreachable!(),
                }
                ins.i32_const(is_ref);
                ins.call(record);
                ins.local_get(0);
            }
            ins.end();
        }
        func
    }

    fn emit_entry(&mut self, entry: &Run) {
        // The entry thunks (one per entry function) come first, matching the definition order
        // reserved in `build`.
        if self.uses_suspend {
            for (i, id) in self.entry_ids.clone().iter().enumerate() {
                let id = *id;
                let thunk = self.emit_thunk(id);
                self.bodies.push(thunk);
                self.entry_thunk = self.thunk_tables[i];
            }
        }
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let ins = &mut func.instructions();
            match entry {
                Run::Main(_) => {
                    if self.uses_suspend {
                        if !self.entry_ids.is_empty() {
                            ins.i32_const(self.thunk_tables[0] as i32);
                            let repr = self.repr(self.m.functions[self.entry_ids[0]].ret);
                            ins.i32_const(repr_code(repr) as i32);
                            ins.call(self.rt("run"));
                        }
                    } else {
                        let main = self.entry_ids.first().copied();
                        if let Some(id) = main {
                            let index = self.fn_index(id);
                            let ret = self.repr(self.m.functions[id].ret);
                            ins.call(index);
                            self.drop_result(ins, ret);
                        }
                    }
                }
                Run::Tests(tests) => {
                    for (i, (name, id)) in tests.iter().enumerate() {
                        if self.uses_suspend {
                            ins.i32_const(self.thunk_tables[i] as i32);
                            let repr = self.repr(self.m.functions[*id].ret);
                            ins.i32_const(repr_code(repr) as i32);
                            ins.call(self.rt("run"));
                        } else {
                            let index = self.fn_index(*id);
                            let ret = self.repr(self.m.functions[*id].ret);
                            ins.call(index);
                            self.drop_result(ins, ret);
                        }
                        let name = self.string_literal(name.as_bytes());
                        ins.i32_const(name as i32);
                        ins.call(self.rt("test_result"));
                    }
                    ins.call(self.rt("test_done"));
                }
            }
            ins.call(self.rt("finish"));
            ins.end();
        }
        self.bodies.push(func);
    }

    /// The root-entry thunk of an entry function: calls it and returns its value as a word (the
    /// runtime `run` takes it through the table).
    fn emit_thunk(&mut self, id: FnId) -> WasmFunction {
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let mut ins = func.instructions();
            let index = self.fn_index(id);
            let ret = self.repr(self.m.functions[id].ret);
            ins.call(index);
            match ret {
                Repr::Void => {
                    ins.i64_const(0);
                }
                Repr::I32 | Repr::Ref => {
                    ins.i64_extend_i32_u();
                }
                Repr::I64 => {}
                Repr::F64 => {
                    ins.i64_reinterpret_f64();
                }
            }
            ins.end();
        }
        func
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
            Term::Defect(_) => self.internal(format!("defect left in {}", cx.f.name)),
        }
    }

    fn stmt(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, id: StmtId) {
        let stmt = cx.f.stmts[id].clone();
        if cx.machine.is_some() {
            self.machine_stmt(cx, ins, id, stmt);
        } else {
            self.plain_stmt(cx, ins, stmt);
        }
    }

    fn plain_stmt(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, stmt: Stmt) {
        match stmt {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                if on_suspend.is_some() {
                    self.internal(format!(
                        "suspension edge outside a state machine in {}",
                        cx.f.name
                    ));
                }
                // Suspending code called where nothing may suspend runs in place (spec 8.7).
                let sync = !cx.f.suspends && emela_lir::may_suspend(&value);
                if sync {
                    let f = self.rt("sync_enter");
                    ins.call(f);
                }
                self.value(cx, ins, dst, &value);
                if sync {
                    let f = self.rt("sync_leave");
                    ins.call(f);
                }
                self.let_landings(cx, ins, &value, on_defect, None);
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

    /// The `on_defect` and `on_fail` landings of a `let`. In a state machine the landing may
    /// itself contain suspension points, so `landing_range` widens the guard to enter it while a
    /// resume is looking for a point inside it.
    fn let_landings(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        value: &Value,
        on_defect: Option<SeqId>,
        landing_range: Option<(u32, u32)>,
    ) {
        if let Some(landing) = on_defect {
            let resuming = landing_range.is_some() && cx.machine.is_some();
            if resuming {
                let (lo, hi) = landing_range.unwrap();
                // `defect != 0 || state in [lo, hi]`
                ins.global_get(G_DEFECT);
                ins.i32_eqz();
                ins.i32_eqz();
                self.machine_in_range(cx, ins, lo, hi);
                ins.i32_or();
            } else {
                ins.global_get(G_DEFECT);
            }
            ins.if_(BlockType::Empty);
            cx.push(Lbl::Other);
            self.seq(cx, ins, landing, None);
            cx.pop();
            ins.end();
        }
        if let Some(on_fail) = value_on_fail(value) {
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
                // A `suspend fn` external bound to an internal host that takes the task (`i32`)
                // last is called through the task wrapper (spec 12.6). A host that returns its
                // value directly is called like any other external.
                let suspend_host = {
                    let f = &self.m.functions[*g];
                    if f.suspends {
                        self.wasm_host(f).and_then(|(m, n)| {
                            let host = external::internal_host_name(&m, &n)?;
                            let (hp, hr) = runtime::signature(host)?;
                            (hp.len() == f.params.len() + 1 && hr.is_empty()).then_some((m, n))
                        })
                    } else {
                        None
                    }
                };
                if let Some((module, name)) = suspend_host {
                    self.suspend_external_call(cx, ins, dst, *g, &module, &name, args);
                    return;
                }
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
                if def.suspend {
                    let ret = self.repr(def.ret);
                    self.suspend_op_call(cx, ins, dst, *handler, *op, &def.name, args, ret);
                    return;
                }
                let ret = self.repr(def.ret);
                self.op_call(cx, ins, dst, *handler, *op, args, ret);
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
    ) {
        let mut params = vec![Repr::Ref];
        params.extend(args.iter().map(|a| Self::operand_repr(cx.f, a)));
        let ty = self.call_ty(&params, ret);
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
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        for a in args {
            self.operand(cx, ins, a);
        }
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        ins.i32_load(mem(HDR, 4));
        ins.call_indirect(0, ty);
        self.assign(cx, ins, dst, ret);
        ins.end();
    }
}

/// A switch's scrutinee.
#[derive(Clone, Copy)]
enum Scrutinee {
    Tag(VarId),
    Lit(VarId),
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

/// Loads a const cell's remembered value (spec 5.6) with the representation's width.
fn load_cell_value(ins: &mut InstructionSink, ret: Repr) {
    match ret {
        Repr::I32 | Repr::Ref => {
            ins.i32_load(mem(CELL_VALUE, 4));
        }
        Repr::I64 => {
            ins.i64_load(mem(CELL_VALUE, 8));
        }
        Repr::F64 => {
            ins.f64_load(mem(CELL_VALUE, 8));
        }
        Repr::Void => {}
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
    /// The `state` local of a state machine: -1 while it runs normally, else the resume point the
    /// function is skipping to. `None` for an ordinary function.
    machine: Option<Machine>,
    /// The `frame` parameter local of a state-machine body.
    frame: u32,
    /// The `state` local of a state-machine body.
    state: u32,
    /// A temporary i32 local.
    take: u32,
}

/// A suspending function's state machine: the resume points and the variables they restore.
pub(crate) struct Machine {
    /// The state range `[lo, hi]` of every statement (and sequence) that contains a suspension
    /// point, so a resume can tell whether to enter or skip it.
    pub(crate) stmt_range: FxHashMap<StmtId, (u32, u32)>,
    pub(crate) seq_range: FxHashMap<SeqId, (u32, u32)>,
    /// The resume points, by state.
    pub(crate) edges: FxHashMap<StmtId, emela_lir::Suspend>,
    /// The states and the variables they restore, sorted by state.
    pub(crate) resumes: Vec<(u32, Vec<VarId>)>,
    /// The table index of the resume wrapper (stored in the frames).
    pub(crate) table: u32,
}

impl<'a> FnCtx<'a> {
    fn new(f: &'a Function, b: &Builder<'_>) -> Self {
        FnCtx::build(f, b, 0, None)
    }

    fn new_machine(f: &'a Function, b: &Builder<'_>, machine: Machine) -> Self {
        FnCtx::build(f, b, 1, Some(machine))
    }

    fn build(
        f: &'a Function,
        b: &Builder<'_>,
        extra_params: u32,
        machine: Option<Machine>,
    ) -> Self {
        let nparams = f.params.len() as u32;
        let mut local = vec![0u32; f.vars.len()];
        let mut decls: Vec<(u32, ValType)> = Vec::new();
        for (i, &p) in f.params.iter().enumerate() {
            local[ix(p)] = i as u32;
        }
        let mut next = nparams + extra_params;
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
        let (frame, state, take) = if machine.is_some() {
            let state = next;
            next += 1;
            let take = next;
            decls.push((1, ValType::I32));
            decls.push((1, ValType::I32));
            (nparams, state, take)
        } else {
            (0, 0, 0)
        };
        let ret = b.repr(f.ret);
        FnCtx {
            f,
            ret,
            local,
            scratch_base,
            decls,
            labels: Vec::new(),
            machine,
            frame,
            state,
            take,
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

/// Whether the module reaches the scheduler at all: a suspending function, a suspending call, a
/// `yield`/`hold`/`unhold`, or a `Value::Fiber` (spec chapter 11, e.g. `Fiber.attempt` reached from
/// a `main` that does not declare `Async`). Programs without any of these keep their exact bytes.
fn uses_suspend(m: &Module) -> bool {
    m.functions.iter().any(|(_, f)| {
        f.suspends
            || f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        on_suspend: Some(_),
                        ..
                    }
                ) || matches!(
                    s,
                    Stmt::Let {
                        value: Value::Call { suspends: true, .. }
                            | Value::Fiber { .. }
                            | Value::Runtime {
                                op: RuntimeOp::Hold
                                    | RuntimeOp::Unhold
                                    | RuntimeOp::Yield
                                    | RuntimeOp::YieldPerform,
                                ..
                            },
                        ..
                    }
                )
            })
    })
}

/// Whether a function is emitted as a state machine: it suspends and has suspension points.
///
/// A const is never a machine: `emela_core::lower` sets `suspends` from the declared function
/// type, and a const's type is a value, not a `fn` (`Lowerer::fn_suspends` returns false), so
/// `f.constant && is_machine(f)` cannot happen. That is why the const wrapper (spec 5.6) is
/// emitted for every const, as the C target does; the assertion records the assumption.
fn is_machine(f: &Function) -> bool {
    debug_assert!(
        !(f.constant && f.suspends),
        "a const cannot suspend (it would get no const wrapper)"
    );
    f.suspends
        && f.body.is_some()
        && f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    on_suspend: Some(_),
                    ..
                }
            )
        })
}

/// Whether the module has a const (spec 5.6), so that the const helpers and the self-reference
/// global are linked. The `!is_machine` guard can never matter (a const's `suspends` is always
/// false, see [`is_machine`]); it is kept so the intent is explicit.
fn has_const(m: &Module) -> bool {
    m.functions
        .iter()
        .any(|(_, f)| f.constant && !is_machine(f))
}

/// Whether the module can record a release-failure note (spec 14.4): it retires a handler (a
/// `with` binding with a `release`), so the release can run and fail while a defect unwinds.
/// Programs without one keep the bytes they had.
fn has_release(m: &Module) -> bool {
    m.functions.iter().any(|(_, f)| {
        f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Runtime {
                        op: RuntimeOp::Retire,
                        ..
                    },
                    ..
                }
            )
        })
    })
}

/// The index of a representation's resume closure in [`Builder::resume_closure`].
pub(crate) fn repr_code(r: Repr) -> usize {
    match r {
        Repr::I32 => 0,
        Repr::I64 => 1,
        Repr::F64 => 2,
        Repr::Ref => 3,
        Repr::Void => 4,
    }
}

/// The function itself: enters the state machine without a frame.
fn emit_machine_wrapper(f: &Function, info: MachineInfo) -> WasmFunction {
    let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
    {
        let mut ins = func.instructions();
        for i in 0..f.params.len() as u32 {
            ins.local_get(i);
        }
        ins.i32_const(0);
        ins.call(info.body);
        ins.end();
    }
    func
}

/// `f_resume(fr) -> i64`: the driver loop's entry into the state machine with a frame. The saved
/// variables are restored from the frame by the body, so the parameters are dead and pass zeros.
fn emit_machine_resume(f: &Function, info: MachineInfo, ret: Repr) -> WasmFunction {
    let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
    {
        let mut ins = func.instructions();
        for &p in &f.params {
            match f.vars[p].repr {
                Repr::I64 => {
                    ins.i64_const(0);
                }
                Repr::F64 => {
                    ins.f64_const(0.0.into());
                }
                Repr::I32 | Repr::Ref => {
                    ins.i32_const(0);
                }
                Repr::Void => {}
            }
        }
        ins.local_get(0);
        ins.call(info.body);
        match ret {
            Repr::Void => {
                ins.i64_const(0);
            }
            Repr::I32 | Repr::Ref => {
                ins.i64_extend_i32_u();
            }
            Repr::I64 => {}
            Repr::F64 => {
                ins.i64_reinterpret_f64();
            }
        }
        ins.end();
    }
    func
}
