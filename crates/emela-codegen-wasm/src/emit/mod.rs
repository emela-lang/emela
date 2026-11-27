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
mod values;

use emela_core::{Builtin, EnumId, FiberOp, OPTION_NONE, OPTION_SOME, Type, TypeId};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, Module, OnFail, Operand, PrimOp, PrimTy, Repr,
    SeqId, Stage, Stmt, StmtId, SwitchOn, Term, Value, VarId, enum_repr, repr_of, repr_of_subst,
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
use crate::{WasmError, WasmErrorKind, WasmOptions, WasmOutput};

/// What the generated `wasi:cli/run@0.3.0` export runs.
#[allow(dead_code)]
pub(crate) enum Run {
    /// The entry's `main` (its monomorphized copy).
    Main(Option<FnId>),
    /// Test functions, with their names.
    Tests(Vec<(String, FnId)>),
    /// The fn of an HTTP service that `@entry(Http.serve)` made (its monomorphized copy), called
    /// by the exported `handle` of `wasi:http/handler@0.3.0` with the request
    /// (`runtime/http.rs`).
    Service(Option<FnId>),
}

impl Run {
    /// The functions the output calls first.
    pub(crate) fn roots(&self) -> Vec<FnId> {
        match self {
            Run::Main(main) | Run::Service(main) => main.iter().copied().collect(),
            Run::Tests(tests) => tests.iter().map(|&(_, id)| id).collect(),
        }
    }

