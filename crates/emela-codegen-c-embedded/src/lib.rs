//! Emela's embedded C output (spec 16.6).
//!
//! Like the host C target, it goes through the Flow IR and the Heap IR (`emela-lir`), with value
//! types (`Repr::Agg`) on, defects halting where they happen, and function values, handlers and
//! evidence tables specialized statically. There is no runtime: the heap, when there is one, is
//! generated per program, and the board's features come as the C functions and hooks of the board
//! headers. The design is in `docs/design/c-embedded-architecture.md`.

mod board;
mod check;
mod emit;
mod heap;
#[allow(dead_code)] // A copy of emela-codegen-c's; not every helper is used here.
mod names;
mod strings;
#[allow(dead_code)] // A copy of emela-codegen-c's; not every helper is used here.
mod writer;

use emela_core::{EnumId, FnId, Module, TypeId};
use emela_lir::{Layout, Repr};
use rustc_hash::FxHashMap;

use crate::check::Entry;

/// The most steps the compile-time evaluation of one const takes.
const CONST_STEPS: u64 = 10_000_000;

/// The board hooks' header.
pub const BOARD_H_FILE: &str = "emela_board.h";
/// The header of the entries the board calls.
pub const EMBEDDED_H_FILE: &str = "emela_embedded.h";
/// The program.
pub const MAIN_C_FILE: &str = "main.c";

/// The output files as `(file name, contents)`, `main.c` first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub files: Vec<(String, String)>,
    /// The project's C files external functions point to, as `(name in the output, source path)`;
    /// the caller copies them.
    pub copies: Vec<(String, std::path::PathBuf)>,
}

/// The settings of the embedded C output (`[embedded.*]` of `Pome.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The bytes (UTF-8) one String can hold; more is a defect. `[embedded.memory] string_capacity`.
    pub string_capacity: u16,
    /// The bytes of Emela's heap (`[embedded.memory] heap_bytes`); 0 means no heap.
    pub heap_bytes: u32,
}

impl Options {
    /// The most `string_capacity` may be.
    pub const MAX_STRING_CAPACITY: u16 = 512;
}

impl Default for Options {
    fn default() -> Self {
        Options {
            string_capacity: 256,
            heap_bytes: 0,
        }
    }
}

/// A diagnostic for a program the embedded C target cannot output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Error {
            message: message.into(),
        }
    }
}

/// Writes the embedded C11 source and the board headers from Core IR (lowered, not yet
/// monomorphized).
///
/// The entries are `main` and the public functions, which the board calls as
/// `emela_export_<name>`. A public function taking a `GpioEvent` is an interrupt entry; the
/// functions it reaches may not be recursive.
pub fn compile(module: &Module) -> Result<Output, Vec<Error>> {
    compile_with(module, &Options::default())
}

/// [`compile`] with the settings.
pub fn compile_with(module: &Module, options: &Options) -> Result<Output, Vec<Error>> {
    let roots = roots(module)?;
    compile_entries(module, roots, options)
}

