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
//! The lowering is split by feature: this module has the entry point, the `Builder` skeleton, the
//! operands and the value of a `let`; `imports` adds the WASI imports, `statics` the static data,
//! `bodies` the function bodies and the assembled module, and `lower` the IR statements and the
//! rest of the values; `builtins` lowers `Builtin`; `external` handles the host imports and their
//! stubs; `runtimeop` lowers `RuntimeOp`; `suspend` emits the state machines of suspending
//! functions and begins a `suspend fn` operation call.

mod bodies;
mod builtins;
mod external;
mod imports;
pub mod layout;
mod lower;
mod runtime;
mod runtimeop;
mod statics;
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
    /// (`runtime/wasi/http.rs`).
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
/// The output streams and the waitable set of the WASI 0.3 host calls (`runtime/wasi/host.rs`).
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
    pub(crate) fs_plan: Option<runtime::wasi::fs::Plan>,
    pub(crate) http_plan: Option<runtime::wasi::http::Plan>,
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
            runtime::wasi::fs::imports(self);
        }
        self.exit_with_code();
        self.output_imports("wasi:cli/stdout@0.3.0");
        self.output_imports("wasi:cli/stderr@0.3.0");
        self.waitable_set_new();
        self.waitable_set_wait();
        self.waitable_set_drop();
        self.waitable_join();
        if self.service {
            runtime::wasi::http::imports(self);
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