    /// Replaces the roots with their indices after [`prune_functions`](emela_lir::analysis::reach::prune_functions),
    /// in the same order as [`Run::roots`] returned them.
    pub(crate) fn set_roots(&mut self, roots: &[FnId]) {
        let mut roots = roots.iter().copied();
        match self {
            Run::Main(main) | Run::Service(main) => {
                if main.is_some() {
                    *main = roots.next();
                }
            }
            Run::Tests(tests) => {
                for (_, id) in tests {
                    *id = roots.next().expect("a test is a root");
                }
            }
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

pub(crate) fn emit(
    m: &Module,
    entry: &Run,
    options: WasmOptions,
) -> Result<WasmOutput, Vec<WasmError>> {
    assert_eq!(
        m.stage,
        Stage::RefCounted,
        "the WASM emitter reads the Heap IR"
    );
    let mut b = Builder::new(m, options, &entry.roots());
    b.build(entry)?;
    let (core, layout) = b.finish();
    let optional = crate::component::OptionalImports {
        environment: b.uses_env() || b.args_from_host(),
        random: b.uses_random(),
        clock: b.uses_clock(),
        monotonic_clock: b.uses_std_host(StdHost::is_monotonic),
        filesystem: b.uses_std_host(StdHost::is_fs),
        service: b.service,
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
    /// 0 = fields, 1 = string (variable size), 2 = array (variable size, `runtime/array.rs`).
    pub kind: u32,
    pub tag: u32,
    /// The enum index of an error value, or -1.
    pub enum_id: i32,
    pub name: String,
    pub show: Option<u32>,
    pub refs: Vec<bool>,
    /// The element stride in 8-byte slots. Only an array descriptor (kind 2) keeps it, at
    /// [`DESC_STRIDE`]; every other kind writes 0 and stays [`DESC_SIZE`] bytes. Scalar elements
    /// keep a stride of 1, so their arrays are unchanged.
    pub stride: u32,
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

/// The index of a representation in the runtime's thunk tables and the fiber operations' repr
/// argument (the order of `emit::suspend::repr_index`).
fn repr_code(r: Repr) -> i32 {
    match r {
        Repr::Void => 0,
        Repr::I32 => 1,
        Repr::I64 => 2,
        Repr::F64 => 3,
        Repr::Ref => 4,
    }
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
/// The value of `G_STDOUT` once a write found standard output closed by its reader (`DROPPED`);
/// -1 is a stream the program closed itself, at its end.
const STDOUT_LOST: i32 = -2;
/// The name of the handler retired last (spec 14.4: the release note names it).
pub(crate) const G_LAST_RETIRED: u32 = 11;
/// Whether `Std.Env` has fetched the environment list, and the list's pointer and length. The
/// WIT contract makes `get-environment` a value import, so the list is fetched once per command.
const G_ENV_LOADED: u32 = 12;
const G_ENV_PTR: u32 = 13;
const G_ENV_LEN: u32 = 14;
/// Whether `Std.Process` has fetched the argument list, and the list's pointer and length (the
/// first element, the name of the program, included). `get-arguments` is a value import too.
const G_ARGS_LOADED: u32 = 15;
const G_ARGS_PTR: u32 = 16;
const G_ARGS_LEN: u32 = 17;
const GLOBALS: u32 = 18;

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
/// The element stride of an array in 8-byte slots, after [`DESC_SIZE`]. Only an array descriptor
/// has it (E2); a descriptor of any other kind keeps [`DESC_SIZE`] bytes, so no other output moves.
const DESC_STRIDE: u32 = 32;
/// The descriptor kind of an array.
const DESC_KIND_ARRAY: u32 = 2;

pub(crate) struct Builder<'m> {
    pub(crate) m: &'m Module,
    /// The representation options (see [`WasmOptions`]).
    options: WasmOptions,
    /// The records and tuples held by value in locals (E1). Empty unless `options.values`.
    values: values::Values<'m>,
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
    /// The IR functions whose body the entry can run, from
    /// `emela_lir::analysis::reach::Reachability` on the Heap IR. The runtime gates read it so that a
    /// feature only reachable code uses is emitted.
    pub(crate) reachable: FxHashSet<FnId>,
    /// The runtime functions the runtime body being emitted calls. `runtime::emit` reads it to
    /// trim the runtimes whose entry points are independent (see `runtime::trimmable`).
    pub(crate) rt_calls: std::cell::RefCell<FxHashSet<&'static str>>,
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
    /// The trapping bodies of the hosts the entry cannot reach, emitted after the IR functions.
    traps: Vec<WasmFunction>,
    /// The external functions resolved to a supported WASI 0.3 host, with their runtime wrapper.
    std_hosts: Vec<(FnId, StdHost)>,
    /// The suspend runtime's cells and per-function entries, when the program suspends.
    pub(crate) sus: Option<suspend::Plan>,
    /// The file system runtime's cells, when the program uses `Std.FileSystem`.
    pub(crate) fs_plan: Option<runtime::fs::Plan>,
    pub(crate) http_plan: Option<runtime::http::Plan>,
    /// Whether the output is an HTTP service ([`Run::Service`]): it exports `handle` of
    /// `wasi:http/handler@0.3.0` instead of `run` of `wasi:cli/run@0.3.0`.
    pub(crate) service: bool,
    /// Whether the output runs the tests ([`Run::Tests`]).
    pub(crate) tests: bool,
    /// The fiber scheduler's cells and descriptors, when the program suspends.
    pub(crate) fiber_plan: Option<runtime::fiber::Plan>,
}

impl<'m> Builder<'m> {
    fn new(m: &'m Module, options: WasmOptions, roots: &[FnId]) -> Self {
        let mut b = Builder {
            m,
            options,
            values: if options.values {
                values::Values::new(m, roots)
            } else {
                values::Values::none(m)
            },
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
            reachable: FxHashSet::default(),
            rt_calls: std::cell::RefCell::new(FxHashSet::default()),
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
            traps: Vec::new(),
            std_hosts: Vec::new(),
            sus: None,
            fs_plan: None,
            http_plan: None,
            service: false,
            tests: false,
            fiber_plan: None,
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

    /// The layout of `ty` if the local-only slice holds it by value, or `None` if the boxed
    /// representation stays.
    pub(crate) fn layout(&self, ty: TypeId) -> Option<usize> {
        self.values.index(ty)
    }

    /// The scalar leaves of `ty` if it is held by value.
    pub(crate) fn leaves(&self, ty: TypeId) -> Option<&[Repr]> {
        Some(&self.values.get(self.layout(ty)?).leaves)
    }

    /// Whether any type of the module is held by value. A function whose module has none needs no
    /// scratch local for a box (E1 step 5).
    fn has_values(&self) -> bool {
        self.values.any()
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

    /// Whether reachable code has a `Builtin` that `op` accepts. The runtime gates read it so a
    /// feature that only unreachable code uses is not emitted.
    pub(crate) fn reaches_builtin(&self, mut used: impl FnMut(Builtin) -> bool) -> bool {
        self.m.functions.iter().any(|(id, f)| {
            self.reachable.contains(&id)
                && f.stmts.iter().any(|(_, stmt)| {
                    matches!(
                        stmt,
                        Stmt::Let { value: Value::Builtin { op, .. }, .. } if used(*op)
                    )
                })
        })
    }

    pub(crate) fn uses_console(&self) -> bool {
        self.uses_std_host(StdHost::is_console)
    }

    pub(crate) fn uses_env(&self) -> bool {
        self.uses_std_host(StdHost::is_env)
    }

    /// Whether the arguments come from `get-arguments`. The tests and an HTTP service have none
    /// (`[api.std.process]`), so their output does not import it.
    pub(crate) fn args_from_host(&self) -> bool {
        self.uses_std_host(StdHost::is_args) && !self.tests && !self.service
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

    /// Whether reachable code uses an Array builtin. A program without arrays keeps the same bytes:
    /// the functions of `runtime::array` are left out, and so is the branch of `drop` for arrays.
    pub(crate) fn uses_arrays(&self) -> bool {
        self.reaches_builtin(|op| {
            matches!(
                op,
                Builtin::ArrayFromList
                    | Builtin::ArrayToList
                    | Builtin::ArraySize
                    | Builtin::ArrayGet
                    | Builtin::ArrayGetOr
                    | Builtin::ArrayLoadUnchecked
                    | Builtin::ArraySet
                    | Builtin::ArraySlice
                    | Builtin::ArrayConcat
                    | Builtin::ArrayMapRange
            )
        })
    }

    /// Whether the program uses `Exit.attempt` or an operation of `Std.Cause`, the only concurrency
    /// operations the target supports (`check::value`).
    pub(crate) fn uses_attempt(&self) -> bool {
        self.m.functions.iter().any(|(_, f)| {
            f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        value: Value::Fiber { .. },
                        ..
                    }
                )
            })
        })
    }

    /// Whether some `Exit.attempt` runs a function that may suspend, so the scheduler needs the
    /// resume function of its frame.
    pub(crate) fn attempt_suspends(&self) -> bool {
        self.m.functions.iter().any(|(_, f)| {
            f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        value: Value::Fiber {
                            op: FiberOp::Attempt { suspends: true, .. },
                            ..
                        },
                        ..
                    }
                )
            })
        })
    }

    /// Whether the program forks, scopes or shares a Deferred: the operations that need the
    /// scheduler. `Exit.attempt` and the operations of `Std.Cause` run on the current fiber and need
    /// none of this.
    pub(crate) fn uses_fibers(&self) -> bool {
        self.m.functions.iter().any(|(_, f)| {
            f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        value: Value::Fiber { op, .. },
                        ..
                    } if !matches!(
                        op,
                        FiberOp::Attempt { .. }
                            | FiberOp::CauseKind { .. }
                            | FiberOp::CauseToString
                            | FiberOp::CauseRaise { .. }
                    )
                )
            })
        })
    }

    /// The cell that says the run already ended, when the program has the fiber runtime.
    pub(crate) fn run_over_cell(&self) -> Option<u32> {
        self.fiber_plan.as_ref().map(|p| p.run_over)
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

    pub(crate) fn arguments_get(&mut self) -> u32 {
        self.host_import(
            "wasi:cli/environment@0.3.0",
            "get-arguments",
            // The result is a `list<string>`, returned through a pointer.
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

    /// The wasm parameters and results of `f`. A parameter held by value is its leaves, and a
    /// result held by value is one wasm result per leaf (multi-value). The layout predicate keeps
    /// a function whose signature another calling convention sees (the entry, an external host, a
    /// fiber thunk, the table, an exporting thunk) boxed, so the flattened form is used by every
    /// call of a function type that agrees on it: a direct `call`, `call_indirect`, a lifted
    /// closure, an operation implementation, and a suspending function's wrapper, body and
    /// trampoline.
    fn signature(&self, f: &Function) -> (Vec<ValType>, Vec<ValType>) {
        let mut params = Vec::new();
        for &p in &f.params {
            match self.leaves(f.vars[p].ty) {
                Some(leaves) => params.extend(leaves.iter().map(|&r| val(r))),
                None => params.push(val(f.vars[p].repr)),
            }
        }
        let results = match self.leaves(f.ret) {
            Some(leaves) => leaves.iter().map(|&r| val(r)).collect(),
            None => repr_val(self.repr(f.ret)).into_iter().collect(),
        };
        (params, results)
    }

    fn fn_ty(&mut self, f: &Function) -> u32 {
        let (params, results) = self.signature(f);
        self.ty(params, results)
    }

    /// Reserves a trapping stub per wasm signature for the externals the entry cannot reach, and
    /// points their indices at it.
    ///
    /// A handler constructor builds a closure for every operation of its effect, whether the
    /// entry performs the operation or not, so the emitted bodies contain calls to those hosts.
    /// Emitting the import would carry a host the program never uses, and the body of the call
    /// cannot run because only performing the operation reaches it. The stub keeps the calls
    /// valid without the import (spec `[target.wasm.output]`).
    fn reserve_traps(&mut self, ids: &[FnId]) {
        let mut traps: FxHashMap<u32, u32> = FxHashMap::default();
        for &id in ids {
            let (params, results) = {
                let f = &self.m.functions[id];
                (
                    f.params
                        .iter()
                        .map(|&p| val(f.vars[p].repr))
                        .collect::<Vec<_>>(),
                    repr_val(self.repr(f.ret)).into_iter().collect::<Vec<_>>(),
                )
            };
            let ty = self.ty(params, results);
            let index = *traps.entry(ty).or_insert_with(|| {
                let index = self.imports.len() as u32 + self.defs.len() as u32;
                self.defs.push(ty);
                let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
                {
                    let ins = &mut func.instructions();
                    ins.unreachable();
                    ins.end();
                }
                self.traps.push(func);
                index
            });
            self.fn_index[ix(id)] = index;
        }
    }

    fn imported(&self, id: FnId) -> bool {
        let f = &self.m.functions[id];
        f.external
            .as_ref()
            .is_some_and(|e| e.binding("wasm").is_some())
    }

    /// Whether the entry reaches the body of `f`. Every function keeps its index (a call, a
    /// closure record or the table names it); only the body of an unreachable one is left out.
    /// The roots that are not calls stay: a const is evaluated from its own cell, an error's
    /// `show` function is named by its descriptor, and the trapping stubs of the hosts are
    /// reserved separately (`reserve_traps`). A static closure and a table entry are reached
    /// through the `MakeClosure`/`FnRef` that names them, which the walk follows.
    fn keeps_body(&self, id: FnId, f: &Function) -> bool {
        self.reachable.contains(&id)
            || f.constant
            || self.m.error_shows.iter().any(|&(_, show)| show == id)
    }

    /// Emits the whole module into the builder, or returns every error met on the way.
    fn build(&mut self, entry: &Run) -> Result<(), Vec<WasmError>> {
        // The runtime gates read which functions the entry reaches. The analysis is the LIR's
        // (emela-lir), walked here on the Heap IR: a closure body is its own function at this
        // stage and a handler operation's body is deferred until it is performed. The same set
        // decides which host imports are emitted, so a body pruned here never needs an import the
        // analysis did not count.
        let reach = emela_lir::analysis::reach::Reachability::walk(self.m, &entry.roots());
        self.reachable = self
            .m
            .functions
            .iter()
            .filter(|(id, _)| reach.is_called(*id))
            .map(|(id, _)| id)
            .collect();
        self.service = matches!(entry, Run::Service(_));
        self.tests = matches!(entry, Run::Tests(_));
        // The imports (the WASI host and the externals with a wasm binding) take the lowest
        // indices. A supported `Std` host is a runtime wrapper instead; it is resolved after the
        // runtime functions have their indices.
        let mut unreachable_hosts = Vec::new();
        for (id, f) in self.m.functions.iter() {
            if let Some(ext) = &f.external
                && let Some(binding) = ext.binding("wasm")
            {
                match crate::std_host::classify(f) {
                    Some(host) if reach.is_called(id) => self.std_hosts.push((id, host)),
                    // A host the entry cannot reach is in the module because a handler it builds
                    // bundles the external (`Process` of `Std.Process` bundles several system handlers). It is
                    // never called, so neither its runtime wrapper nor its import is emitted
                    // (spec `[target.wasm.output]`). Its trapping stub is reserved for a call in
                    // an emitted body; an operation body that is only referenced, not called, is
                    // a stub too, so it cannot call the host.
                    Some(_) => unreachable_hosts.push(id),
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
        if self.args_from_host() {
            self.arguments_get();
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
        if self.service {
            runtime::http::imports(self);
        } else {
            self.task_return();
        }
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
        // The error `show` functions, as table indices (their descriptors point at them). A Show
        // impl item always has a body (the `impl_item` production in [grammar.decl]), so its
        // function is never imported and the loop above gives it an index in the table.
        for (enum_id, function) in self.m.error_shows.iter().copied() {
            debug_assert!(
                !self.imported(function),
                "an error's show function cannot be imported: a Show impl item needs a body"
            );
            self.error_shows
                .insert(enum_id, self.fn_table[ix(function)]);
        }
        // The stubs of the hosts the entry cannot reach. They are definitions like the IR
        // functions (one per distinct signature), so the calls in the emitted handler bodies
        // validate; the calls can never run.
        self.reserve_traps(&unreachable_hosts);
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
            runtime::fiber::prepare(self);
        }
        // The async-lifted `wasi:cli/run@0.3.0#run` export (or `handle` of
        // `wasi:http/handler@0.3.0`, which takes the request) and its callback. Both return the
        // component-model status code.
        let params = if self.service {
            vec![ValType::I32]
        } else {
            Vec::new()
        };
        let ty = self.ty(params, vec![ValType::I32]);
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
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(std::mem::take(&mut self.errors))
        }
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
            stride: 0,
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
            stride: 0,
        })
    }

    pub(crate) fn desc(&mut self, key: DescKey) -> u32 {
        // `from_core` gives every constructed error enum a show function (its Show, or the
        // derived name when the type does not satisfy Show), and `build` maps it to a table
        // index, so a language-produced error's descriptor always has one. `show_failure`
        // relies on that.
        debug_assert!(
            key.enum_id < 0 || key.show.is_some(),
            "the error `{}` has no show function; `from_core` must give every constructed error \
             enum one (`Derive::ErrorName` when its type does not satisfy Show)",
            key.name
        );
        if let Some(&o) = self.descs.get(&key) {
            return o;
        }
        let o = self.alloc_static(if key.kind == DESC_KIND_ARRAY {
            DESC_SIZE + 4
        } else {
            DESC_SIZE
        });
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
        if key.kind == DESC_KIND_ARRAY {
            self.write_u32(o + DESC_STRIDE, key.stride);
        }
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
        let (some, none) = self.option_consts(repr == Repr::Ref);
        Some((some, none, repr))
    }

    /// The descriptor of Prelude's `Some` whose field is a reference (`field_ref`) or another
    /// word, and the static `None` object (the same for every field).
    pub(crate) fn option_consts(&mut self, field_ref: bool) -> (u32, u32) {
        let some = self.desc(DescKey {
            kind: 0,
            tag: OPTION_SOME as u32,
            enum_id: -1,
            name: "Some".to_owned(),
            show: None,
            refs: vec![field_ref],
            stride: 0,
        });
        let none_desc = self.desc(DescKey {
            kind: 0,
            tag: OPTION_NONE as u32,
            enum_id: -1,
            name: "None".to_owned(),
            show: None,
            refs: Vec::new(),
            stride: 0,
        });
        let none = self.nullary(none_desc);
        (some, none)
    }

    /// The representation of the elements of the array type `ty` (`[api.std.array]`).
    pub(crate) fn array_elem_repr(&self, ty: TypeId) -> Repr {
        match self.core().ty(ty) {
            Type::Array(elem) => self.repr(*elem),
            _ => Repr::Ref,
        }
    }

    /// The layout of the element of the array type `ty` when it is an all-scalar record or tuple
    /// stored inline (E2), or `None` when the elements stay one-word values or references.
    ///
    /// This ignores the local-only exclusions of [`values::Values`]: an array element can be
    /// stored inline even when the same type is a function parameter, a result or a list head.
    pub(crate) fn inline_array_elem(&self, ty: TypeId) -> Option<values::Layout> {
        if !self.options.aggregate_arrays {
            return None;
        }
        match self.core().ty(ty) {
            Type::Array(elem) => values::all_scalar(self.core(), *elem),
            _ => None,
        }
    }

    /// The descriptor of the box made when an inline array element is read as a reference (E2).
    /// Its slots are the element's scalar leaves and it has no references, exactly what
    /// `Value::Make` gives a boxed all-scalar record or tuple.
    pub(crate) fn inline_array_elem_desc(&mut self, ty: TypeId) -> Option<u32> {
        let layout = self.inline_array_elem(ty)?;
        let slots = layout.slots();
        Some(self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name: layout.name.clone(),
            show: None,
            refs: vec![false; slots],
            stride: 0,
        }))
    }

    /// The descriptor of an array of type `ty` (`runtime/array.rs`): kind 2, with `refs[0]`
    /// telling whether the elements are references and the element stride in slots.
    ///
    /// A scalar array keeps a stride of 1 (one 8-byte word). An array of an all-scalar record or
    /// tuple stores the element inline, one slot per scalar leaf, so `refs[0]` is false whatever
    /// the IR representation of the element is.
    pub(crate) fn array_desc(&mut self, ty: TypeId) -> u32 {
        let inline = self.inline_array_elem(ty);
        let stride = inline.as_ref().map_or(1, |layout| layout.slots() as u32);
        let elem_ref = inline.is_none() && self.array_elem_repr(ty) == Repr::Ref;
        self.desc(DescKey {
            kind: DESC_KIND_ARRAY,
            tag: 0,
            enum_id: -1,
            name: "Array".to_owned(),
            show: None,
            refs: vec![elem_ref],
            stride,
        })
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
            stride: 0,
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

    /// The wasm value types the operands push, in order: one per scalar leaf of a value held by
    /// value, one for any other value, and none for `()`.
    ///
    /// The operands are the authority, not the declared function type: a lifted closure can take
    /// the evidence table as an ordinary last parameter (spec 8.4), and a `()` parameter has no
    /// representation.
    fn operand_val_types(&self, cx: &FnCtx, args: &[Operand]) -> Vec<ValType> {
        let mut out = Vec::new();
        for arg in args {
            match arg {
                Operand::Var(v) => match cx.aggregate(*v) {
                    Some(n) => {
                        for i in 0..n {
                            out.push(val(cx.leaf_repr(*v, i)));
                        }
                    }
                    None => out.push(val(cx.f.vars[*v].repr)),
                },
                Operand::Const(c) => {
                    if c.repr() != Repr::Void {
                        out.push(val(c.repr()));
                    }
                }
            }
        }
        out
    }

    /// The wasm result types of a call returning `ty`, and the leaves to store when the result is
    /// held by value.
    fn result_val_types(&self, ty: TypeId) -> (Vec<ValType>, Option<Vec<Repr>>) {
        match self.leaves(ty) {
            Some(leaves) => (
                leaves.iter().map(|&r| val(r)).collect(),
                Some(leaves.to_vec()),
            ),
            None => (repr_val(self.repr(ty)).into_iter().collect(), None),
        }
    }

    /// Stores a call's result: one wasm local per leaf of a value held by value, or the single
    /// representation of any other result.
    fn assign_result(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        ret: Repr,
        leaves: Option<Vec<Repr>>,
    ) {
        let Some(leaves) = leaves else {
            self.assign(cx, ins, dst, ret);
            return;
        };
        match dst {
            Some(d) => {
                if cx.aggregate(d) != Some(leaves.len() as u32) {
                    self.internal(format!(
                        "a result of {} has no destination in {}",
                        leaves.len(),
                        cx.f.name
                    ));
                    return;
                }
                // The leaves are on the stack in field order; store them in reverse.
                for i in (0..leaves.len() as u32).rev() {
                    ins.local_set(cx.leaf(d, i));
                }
            }
            // An unused result is given up like any other value: the leaves own no reference, so
            // each is dropped from the operand stack.
            None => {
                for _ in &leaves {
                    ins.drop();
                }
            }
        }
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
        let entry = if self.service {
            "wasi:http/handler@0.3.0#handle"
        } else {
            "wasi:cli/run@0.3.0#run"
        };
        exports.export(
            &format!("[async-lift]{entry}"),
            ExportKind::Func,
            self.entry_index,
        );
        exports.export(
            &format!("[callback][async-lift]{entry}"),
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
            if !self.keeps_body(id, f) {
                // The index space keeps every function (calls and table entries name it), but a
                // body the entry cannot reach costs three bytes instead of its code. A const is
                // always kept, so this one is not a const and has a single body.
                bodies.push(runtime::stub());
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
        bodies.append(&mut self.traps);
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
        // The wrapper has the function's ordinary (flattened) signature and forwards every wasm
        // parameter to the body, which takes the same parameters then the frame pointer. A by-value
        // parameter is several wasm parameters, in field order.
        let params = self.signature(f).0;
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let mut ins = func.instructions();
            for i in 0..params.len() as u32 {
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
            let leaves = cx.ret_leaves.clone();
            dummy_return_values(&mut ins, &leaves);
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
                    if self.sus.is_some() {
                        let begin = self.rt("fb_root_begin");
                        ins.call(begin);
                    }
                    ins.call(index);
                    self.drop_result(ins, ret);
                    if self.sus.is_some() {
                        let root = self.rt("fb_root_run");
                        ins.call(root);
                    }
                }
                Run::Main(None) => {}
                Run::Service(service) => {
                    // One request: the launch, then the response (`runtime/http.rs`). A
                    // launch that ended interrupted leaves no response; its unwinding is not
                    // carried to the next request.
                    if let Some(id) = service {
                        let index = self.fn_index(*id);
                        let ret = self.repr(self.m.functions[*id].ret);
                        // `Incoming` is a counted reference: the handle goes in a box.
                        ins.local_get(0);
                        ins.call(self.rt("http_box"));
                        if self.sus.is_some() {
                            let begin = self.rt("fb_root_begin");
                            ins.call(begin);
                        }
                        ins.call(index);
                        self.drop_result(ins, ret);
                        if self.sus.is_some() {
                            let root = self.rt("fb_root_run");
                            ins.call(root);
                        }
                    }
                    // The run's result is a counted value (`fb_finish_run` put it back into the
                    // global). Release it before the next request starts.
                    ins.global_get(G_DEFECT);
                    ins.call(self.rt("drop"));
                    ins.global_get(G_FAILURE);
                    ins.call(self.rt("drop"));
                    ins.i32_const(0);
                    ins.global_set(G_DEFECT);
                    ins.i32_const(0);
                    ins.global_set(G_FAILURE);
                    ins.call(self.rt("http_respond"));
                }
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
                        if self.sus.is_some() {
                            let begin = self.rt("fb_root_begin");
                            ins.call(begin);
                        }
                        ins.call(index);
                        self.drop_result(ins, ret);
                        if self.sus.is_some() {
                            let root = self.rt("fb_root_run");
                            ins.call(root);
                        }
                        let name = self.string_literal(name.as_bytes());
                        ins.i32_const(name as i32);
                        ins.call(self.rt("test_result"));
                    }
                    ins.call(self.rt("test_done"));
                }
            }
            // A service keeps its output open for the next request, and `http_respond` did the
            // `task.return`.
            if !self.service {
                ins.call(self.rt("finish"));
                ins.call(self.rt("close_output"));
                ins.i32_const(0);
                ins.call(self.task_return());
            }
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
                if let Some(n) = cx.aggregate(d) {
                    // The value's leaves are on the stack in field order; store them in reverse.
                    for i in (0..n).rev() {
                        ins.local_set(cx.leaf(d, i));
                    }
                } else if repr != Repr::Void {
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
                if let Some(n) = cx.aggregate(*v) {
                    for i in 0..n {
                        ins.local_get(cx.leaf(*v, i));
                    }
                } else {
                    ins.local_get(cx.local(*v));
                }
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

    /// The number of wasm values [`Builder::operand`] pushes: one per scalar leaf of a value held
    /// by value, one for any other value, and none for a `()` constant.
    fn operand_width(cx: &FnCtx, o: &Operand) -> usize {
        match o {
            Operand::Var(v) => cx.aggregate(*v).map_or(1, |n| n as usize),
            Operand::Const(c) => usize::from(c.repr() != Repr::Void),
        }
    }

    /// The descriptor of a box of `ty`, a value held by value: one slot per scalar leaf, no
    /// references, tag 0, the value name. It is the same object the C backend's `layout_desc` and
    /// `em_box_value` make, so a box can be unboxed by loading its slots.
    fn box_desc(&mut self, ty: TypeId) -> Option<u32> {
        let i = self.layout(ty)?;
        let (name, slots) = {
            let layout = self.values.get(i);
            (layout.name.clone(), layout.slots())
        };
        Some(self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name,
            show: None,
            refs: vec![false; slots],
            stride: 0,
        }))
    }

    /// Stores the top of the stack as `repr` at `off` of the object below it.
    fn store_mem(ins: &mut InstructionSink, off: u32, repr: Repr) {
        match repr {
            Repr::I32 | Repr::Ref => {
                ins.i32_store(mem(off, 4));
            }
            Repr::I64 => {
                ins.i64_store(mem(off, 8));
            }
            Repr::F64 => {
                ins.f64_store(mem(off, 8));
            }
            Repr::Void => {}
        }
    }

    /// Loads `repr` at `off` of the object on top of the stack.
    fn load_mem(ins: &mut InstructionSink, off: u32, repr: Repr) {
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

    /// Allocates a box of the value in local `v` (held by value, E1 step 5), stores its leaves,
    /// and leaves the box reference on the stack. The local copy owns nothing, so it is not
    /// consumed.
    fn box_value(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, v: VarId) {
        let ty = cx.f.vars[v].ty;
        let Some(i) = self.layout(ty) else {
            let name = cx.f.name.clone();
            self.internal(format!("a value with no layout is boxed in {name}"));
            ins.i32_const(0);
            return;
        };
        let leaves = self.values.get(i).leaves.clone();
        let Some(desc) = self.box_desc(ty) else {
            ins.i32_const(0);
            return;
        };
        let Some(box_ref) = cx.box_ref else {
            let name = cx.f.name.clone();
            self.internal(format!("no box local in {name}"));
            ins.i32_const(0);
            return;
        };
        ins.i32_const(leaves.len() as i32);
        ins.i32_const(desc as i32);
        let alloc = self.rt("alloc");
        ins.call(alloc);
        ins.local_set(box_ref);
        for (k, &r) in leaves.iter().enumerate() {
            ins.local_get(box_ref);
            ins.local_get(cx.leaf(v, k as u32));
            Self::store_mem(ins, HDR + k as u32 * 8, r);
        }
        ins.local_get(box_ref);
    }

    /// Copies the leaves of the box on top of the stack into the locals of the value in `dst`.
    /// The box reference is borrowed; it is left in the box local and not dropped here.
    fn unbox_top(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, dst: VarId) {
        let Some(box_ref) = cx.box_ref else {
            let name = cx.f.name.clone();
            self.internal(format!("no box local in {name}"));
            return;
        };
        ins.local_set(box_ref);
        self.unbox_from(cx, ins, dst, box_ref);
    }

    /// Copies the leaves of the box in local `box_ref` into the locals of the value in `dst`.
    fn unbox_from(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, dst: VarId, box_ref: u32) {
        let ty = cx.f.vars[dst].ty;
        let Some(i) = self.layout(ty) else {
            let name = cx.f.name.clone();
            self.internal(format!("a value with no layout is unboxed in {name}"));
            return;
        };
        let leaves = self.values.get(i).leaves.clone();
        if cx.aggregate(dst) != Some(leaves.len() as u32) {
            let name = cx.f.name.clone();
            self.internal(format!(
                "a box of {} values has no destination in {name}",
                leaves.len()
            ));
            return;
        }
        for (k, &r) in leaves.iter().enumerate() {
            ins.local_get(box_ref);
            Self::load_mem(ins, HDR + k as u32 * 8, r);
            ins.local_set(cx.leaf(dst, k as u32));
        }
    }

    /// Stores the operand into slot `index` of the object in local `base` (a word: any word for a
    /// value without a representation). An operand held by value is boxed first (E1 step 5).
    fn store_slot(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        base: VarId,
        index: usize,
        o: &Operand,
    ) {
        // A value held in locals has no single reference to store in a box: write a box of its
        // leaves and store that reference. The box owns no reference itself, so the containing
        // object's descriptor marks the slot a reference and releases the box with the object.
        if let Operand::Var(v) = o
            && cx.aggregate(*v).is_some()
        {
            let off = HDR + index as u32 * 8;
            ins.local_get(cx.local(base));
            self.box_value(cx, ins, *v);
            ins.i32_store(mem(off, 4));
            return;
        }
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
                    let repr = Self::operand_repr(cx.f, v);
                    self.assign(cx, ins, Some(d), repr);
                } else if let (None, Some(_)) = (yield_dst, v) {
                    self.internal(format!("yield without a target in {}", cx.f.name));
                }
            }
            Term::Return(v) => {
                // A flattened function's result is one wasm result per leaf, so the operand must
                // push exactly as many values as the signature declares.
                let width = v.as_ref().map_or(0, |o| Self::operand_width(cx, o));
                if width != cx.ret_leaves.len() {
                    self.internal(format!(
                        "return of {width} values from {} with {} results",
                        cx.f.name,
                        cx.ret_leaves.len()
                    ));
                    return;
                }
                if let Some(v) = v {
                    self.operand(cx, ins, v);
                }
                ins.return_();
            }
            Term::Fail(v) => {
                self.operand(cx, ins, v);
                ins.global_set(G_FAILURE);
                dummy_return_values(ins, &cx.ret_leaves);
            }
            Term::Break { label, value } => {
                let Some((depth, dst)) = cx.find_block(*label) else {
                    self.internal(format!("break to an unknown label in {}", cx.f.name));
                    return;
                };
                if let (Some(d), Some(v)) = (dst, value) {
                    self.operand(cx, ins, v);
                    let repr = Self::operand_repr(cx.f, v);
                    self.assign(cx, ins, Some(d), repr);
                }
                ins.br(depth);
            }
            Term::Recur(args) => {
                let Some((depth, vars)) = cx.find_loop() else {
                    self.internal(format!("recur outside a loop in {}", cx.f.name));
                    return;
                };
                // Parallel assignment through scratch locals. A value held by value contributes
                // one scratch slot per leaf, in field order; `recur_slots` made room for the
                // widest `recur`.
                let mut slot = 0u32;
                for (o, &v) in args.iter().zip(&vars) {
                    self.operand(cx, ins, o);
                    if let Some(n) = cx.aggregate(v) {
                        for i in (0..n).rev() {
                            let r = cx.leaf_repr(v, i);
                            ins.local_set(cx.scratch(r, (slot + i) as usize));
                        }
                        slot += n;
                    } else {
                        let r = cx.f.vars[v].repr;
                        ins.local_set(cx.scratch(r, slot as usize));
                        slot += 1;
                    }
                }
                let mut slot = 0u32;
                for &v in &vars {
                    if let Some(n) = cx.aggregate(v) {
                        for i in 0..n {
                            let r = cx.leaf_repr(v, i);
                            ins.local_get(cx.scratch(r, (slot + i) as usize));
                            ins.local_set(cx.leaf(v, i));
                        }
                        slot += n;
                    } else {
                        let r = cx.f.vars[v].repr;
                        ins.local_get(cx.scratch(r, slot as usize));
                        ins.local_set(cx.local(v));
                        slot += 1;
                    }
                }
                ins.br(depth);
            }
            Term::Unwind => dummy_return_values(ins, &cx.ret_leaves),
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
                match cx.f.reuse.get(&id).copied() {
                    // Perceus reuse (C4/PR-042): write the new fields into the old block instead
                    // of allocating a new one.
                    Some(old) => self.reuse_make(cx, ins, dst, old, &value),
                    None => self.value(cx, ins, dst, &value),
                }
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
                        if cx.aggregate(v).is_some() {
                            // A value held by value has exactly one constructor, so its tag is 0
                            // (as the C backend does).
                            Scrutinee::TagZero
                        } else if cx.f.vars[v].repr == Repr::Ref {
                            Scrutinee::Tag(v)
                        } else {
                            Scrutinee::Variant(v)
                        }
                    }
                    SwitchOn::Lit(v) => Scrutinee::Lit(v),
                };
                self.switch(cx, ins, &cases, &default, dst, scrutinee);
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                for (&v, init) in vars.iter().zip(&inits) {
                    self.operand(cx, ins, init);
                    let repr = Self::operand_repr(cx.f, init);
                    self.assign(cx, ins, Some(v), repr);
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
                // A value held by value owns no reference: nothing to count (the C backend's
                // `count` does the same).
                if cx.aggregate(v).is_some() {
                    return;
                }
                ins.local_get(cx.local(v));
                // `dup`: count the object unless it is null or static (rc == 0).
                let obj = cx.scratch(Repr::I32, 0);
                let rc = cx.scratch(Repr::Ref, 0);
                ins.local_set(obj);
                ins.local_get(obj);
                ins.i32_eqz();
                ins.if_(BlockType::Empty);
                ins.else_();
                ins.local_get(obj);
                ins.i32_load(mem(0, 4));
                ins.local_set(rc);
                ins.local_get(rc);
                ins.i32_eqz();
                ins.if_(BlockType::Empty);
                ins.else_();
                ins.local_get(obj);
                ins.local_get(rc);
                ins.i32_const(1);
                ins.i32_add();
                ins.i32_store(mem(0, 4));
                ins.end();
                ins.end();
            }
            Stmt::Drop(v) => {
                // A value held by value owns no reference: nothing to release.
                if cx.aggregate(v).is_some() {
                    return;
                }
                ins.local_get(cx.local(v));
                // Drop specialisation (C3/PR-027): a value whose payload holds no reference and
                // whose type has no finalizer gives its block back without walking it.
                let name = match cx.f.drop_kinds.get(&id) {
                    Some(emela_lir::opt::drop_spec::DropKind::Block) => "drop_block",
                    _ => "drop",
                };
                let drop = self.rt(name);
                ins.call(drop);
            }
        }
    }

    /// A switch on an enum tag whose variant keys are dense enough is dispatched with `br_table`;
    /// everything else (a sparse table, a switch on a literal, too few cases) stays the chain of
    /// comparing `if`s. A literal switch cannot use a table: its keys are arbitrary `i32`s, and
    /// an `i64` one is not the `i32` a `br_table` indexes with.
    ///
    /// The keys and their span come from the LIR ([`emela_lir::opt::switch::variant_keys`],
    /// [`emela_lir::opt::switch::variant_span`]); when the normalisation pass ran they are already
    /// sorted, the keys of a shared body are joined and duplicates are gone, and the emitter only
    /// decides whether a table is worth its bytes. The helpers also accept the un-normalised form
    /// of a `--no-opt` build.
    fn switch(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        cases: &[emela_lir::Case],
        default: &Option<SeqId>,
        dst: Option<VarId>,
        scrutinee: Scrutinee,
    ) {
        if matches!(scrutinee, Scrutinee::Lit(_)) {
            return self.switch_chain(cx, ins, cases, 0, default, dst, scrutinee);
        }
        // The verifier gives a tag switch only variant keys, in range of the enum, each once.
        let Some(keys) = emela_lir::opt::switch::variant_keys(cases) else {
            self.internal(format!("a literal case in a tag switch in {}", cx.f.name));
            return;
        };
        let Some(width) = emela_lir::opt::switch::variant_span(cases) else {
            return self.switch_chain(cx, ins, cases, 0, default, dst, scrutinee);
        };
        // A table entry costs a byte; a case of the chain costs a comparison and its `if`, which
        // is more. A table wider than that is not worth it, nor is one for a couple of cases,
        // where the dispatch is one more label than the comparison it replaces. The count is of
        // keys, not of bodies: a chain compares every key, a table has one block per body.
        if keys.len() < SWITCH_TABLE_MIN_CASES || width > SWITCH_TABLE_WIDTH_SCALE * keys.len() {
            return self.switch_chain(cx, ins, cases, 0, default, dst, scrutinee);
        }
        self.switch_table(cx, ins, cases, default, dst, scrutinee, width);
    }

    /// A switch as one nested block per case, dispatched by `br_table`, with a block around them
    /// for the end of the switch and a block inside it for the default.
    ///
    /// The blocks are pushed in case order, so the innermost (last pushed) block belongs to the
    /// first case: a `br_table` depth of `i` leaves the blocks of the cases before `i` and lands
    /// after the `end` of case `i`'s block, which is where case `i`'s body starts. A gap between
    /// variant numbers lands on the default block. A case body that falls through (`yield`) leaves
    /// the switch at the outer block, past the default, so no case can run the default's code.
    /// Every one of those blocks is on `cx.labels`, so the depth of a break or a recur out of a
    /// case body counts them exactly as any other `if` or `block` would be counted.
    #[allow(clippy::too_many_arguments)]
    fn switch_table(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        cases: &[emela_lir::Case],
        default: &Option<SeqId>,
        dst: Option<VarId>,
        scrutinee: Scrutinee,
        width: usize,
    ) {
        let n = cases.len();
        // The outer block is the switch's end; the next one is where a gap lands to run the
        // default.
        ins.block(BlockType::Empty);
        cx.push(Lbl::Other);
        ins.block(BlockType::Empty);
        cx.push(Lbl::Other);
        for _ in 0..n {
            ins.block(BlockType::Empty);
            cx.push(Lbl::Other);
        }
        self.push_scrutinee(cx, ins, scrutinee);
        // From the innermost case block, the `n` case blocks are at depths `0..n`, the default
        // block at `n` and the switch's end at `n + 1`.
        let mut targets = vec![n as u32; width];
        for (i, case) in cases.iter().enumerate() {
            for key in &case.keys {
                match key {
                    CaseKey::Variant(k) => targets[*k] = i as u32,
                    CaseKey::Lit(_) => {
                        self.internal(format!("a literal case in a tag switch in {}", cx.f.name));
                        return;
                    }
                }
            }
        }
        ins.br_table(targets, n as u32);
        for (i, case) in cases.iter().enumerate() {
            cx.pop();
            ins.end();
            let body = case.body;
            self.seq(cx, ins, body, dst);
            // The case may fall through (`yield`). The switch's end is `n - i` labels out: the
            // `n - 1 - i` case blocks left and the default block are above it.
            ins.br((n - i) as u32);
        }
        cx.pop();
        ins.end();
        match default {
            Some(d) => self.seq(cx, ins, *d, dst),
            // Exhaustive without a default, as the verifier requires: the entry is not taken.
            None => {
                ins.unreachable();
            }
        }
        cx.pop();
        ins.end();
    }

    /// A switch as a chain of comparing `if` / `else`, nested once per case.
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
        self.push_case_match(cx, ins, case, scrutinee);
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        let body = case.body;
        self.seq(cx, ins, body, dst);
        ins.else_();
        self.switch_chain(cx, ins, cases, i + 1, default, dst, scrutinee);
        cx.pop();
        ins.end();
    }

    /// Pushes whether the scrutinee matches one of the case's keys: the comparison of each key,
    /// combined with `i32_or`. A case has one key unless the LIR pass joined cases whose bodies
    /// are the same sequence.
    fn push_case_match(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        case: &emela_lir::Case,
        scrutinee: Scrutinee,
    ) {
        let literal = matches!(scrutinee, Scrutinee::Lit(_));
        for (i, key) in case.keys.iter().enumerate() {
            self.push_scrutinee(cx, ins, scrutinee);
            match key {
                CaseKey::Variant(k) => {
                    ins.i32_const(*k as i32);
                }
                CaseKey::Lit(c) => {
                    self.const_val(ins, c);
                }
            }
            if literal {
                match scrutinee {
                    Scrutinee::Lit(v) if cx.f.vars[v].repr == Repr::I64 => ins.i64_eq(),
                    _ => ins.i32_eq(),
                };
            } else {
                ins.i32_eq();
            }
            if i > 0 {
                ins.i32_or();
            }
        }
        if case.keys.is_empty() {
            ins.i32_const(0);
        }
    }

    fn push_scrutinee(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, s: Scrutinee) {
        match s {
            Scrutinee::Tag(v) => self.tag_value(cx, ins, v),
            Scrutinee::Variant(v) | Scrutinee::Lit(v) => {
                ins.local_get(cx.local(v));
            }
            Scrutinee::TagZero => {
                ins.i32_const(0);
            }
        }
    }

    /// The variant number of a heap enum value: the tag of its descriptor. The runtime's `tag` is
    /// these three instructions, so the emitter writes them where it reads a tag.
    fn tag_value(&mut self, cx: &FnCtx, ins: &mut InstructionSink, v: VarId) {
        ins.local_get(cx.local(v));
        ins.i32_load(mem(8, 4));
        ins.i32_load(mem(DESC_TAG, 4));
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
                // A record held by value is a span of leaves, without an allocation.
                if let Some(d) = dst
                    && self.layout(cx.f.vars[d].ty).is_some()
                {
                    for a in args {
                        self.operand(cx, ins, a);
                    }
                    self.assign(cx, ins, Some(d), Repr::Ref);
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
                    stride: 0,
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
                    if let Some(i) = self.layout(cx.f.vars[*base].ty) {
                        // A record or tuple held by value: its field is a span of leaves, no
                        // pointer.
                        let span = self.values.get(i).field(*index);
                        for k in 0..span.len {
                            ins.local_get(cx.leaf(*base, (span.start + k) as u32));
                        }
                        self.assign(cx, ins, Some(d), Repr::Ref);
                        return;
                    }
                    if cx.aggregate(d).is_some() {
                        // A field of a boxed enum is a box of the value (E1 step 5); unbox it.
                        let base = *base;
                        self.load_field(cx, ins, base, *index, Repr::Ref);
                        self.unbox_top(cx, ins, d);
                        return;
                    }
                    let base = *base;
                    let repr = cx.f.vars[d].repr;
                    self.load_field(cx, ins, base, *index, repr);
                    ins.local_set(cx.local(d));
                }
            }
            Value::Tag(v) => {
                if cx.aggregate(*v).is_some() {
                    // A record has one constructor.
                    ins.i32_const(0);
                } else if cx.f.vars[*v].repr == Repr::Ref {
                    self.tag_value(cx, ins, *v);
                } else {
                    ins.local_get(cx.local(*v));
                }
                self.assign(cx, ins, dst, Repr::I32);
            }
            Value::Tuple(elems) => {
                if let Some(d) = dst
                    && self.layout(cx.f.vars[d].ty).is_some()
                {
                    for a in elems {
                        self.operand(cx, ins, a);
                    }
                    self.assign(cx, ins, Some(d), Repr::Ref);
                    return;
                }
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
                    stride: 0,
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
                    if cx.aggregate(d).is_some() {
                        // The list head is a box of the value (E1 step 5); unbox it.
                        let base = *v;
                        self.load_field(cx, ins, base, 0, Repr::Ref);
                        self.unbox_top(cx, ins, d);
                        return;
                    }
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
                    stride: 0,
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
                    if cx.aggregate(d).is_some() {
                        // A closure capture is a box of the value (E1 step 5); unbox it.
                        let base = *env;
                        self.load_field(cx, ins, base, index + 1, Repr::Ref);
                        self.unbox_top(cx, ins, d);
                        return;
                    }
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
                // The empty table is 0, which `ev_lookup`, `dup` and `drop` accept, so passing it
                // (to a function value whose R needs no table) allocates nothing.
                if base.is_none() && bindings.is_empty() {
                    ins.i32_const(0);
                    ins.local_set(cx.local(d));
                    return;
                }
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
                    stride: 0,
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
                // Slots 0 and 1 are left from the check of a released handler, which the types
                // made unneeded (`[effect.escape]`): a flag that stays 0 and the old message.
                let defect = self
                    .string_literal(format!("handler {name} was used after release").as_bytes());
                let desc = self.desc(DescKey {
                    kind: 0,
                    tag: 0,
                    enum_id: -1,
                    name: name.clone(),
                    show: None,
                    refs,
                    stride: 0,
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
                let from_agg = cx.aggregate(*v).is_some();
                let to_agg = cx.aggregate(d).is_some();
                match (from_agg, to_agg) {
                    // A conversion between two values keeps the leaves.
                    (true, true) => {
                        let Some(n) = cx.aggregate(*v) else { return };
                        for k in 0..n {
                            ins.local_get(cx.leaf(*v, k));
                            ins.local_set(cx.leaf(d, k));
                        }
                    }
                    // `Agg -> Ref`: the `Any` box holds a reference to a box of the value, so its
                    // one slot is a reference (E1 step 5).
                    (true, false) => {
                        let to = cx.f.vars[d].repr;
                        if to != Repr::Ref {
                            let name = cx.f.name.clone();
                            self.internal(format!("a value crosses `Any` into a word in {name}"));
                            return;
                        }
                        let desc = self.desc(DescKey {
                            kind: 0,
                            tag: 0,
                            enum_id: -1,
                            name: "box".to_owned(),
                            show: None,
                            refs: vec![true],
                            stride: 0,
                        });
                        ins.i32_const(1);
                        ins.i32_const(desc as i32);
                        let alloc = self.rt("alloc");
                        ins.call(alloc);
                        ins.local_set(cx.local(d));
                        ins.local_get(cx.local(d));
                        self.box_value(cx, ins, *v);
                        ins.i32_store(mem(HDR, 4));
                    }
                    // `Ref -> Agg`: an `Any` box whose slot 0 is a box of the value (E1 step 5).
                    (false, true) => {
                        let from = cx.f.vars[*v].repr;
                        if from != Repr::Ref {
                            let name = cx.f.name.clone();
                            self.internal(format!("a word is unboxed from `Any` in {name}"));
                            return;
                        }
                        ins.local_get(cx.local(*v));
                        ins.i32_load(mem(HDR, 4));
                        self.unbox_top(cx, ins, d);
                    }
                    (false, false) => {
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
                                stride: 0,
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

    /// The `Make` `dst = …` of a statement whose old block is reused (C4/PR-042): the destination
    /// becomes the old variable, every old field that holds a reference is released, and the new
    /// fields are written into the same object.
    ///
    /// `opt::reuse` proved that the old variable has exactly one use, the removed drop, so no
    /// argument reads the old block and no other variable reaches it. A reference field is
    /// released with the drop the old block's own release would have run, before the new field is
    /// stored: the slot owns one reference and the new argument owns another (the `Make` consumes
    /// it), so the release cannot free the argument's value, even when both denote the same
    /// object. The constructor and the type are the same, so the descriptor, the tag, the number
    /// of slots and the reference slots are the ones the object already has. A nullary
    /// constructor is static data, which both values already are.
    fn reuse_make(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        old: VarId,
        value: &emela_lir::Value,
    ) {
        let emela_lir::Value::Make { ctor, args } = value else {
            self.internal(format!(
                "a reuse of a value that is not a `make` in {}",
                cx.f.name
            ));
            return;
        };
        let Some(d) = dst else {
            self.internal(format!("reuse without a destination in {}", cx.f.name));
            return;
        };
        if enum_repr(self.core(), ctor.enum_id) == Repr::I32 {
            // The representation is the tag itself: there is no block.
            ins.local_get(cx.local(old));
            ins.local_set(cx.local(d));
            return;
        }
        if self.layout(cx.f.vars[d].ty).is_some() {
            // Held by value: there is no allocation to reuse; the copy is the whole statement.
            for a in args {
                self.operand(cx, ins, a);
            }
            self.assign(cx, ins, Some(d), Repr::Ref);
            return;
        }
        ins.local_get(cx.local(old));
        ins.local_set(cx.local(d));
        // The old slots own one reference each: give each back with the drop the removed `drop`
        // would have run, in slot order, before the slot is overwritten.
        let drop = self.rt("drop");
        for slot in self.ref_slots(cx.f, old, ctor.enum_id, ctor.variant) {
            ins.local_get(cx.local(d));
            ins.i32_load(mem(HDR + slot as u32 * 8, 4));
            ins.call(drop);
        }
        for (i, a) in args.iter().enumerate() {
            self.store_slot(cx, ins, d, i, a);
        }
    }

    /// The slots of the object of `old` that hold a reference, in slot order, from the fields of
    /// the constructor's variant and the type arguments of the object's type.
    ///
    /// The descriptor was made from the same field representations ([`Value::Make`] marks a slot
    /// a reference when its argument is `Repr::Ref`, and the verifier takes an argument's
    /// representation from the field type), so these are the slots the old block's drop would
    /// release.
    fn ref_slots(
        &mut self,
        f: &Function,
        old: VarId,
        enum_id: EnumId,
        variant: usize,
    ) -> Vec<usize> {
        let reprs = {
            let core = self.core();
            match core.ty(f.vars[old].ty) {
                Type::Enum(id, args) if *id == enum_id => {
                    core.enums[enum_id].variants.get(variant).map(|v| {
                        v.tys
                            .iter()
                            .map(|&t| repr_of_subst(core, t, args))
                            .collect::<Vec<_>>()
                    })
                }
                _ => None,
            }
        };
        let Some(reprs) = reprs else {
            self.internal(format!("a reuse of an unexpected value in {}", f.name));
            return Vec::new();
        };
        reprs
            .iter()
            .enumerate()
            .filter(|(_, r)| **r == Repr::Ref)
            .map(|(i, _)| i)
            .collect()
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
                let m = self.m;
                let f = &m.functions[*g];
                // The result of a flattened function is one wasm result per scalar leaf.
                let ret_leaves = self.leaves(f.ret).map(|l| l.to_vec());
                let ret = self.repr(f.ret);
                // The arguments must push exactly the parameters the signature declares: one
                // value per scalar leaf of a by-value parameter, one for any other.
                let expected: usize = f
                    .params
                    .iter()
                    .map(|&p| self.leaves(f.vars[p].ty).map_or(1, |l| l.len()))
                    .sum();
                let pushed: usize = args.iter().map(|a| Self::operand_width(cx, a)).sum();
                if pushed != expected {
                    let (callee, caller) = (f.name.clone(), cx.f.name.clone());
                    self.internal(format!(
                        "{callee} takes {expected} wasm arguments in {caller} but the call \
                         pushes {pushed}"
                    ));
                    return;
                }
                for a in args {
                    self.operand(cx, ins, a);
                }
                let index = self.fn_index(*g);
                ins.call(index);
                self.assign_result(cx, ins, dst, ret, ret_leaves);
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
                self.indirect(cx, ins, dst, *c, args, fty.ret);
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

    /// A `call_indirect` of a closure. The closure record is one reference parameter (the
    /// environment); each other parameter and the result follow the layouts of the function type,
    /// so an indirect call has the same signature as the function it enters.
    fn indirect(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        closure: VarId,
        args: &[Operand],
        ret_ty: TypeId,
    ) {
        let mut params = vec![val(Repr::Ref)];
        params.extend(self.operand_val_types(cx, args));
        let (results, ret_leaves) = self.result_val_types(ret_ty);
        let ty = self.ty(params, results);
        ins.local_get(cx.local(closure));
        for a in args {
            self.operand(cx, ins, a);
        }
        ins.local_get(cx.local(closure));
        ins.i32_load(mem(HDR, 4));
        ins.call_indirect(0, ty);
        let ret = self.repr(ret_ty);
        self.assign_result(cx, ins, dst, ret, ret_leaves);
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
        // value to continue with comes from `perform_end` (spec 8.4, 14.5). Its parameters follow
        // the layouts of the operation's function type (the implementation is a lifted closure of
        // that type); its result stays boxed, because the resume channel carries one word.
        let mut params = vec![val(Repr::Ref)];
        params.extend(self.operand_val_types(cx, args));
        if suspend {
            params.push(val(Repr::Ref));
        }
        let (results, ret_leaves) = if suspend {
            (Vec::new(), None)
        } else {
            self.result_val_types(def.ret)
        };
        let ty = self.ty(params, results);
        let op_off = HDR + 8 * (op as u32 + 2);
        let rs = if suspend {
            let rs = cx.scratch(Repr::Ref, 0);
            let prefix =
                self.string_literal(format!("operation `{}` of handler `", def.name).as_bytes());
            let resume = self.resume_table(ret);
            ins.i32_const(prefix as i32);
            ins.local_get(cx.local(handler));
            ins.i32_const(resume as i32);
            ins.i32_const(repr_code(ret));
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
            self.assign(cx, ins, dst, ret);
        } else {
            self.assign_result(cx, ins, dst, ret, ret_leaves);
        }
    }

    /// the run-time scope of a group, `fork`, `join`, the Deferred operations and the
    /// operations of `Std.Cause` (spec chapter 11), with the runtime of `runtime::fiber`.
    fn fiber(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        op: FiberOp,
        args: &[Operand],
    ) {
        match op {
            // The Semaphore and Queue operations (`runtime::fiber::line`): the operands in order, and
            // the result, if any, in the destination.
            FiberOp::Semaphore
            | FiberOp::Queue
            | FiberOp::Acquire
            | FiberOp::TryAcquire
            | FiberOp::Release
            | FiberOp::Available
            | FiberOp::Take
            | FiberOp::QueuePoll
            | FiberOp::Size
            | FiberOp::Close => {
                let (name, ret) = match op {
                    FiberOp::Semaphore => ("fb_semaphore", Some(Repr::Ref)),
                    FiberOp::Queue => ("fb_queue", Some(Repr::Ref)),
                    FiberOp::Acquire => ("fb_acquire", None),
                    FiberOp::TryAcquire => ("fb_try_acquire", Some(Repr::I32)),
                    FiberOp::Release => ("fb_release", None),
                    FiberOp::Available => ("fb_available", Some(Repr::I32)),
                    FiberOp::Take => ("fb_take", Some(Repr::Ref)),
                    FiberOp::QueuePoll => ("fb_queue_poll", Some(Repr::Ref)),
                    FiberOp::Size => ("fb_queue_size", Some(Repr::I32)),
                    _ => ("fb_queue_close", None),
                };
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt(name);
                ins.call(f);
                if let Some(repr) = ret {
                    self.assign(cx, ins, dst, repr);
                }
            }
            FiberOp::Offer | FiberOp::TryOffer => {
                let repr = self.operand_repr_of(cx, &args[1]);
                self.operand(cx, ins, &args[0]);
                // A value without representation is passed as the zero word, as for complete.
                if repr == Repr::Void {
                    ins.i64_const(0);
                } else {
                    self.operand(cx, ins, &args[1]);
                    if repr == Repr::F64 {
                        ins.i64_reinterpret_f64();
                    } else if repr != Repr::I64 {
                        ins.i64_extend_i32_u();
                    }
                }
                ins.i32_const(repr_code(repr));
                let f = self.rt(if op == FiberOp::Offer {
                    "fb_offer"
                } else {
                    "fb_try_offer"
                });
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::Scope { background, .. } => {
                let ret = self.operand_fn_ret(cx, &args[0]);
                let repr = self.repr(ret);
                let table = self.thunk1(repr);
                for a in args {
                    self.operand(cx, ins, a);
                }
                ins.i32_const(table as i32);
                ins.i32_const(repr_code(repr));
                ins.i32_const(i32::from(background));
                let f = self.rt("fb_scope");
                ins.call(f);
                self.assign_i64(cx, ins, dst, repr);
            }
            FiberOp::Fork => {
                let ret = self.operand_fn_ret(cx, &args[1]);
                let repr = self.repr(ret);
                let table = self.thunk0(repr);
                for a in args {
                    self.operand(cx, ins, a);
                }
                ins.i32_const(table as i32);
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_fork");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Deferred => {
                let f = self.rt("fb_deferred");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Join => {
                let repr = self.dst_repr(cx, dst);
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_join");
                ins.call(f);
                self.assign_i64(cx, ins, dst, repr);
            }
            FiberOp::Complete => {
                let repr = self.operand_repr_of(cx, &args[1]);
                self.operand(cx, ins, &args[0]);
                // A value without representation is passed as the zero word (the C runtime's
                // `em_deferred_complete`).
                if repr == Repr::Void {
                    ins.i64_const(0);
                } else {
                    self.operand(cx, ins, &args[1]);
                    if repr == Repr::F64 {
                        ins.i64_reinterpret_f64();
                    } else if repr != Repr::I64 {
                        ins.i64_extend_i32_u();
                    }
                }
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_complete");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::DeferredAwait => {
                let repr = self.dst_repr(cx, dst);
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_deferred_await");
                ins.call(f);
                self.assign_i64(cx, ins, dst, repr);
            }
            FiberOp::Poll => {
                let (some, none, repr) = match dst {
                    Some(d) => self
                        .option_parts(cx.f.vars[d].ty)
                        .unwrap_or_else(|| panic!("Deferred.poll without an Option type")),
                    None => return,
                };
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr));
                ins.i32_const(some as i32);
                ins.i32_const(none as i32);
                let f = self.rt("fb_poll");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
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
                let success = runtime::fiber::attempt::success_desc(self, ret);
                if suspends {
                    let frame = runtime::fiber::attempt::frame_desc(self);
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
        }
    }

    fn operand_fn_ret(&self, cx: &FnCtx<'_>, op: &Operand) -> TypeId {
        match op {
            Operand::Var(v) => match self.core().ty(cx.f.vars[*v].ty) {
                Type::Fn(f) => f.ret,
                _ => TypeId::UNIT,
            },
            Operand::Const(_) => TypeId::UNIT,
        }
    }

    fn operand_repr_of(&self, cx: &FnCtx<'_>, op: &Operand) -> Repr {
        match op {
            Operand::Var(v) => self.repr(cx.f.vars[*v].ty),
            Operand::Const(c) => match c {
                emela_lir::Const::Int(_) | emela_lir::Const::Bool(_) => Repr::I32,
                emela_lir::Const::Int64(_) => Repr::I64,
                emela_lir::Const::Float(_) => Repr::F64,
                emela_lir::Const::String(_) => Repr::Ref,
                emela_lir::Const::Unit => Repr::Void,
            },
        }
    }

    /// The representation of a `join` or `Deferred.await` result. Without a destination the value
    /// is unused; it is read as a reference and released.
    fn dst_repr(&self, cx: &FnCtx<'_>, dst: Option<VarId>) -> Repr {
        match dst {
            Some(d) => self.repr(cx.f.vars[d].ty),
            None => Repr::Ref,
        }
    }

    /// The table index of the closure thunk for `repr` (`fn() -> A`).
    fn thunk0(&self, repr: Repr) -> u32 {
        let plan = self
            .fiber_plan
            .as_ref()
            .expect("the fiber runtime for a forked fiber");
        plan.call_table + repr_code(repr) as u32
    }

    /// The table index of the closure thunk for `repr` (`fn(Scope) -> A`).
    fn thunk1(&self, repr: Repr) -> u32 {
        let plan = self
            .fiber_plan
            .as_ref()
            .expect("the fiber runtime for a scope");
        plan.call1_table + repr_code(repr) as u32
    }

    /// Stores the `i64` word a fiber operation returned, as `repr`.
    fn assign_i64(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        repr: Repr,
    ) {
        match repr {
            Repr::I32 | Repr::Ref => {
                ins.i32_wrap_i64();
            }
            Repr::I64 => {}
            Repr::F64 => {
                ins.f64_reinterpret_i64();
            }
            Repr::Void => {
                ins.drop();
                return;
            }
        }
        self.assign(cx, ins, dst, repr);
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

/// Below this many cases, the chain of comparing `if`s is shorter than the nested blocks and the
/// dispatch of a table.
const SWITCH_TABLE_MIN_CASES: usize = 3;

/// How many table entries (bytes) a case of the chain may be worth. A chain case is a comparison
/// and its `if`, which is more than one byte, so the table may be this much wider than the switch
/// has cases.
const SWITCH_TABLE_WIDTH_SCALE: usize = 8;

/// A switch's scrutinee.
#[derive(Clone, Copy)]
enum Scrutinee {
    /// An enum value with a heap representation: the tag is read with the `tag` runtime function.
    Tag(VarId),
    /// An enum value whose representation is its variant number.
    Variant(VarId),
    /// An enum value held by value in locals: its one constructor is 0.
    TagZero,
    /// A literal `i32` or `i64` value.
    Lit(VarId),
}

/// The type a value that may suspend produces. `None` when it is not a call (a fiber or runtime
/// operation has its own handling).
fn value_ret_ty(b: &Builder<'_>, cx: &FnCtx<'_>, value: &Value) -> Option<TypeId> {
    let Value::Call { callee, .. } = value else {
        return None;
    };
    Some(match callee {
        Callee::Direct(g) => b.m.functions[*g].ret,
        Callee::Op { effect, op, .. } => b.core().effects[*effect].ops[*op].ret,
        Callee::Indirect(c) => match b.core().ty(cx.f.vars[*c].ty) {
            Type::Fn(fty) => fty.ret,
            _ => return None,
        },
    })
}

/// The representation of a value that may suspend. The suspension edge needs it when the value is
/// unused (`dst` is `None`) and the resumed reference must be released.
fn value_repr(b: &Builder<'_>, cx: &FnCtx<'_>, value: &Value) -> Repr {
    value_ret_ty(b, cx, value).map_or(Repr::Void, |ty| b.repr(ty))
}

/// The scalar leaves of a value that may suspend when its type is held by value.
fn value_leaves(b: &Builder<'_>, cx: &FnCtx<'_>, value: &Value) -> Option<Vec<Repr>> {
    let ty = value_ret_ty(b, cx, value)?;
    b.leaves(ty).map(|leaves| leaves.to_vec())
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

/// [`dummy_return`] for a function whose result is a span of leaves (one wasm result each).
fn dummy_return_values(ins: &mut InstructionSink, leaves: &[Repr]) {
    for &r in leaves {
        match r {
            Repr::Void => {}
            Repr::I32 | Repr::Ref => {
                ins.i32_const(0);
            }
            Repr::I64 => {
                ins.i64_const(0);
            }
            Repr::F64 => {
                ins.f64_const(0.0.into());
            }
        }
    }
    ins.return_();
}

/// A wasm label on the stack, for computing `br` depths.
enum Lbl {
    Block(LabelId, Option<VarId>),
    Loop(Vec<VarId>),
    Other,
}

/// The locals of one variable held by value: its representation of each leaf. The first local is
/// `FnCtx::local(v)`.
struct LocalSpan {
    leaves: Vec<Repr>,
}

struct FnCtx<'a> {
    f: &'a Function,
    /// The wasm results of the function: one per leaf of a by-value result, one for any other
    /// representation except `()`, and empty for `()`.
    ret_leaves: Vec<Repr>,
    local: Vec<u32>,
    /// `Some` when the variable is held by value in a span of locals.
    spans: Vec<Option<LocalSpan>>,
    scratch_base: [u32; 4],
    /// A scratch local for the reference of a box while an escape boundary boxes or unboxes a
    /// value (E1 step 5). Only declared when the module holds some value by value.
    box_ref: Option<u32>,
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
        let mut local = vec![0u32; f.vars.len()];
        let mut spans: Vec<Option<LocalSpan>> = (0..f.vars.len()).map(|_| None).collect();
        let mut decls: Vec<(u32, ValType)> = Vec::new();
        // A parameter of a flattened signature occupies one wasm parameter per leaf, in field
        // order (E1 step 3). Otherwise it is its single local from the boxed signature.
        let mut nparams = 0u32;
        for &p in &f.params {
            local[ix(p)] = nparams;
            match b.leaves(f.vars[p].ty) {
                Some(leaves) if !leaves.is_empty() => {
                    nparams += leaves.len() as u32;
                    spans[ix(p)] = Some(LocalSpan {
                        leaves: leaves.to_vec(),
                    });
                }
                _ => nparams += 1,
            }
        }
        let mut next = nparams + extra;
        for (v, data) in f.vars.iter() {
            if f.params.contains(&v) {
                continue;
            }
            local[ix(v)] = next;
            match b.leaves(data.ty) {
                // A value held by value is a span of locals, one per scalar leaf, in field order.
                Some(leaves) if !leaves.is_empty() => {
                    for &r in leaves {
                        decls.push((1, val(r)));
                    }
                    next += leaves.len() as u32;
                    spans[ix(v)] = Some(LocalSpan {
                        leaves: leaves.to_vec(),
                    });
                }
                _ => {
                    decls.push((1, val(data.repr)));
                    next += 1;
                }
            }
        }
        // Scratch locals, one group per representation: for the `recur` parallel assignment and
        // the general temporaries. At least one of each so a temporary always exists. A value
        // held by value takes one slot per leaf, so a whole `recur` needs the sum of the leaves.
        let count = recur_slots(f, b).max(1);
        let mut scratch_base = [0u32; 4];
        for base in scratch_base.iter_mut() {
            *base = next;
            next += count;
        }
        decls.push((count, ValType::I32));
        decls.push((count, ValType::I64));
        decls.push((count, ValType::F64));
        decls.push((count, ValType::I32));
        // A single extra local holds a box reference while an escape boundary boxes or unboxes a
        // value. It is only declared when the module holds a value by value, so a boxed build's
        // bytes (and the existing pins) do not change.
        let box_ref = if b.has_values() {
            let local = next;
            decls.push((1, ValType::I32));
            Some(local)
        } else {
            None
        };
        let ret = b.repr(f.ret);
        let ret_leaves = match b.leaves(f.ret) {
            Some(leaves) => leaves.to_vec(),
            None if ret == Repr::Void => Vec::new(),
            None => vec![ret],
        };
        FnCtx {
            f,
            ret_leaves,
            local,
            spans,
            scratch_base,
            box_ref,
            decls,
            labels: Vec::new(),
        }
    }

    /// The first local of `v`.
    fn local(&self, v: VarId) -> u32 {
        self.local[ix(v)]
    }

    /// The number of locals of `v` if it is held by value, else `None`.
    fn aggregate(&self, v: VarId) -> Option<u32> {
        self.spans[ix(v)].as_ref().map(|s| s.leaves.len() as u32)
    }

    /// The local of leaf `i` of the value in `v`.
    fn leaf(&self, v: VarId, i: u32) -> u32 {
        self.local[ix(v)] + i
    }

    /// The representation of leaf `i` of the value in `v`.
    fn leaf_repr(&self, v: VarId, i: u32) -> Repr {
        self.spans[ix(v)]
            .as_ref()
            .expect("a leaf of a scalar") // a scalar has exactly one leaf at index 0
            .leaves[i as usize]
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

/// The number of scratch locals a `recur` needs: one per argument, but a value held by value
/// contributes one per leaf.
fn recur_slots(f: &Function, b: &Builder<'_>) -> u32 {
    let mut max = 0;
    for (_, seq) in f.seqs.iter() {
        if let Term::Recur(args) = &seq.term {
            let mut n = 0u32;
            for a in args {
                n += match a {
                    Operand::Var(v) => b
                        .leaves(f.vars[*v].ty)
                        .map_or(1, |leaves| leaves.len() as u32),
                    Operand::Const(c) => u32::from(c.repr() != Repr::Void),
                };
            }
            // At least one slot per argument, as before values existed.
            max = max.max(n.max(args.len() as u32));
        }
    }
    max
}