/// The output for the entries `roots`: `main` and the public functions.
fn compile_entries(
    module: &Module,
    roots: Vec<FnId>,
    options: &Options,
) -> Result<Output, Vec<Error>> {
    let entry_count = roots.len();
    let mono_roots: Vec<FnId> = roots.iter().copied().chain(static_roots(module)).collect();
    let (mono, copies) = emela_core::monomorphize_from(module, &mono_roots);
    let mut copies = copies;
    copies.truncate(entry_count);
    let Some(copies) = copies.into_iter().collect::<Option<Vec<_>>>() else {
        return Err(vec![Error::new(
            "the entries of the embedded C target (`main` and public functions) cannot have type parameters",
        )]);
    };
    if let Err(errors) = emela_core::verify_monomorphic(&mono) {
        return Err(internal("monomorphisation", errors));
    }
    let mut lir = emela_lir::from_core_with(
        mono,
        emela_lir::Options {
            value_types: true,
            halt_on_defect: true,
        },
    )
    .map_err(|errors| internal("from_core", errors))?;
    dump("flow", &lir);
    emela_lir::convert_closures(&mut lir);
    // The Flow IR functions are the Core functions in the same order, so a root keeps the index
    // of its copy. (The static states' consts are found among the Flow IR functions.)
    let roots: Vec<emela_lir::FnId> = copies
        .iter()
        .map(|&copy| emela_lir::FnId::from_raw(la_arena::RawIdx::from(u32::from(copy.into_raw()))))
        .collect();
    let board_op = |core: &Module, effect: emela_core::EffectId, op: usize| {
        let def = &core.effects[effect];
        board::split(&def.name).map(|(p, _)| p.op_function(&def.ops[op].name))
    };
    let statics = static_states(&lir)?;
    let state_options: Vec<emela_lir::StaticState> = statics
        .iter()
        .enumerate()
        .map(|(k, s)| emela_lir::StaticState {
            effect: s.effect,
            load: format!("em_state_{k}_load"),
            store: format!("em_state_{k}_store"),
            enter: format!("em_state_{k}_enter"),
            exit: format!("em_state_{k}_exit"),
        })
        .collect();
    // The `@static` consts are roots too: their values are the states' initial values.
    let all_roots: Vec<emela_lir::FnId> = roots
        .iter()
        .copied()
        .chain(statics.iter().map(|s| s.init))
        .collect();
    let copies = emela_lir::specialize_statics(
        &mut lir,
        &all_roots,
        &emela_lir::StaticsOptions {
            board_op: &board_op,
            states: &state_options,
        },
    )
    .map_err(|errors| errors.into_iter().map(Error::new).collect::<Vec<_>>())?;
    let (roots, inits) = copies.split_at(roots.len());
    let roots = roots.to_vec();
    emela_lir::run_to_completion(&mut lir);
    dump("statics", &lir);
    emela_lir::explicit_defects(&mut lir);
    emela_lir::state_machines(&mut lir);
    emela_lir::insert_rc(&mut lir);
    dump("heap", &lir);
    let entries: Vec<Entry> = roots
        .iter()
        .map(|&id| {
            let f = &lir.functions[id];
            let main = f.name == "main";
            Entry {
                function: id,
                export: (!main).then(|| format!("emela_export_{}", c_ident(&f.name))),
                isr: !main
                    && f.params
                        .iter()
                        .any(|&p| check::is_gpio_event(&lir, f.vars[p].ty)),
            }
        })
        .collect();
    // The embedded constraints first: the verifier would also reject some of what they reject
    // (a function value left in a data structure), as an internal error.
    let reached = check::check(&lir, &entries, options.heap_bytes > 0)?;
    let errors = check::pins(&lir, &reached);
    if !errors.is_empty() {
        return Err(errors);
    }
    if let Err(errors) = emela_lir::verify(&lir) {
        return Err(internal("the Heap IR verifier", errors));
    }
    let mut prepared = prepare(&mut lir, &reached);
    prepared.string_capacity = options.string_capacity;
    prepared.heap_bytes = options.heap_bytes;
    // The states' initial values must be known at compile time.
    let values = emela_lir::eval_consts(&lir, inits, CONST_STEPS);
    let mut errors = Vec::new();
    for (k, &init) in inits.iter().enumerate() {
        let name = check::display(&lir.functions[init].name).to_owned();
        match values.get(&init) {
            Some(emela_lir::ConstResult::Value(v)) => prepared.states.push(State {
                ty: lir.functions[init].ret,
                value: v.clone(),
                load: state_options[k].load.clone(),
                store: state_options[k].store.clone(),
                enter: state_options[k].enter.clone(),
                exit: state_options[k].exit.clone(),
                critical: shared_state(&lir, &entries, &state_options[k]),
            }),
            _ => errors.push(Error::new(format!(
                "the value of the `@static` const `{name}` is not known at compile time: it is the initial value of a static State"
            ))),
        }
    }
    let state_types: Vec<TypeId> = prepared.states.iter().map(|s| s.ty).collect();
    for ty in state_types {
        if !matches!(lir.repr(ty), Repr::I32 | Repr::I64 | Repr::F64 | Repr::Agg) {
            let ty = emela_lir::core_ty(&lir.core, ty);
            errors.push(Error::new(format!(
                "the type {ty} of a static State uses the heap: interrupts may touch it, so it must be a value type"
            )));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut errors = Vec::new();
    for (id, result) in emela_lir::eval_consts(&lir, &reached, CONST_STEPS) {
        match result {
            emela_lir::ConstResult::Value(v) => {
                prepared.consts.insert(id, v);
            }
            emela_lir::ConstResult::TooLong => errors.push(Error::new(format!(
                "the const `{}` could not be evaluated at compile time (more than {CONST_STEPS} steps, or too deep a recursion)",
                check::display(&lir.functions[id].name)
            ))),
            // Raised at run time where it is referenced (spec 5.6), or evaluated at run time.
            emela_lir::ConstResult::Defect(_) | emela_lir::ConstResult::NotStatic => {}
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // What only the evaluated consts used is not emitted.
    let evaluated = prepared.consts.keys().copied().collect();
    let reached = check::reach_except(&lir, entries.iter().map(|e| e.function), &evaluated);
    let files = emit::emit(&lir, &prepared, &entries, &reached)?;
    let mut out = vec![
        (MAIN_C_FILE.to_owned(), files.main),
        (BOARD_H_FILE.to_owned(), files.board),
    ];
    out.extend(files.peripherals);
    out.push((EMBEDDED_H_FILE.to_owned(), files.embedded));
    out.extend(files.bundled);
    Ok(Output {
        files: out,
        copies: files.copies,
    })
}

/// A static state: the `State[S]` the target provides for a `@static` const.
struct StaticDecl {
    effect: emela_core::EffectId,
    /// The const giving the initial value.
    init: emela_lir::FnId,
}

/// The static states of the program: the `@static` consts whose `State` is used.
fn static_states(m: &emela_lir::Module) -> Result<Vec<StaticDecl>, Vec<Error>> {
    let mut out: Vec<StaticDecl> = Vec::new();
    let mut errors = Vec::new();
    for (id, f) in m.functions.iter() {
        if !f.static_state {
            continue;
        }
        let ty = f.ret;
        let effect = m.core.effects.iter().find(|(_, e)| {
            e.decl == "Std.State.State" && e.args.as_slice() == [ty] && e.label_of.is_none()
        });
        let Some((effect, _)) = effect else {
            continue;
        };
        if out.iter().any(|s| s.effect == effect) {
            let ty = emela_lir::core_ty(&m.core, ty);
            errors.push(Error::new(format!(
                "two `@static` consts have the type {ty}: there is one static State per type"
            )));
            continue;
        }
        out.push(StaticDecl { effect, init: id });
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// `main` and the public functions.
fn roots(module: &Module) -> Result<Vec<FnId>, Vec<Error>> {
    let mut roots: Vec<FnId> = module
        .functions
        .iter()
        .filter(|(_, f)| f.exported && f.name == "main")
        .map(|(id, _)| id)
        .collect();
    if roots.is_empty() {
        return Err(vec![Error::new(
            "the embedded C target needs a `main` function",
        )]);
    }
    roots.extend(
        module
            .functions
            .iter()
            .filter(|(_, f)| f.exported && f.name != "main")
            .map(|(id, _)| id),
    );
    Ok(roots)
}

/// The `@static` consts, kept by monomorphisation for their states' initial values.
fn static_roots(module: &Module) -> Vec<FnId> {
    module
        .functions
        .iter()
        .filter(|(_, f)| f.static_state)
        .map(|(id, _)| id)
        .collect()
}

/// What the emitter needs that interns types: the layouts of the value aggregates it meets and
/// the types of the board's enums.
pub(crate) struct Prepared {
    pub layouts: FxHashMap<TypeId, Layout>,
    pub board_types: FxHashMap<EnumId, TypeId>,
    /// The error enums a failure (`EmError`) may hold, in tag order (tag `i + 1`).
    pub errors: Vec<(EnumId, TypeId)>,
    /// The values of the consts evaluated at compile time (③ static evaluation).
    pub consts: FxHashMap<emela_lir::FnId, emela_lir::ConstValue>,
    /// The static states, with their accessors' names.
    pub states: Vec<State>,
    /// The bytes a String holds at most (`[embedded.memory] string_capacity`).
    pub string_capacity: u16,
    /// The boxes of the heap values, by type.
    pub boxes: FxHashMap<TypeId, heap::Boxed>,
    /// `heap_bytes`.
    pub heap_bytes: u32,
}

/// A static state held in static storage.
pub(crate) struct State {
    pub ty: TypeId,
    pub value: emela_lir::ConstValue,
    pub load: String,
    pub store: String,
    pub enter: String,
    pub exit: String,
    /// Whether its operations need a critical section: more than one entry (`main`, an interrupt
    /// entry, another exported function, which C may call from anywhere) uses it.
    pub critical: bool,
}

/// Whether more than one entry reaches the operations of `state`.
fn shared_state(m: &emela_lir::Module, entries: &[Entry], state: &emela_lir::StaticState) -> bool {
    let uses = |f: &emela_lir::Function| {
        f.external.as_ref().is_some_and(|e| {
            e.bindings.iter().any(
                |b| matches!(&b.host, emela_core::HostRef::Global(name) if *name == state.enter),
            )
        })
    };
    entries
        .iter()
        .filter(|e| {
            check::reach(m, [e.function])
                .into_iter()
                .any(|id| uses(&m.functions[id]))
        })
        .count()
        > 1
}

fn prepare(m: &mut emela_lir::Module, reached: &[emela_lir::FnId]) -> Prepared {
    let mut board_types = FxHashMap::default();
    let ids: Vec<EnumId> = m
        .core
        .enums
        .iter()
        .filter(|(_, e)| e.params == 0 && board::split(&e.name).is_some())
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        board_types.insert(id, m.core.enum_type(id, Vec::new()));
    }
    // The error enums the program's failures may be: in `EmError` (`Layout::Failure`).
    let mut error_ids: Vec<EnumId> = Vec::new();
    for &id in reached {
        let f = &m.functions[id];
        let mut add = |e: EnumId| {
            if !error_ids.contains(&e) {
                error_ids.push(e);
            }
        };
        for (_, v) in f.vars.iter() {
            if let emela_core::Type::Enum(e, _) = m.core.ty(v.ty)
                && m.core.enums[*e].error
            {
                add(*e);
            }
        }
        for (_, s) in f.stmts.iter() {
            if let emela_lir::Stmt::Let {
                value:
                    emela_lir::Value::IsError { enum_id, .. }
                    | emela_lir::Value::AsError { enum_id, .. },
                ..
            } = s
            {
                add(*enum_id);
            }
        }
    }
    error_ids.sort_by_key(|e| u32::from(e.into_raw()));
    // A board operation's failure is `PeripheralError(code)`.
    if reached.iter().any(|&id| {
        let f = &m.functions[id];
        f.external.is_some() && f.fails && board::split_op(&f.name).is_some()
    }) && let Some((e, _)) = m
        .core
        .enums
        .iter()
        .find(|(_, e)| e.name == "Std.Embedded.Peripheral.PeripheralError")
        && !error_ids.contains(&e)
    {
        error_ids.push(e);
        error_ids.sort_by_key(|e| u32::from(e.into_raw()));
    }
    error_ids.retain(|&e| m.core.enums[e].params == 0);
    let errors: Vec<(EnumId, TypeId)> = error_ids
        .into_iter()
        .map(|e| (e, m.core.enum_type(e, Vec::new())))
        .collect();
    let mut pending: Vec<TypeId> = board_types.values().copied().collect();
    pending.extend(errors.iter().map(|&(_, t)| t));
    for &id in reached {
        let f = &m.functions[id];
        pending.push(f.ret);
        pending.extend(f.vars.iter().map(|(_, v)| v.ty));
    }
    for (_, e) in m.core.effects.iter() {
        if board::split(&e.name).is_some() {
            for op in &e.ops {
                pending.push(op.ret);
                pending.extend(op.params.iter().copied());
            }
        }
    }
    let mut layouts = FxHashMap::default();
    let mut boxes = FxHashMap::default();
    while let Some(ty) = pending.pop() {
        if m.repr(ty) == Repr::Ref {
            if boxes.contains_key(&ty) {
                continue;
            }
            if let Some(b) = heap::box_layout(&mut m.core, ty) {
                pending.extend(b.field_types(ty));
                boxes.insert(ty, b);
            }
            continue;
        }
        if layouts.contains_key(&ty) || m.repr(ty) != Repr::Agg {
            continue;
        }
        let Some(layout) = emela_lir::value_layout(&mut m.core, ty) else {
            continue;
        };
        match &layout {
            Layout::Tuple(ts) => pending.extend(ts.iter().copied()),
            Layout::Enum(vs) => pending.extend(vs.iter().flatten().copied()),
            Layout::Opaque | Layout::String | Layout::Failure => {}
            Layout::Cell(t) => pending.push(*t),
        }
        layouts.insert(ty, layout);
    }
    Prepared {
        layouts,
        board_types,
        errors,
        consts: FxHashMap::default(),
        states: Vec::new(),
        string_capacity: 256,
        boxes,
        heap_bytes: 0,
    }
}

/// A C identifier from an Emela name.
fn c_ident(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// With `EMELA_EMBEDDED_DUMP` set to a directory, writes the IR of each stage there (for
/// development).
fn dump(stage: &str, m: &emela_lir::Module) {
    if let Some(dir) = std::env::var_os("EMELA_EMBEDDED_DUMP") {
        let path = std::path::Path::new(&dir).join(format!("{stage}.lir"));
        let _ = std::fs::write(path, emela_lir::dump(m));
    }
}

fn internal<E: std::fmt::Display>(stage: &str, errors: Vec<E>) -> Vec<Error> {
    let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
    vec![Error::new(format!(
        "internal error in {stage}:\n{}",
        lines.join("\n")
    ))]
}
