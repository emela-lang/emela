//! The embedded constraints, checked on the Heap IR (`docs/design/c-embedded-architecture.md`,
//! 組み込み制約の検査).
//!
//! The whole program may hold no heap value yet (④ static allocation, first stage): every
//! variable of a function the entries reach is a scalar or a value aggregate, except the evidence
//! tables and the handlers of board effects, which are static (they have no C value). Features the
//! emitter does not handle yet are reported here, with the function they are in. The functions an
//! interrupt entry reaches may not be recursive (their stack use must be bounded).

use emela_core::{Builtin, Type, TypeId};
use emela_lir::{Callee, FnId, Function, Module, Repr, RuntimeOp, Stmt, Value};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::Error;
use crate::board;

/// An entry of the program: `main` or a public function the board calls.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub function: FnId,
    /// The C name the board calls (`emela_export_button_changed`); `None` for `main`.
    pub export: Option<String>,
    /// Whether the board calls it from an interrupt (it takes a `GpioEvent`).
    pub isr: bool,
}

/// The functions the entries reach, in the order they are first reached.
/// With `heap` (`heap_bytes` > 0), heap values are allowed outside interrupts.
pub(crate) fn check(m: &Module, entries: &[Entry], heap: bool) -> Result<Vec<FnId>, Vec<Error>> {
    let mut errors = Vec::new();
    for e in entries {
        if e.isr {
            isr_shape(m, e.function, &mut errors);
        }
    }
    let reached = reach(m, entries.iter().map(|e| e.function));
    let mut c = Checker {
        m,
        errors: &mut errors,
        seen: FxHashSet::default(),
        heap_types: FxHashSet::default(),
        heap,
    };
    for &id in &reached {
        c.function(&m.functions[id]);
    }
    let isr_roots: Vec<FnId> = entries
        .iter()
        .filter(|e| e.isr)
        .map(|e| e.function)
        .collect();
    if !isr_roots.is_empty() {
        recursion(m, &isr_roots, &mut errors);
    }
    if errors.is_empty() {
        Ok(reached)
    } else {
        Err(errors)
    }
}

/// The logical pins the program names (`GpioPin(n)`) must be constants.
pub(crate) fn pins(m: &Module, reached: &[FnId]) -> Vec<Error> {
    let mut errors: Vec<Error> = Vec::new();
    for &id in reached {
        let f = &m.functions[id];
        for (_, s) in f.stmts.iter() {
            let Stmt::Let {
                value: Value::Make { ctor, args },
                ..
            } = s
            else {
                continue;
            };
            if m.core.enums[ctor.enum_id].name != "Std.Embedded.Gpio.GpioPin" {
                continue;
            }
            let constant = matches!(
                args.first(),
                Some(emela_lir::Operand::Const(emela_lir::Const::Int(_)))
            );
            let message = format!(
                "`{}` computes the ID of a GpioPin: on the embedded C target, pin IDs must be constants known at compile time",
                display(&f.name)
            );
            if !constant && !errors.iter().any(|e| e.message == message) {
                errors.push(Error::new(message));
            }
        }
    }
    errors
}

/// Whether the type is `Std.Embedded.Gpio.GpioEvent`, which makes a public function an interrupt
/// entry.
pub(crate) fn is_gpio_event(m: &Module, ty: TypeId) -> bool {
    matches!(m.core.ty(ty), Type::Enum(id, _) if m.core.enums[*id].name == "Std.Embedded.Gpio.GpioEvent")
}

/// Whether values of the type carry nothing at run time: `()` and tuples of such, which is what
/// static closure records, handlers and evidence tables without captures become (`emela_lir`'s
/// static specialization). They have no C value.
pub(crate) fn no_data(m: &Module, ty: TypeId) -> bool {
    match m.core.ty(ty) {
        Type::Unit | Type::Never => true,
        Type::Tuple(ts) => ts.iter().all(|&t| no_data(m, t)),
        _ => false,
    }
}

/// Whether the function is an operation of a board effect (an external function made by the
/// static specialization, declared in the peripheral's header).
pub(crate) fn is_board_op(f: &Function) -> bool {
    f.external.is_some() && board::split_op(&f.name).is_some()
}

/// Whether the function is an external function the static specialization made for the target
/// (board hooks and static states' accessors, named `$…`), or a board operation: the emitter
/// declares or defines them itself.
pub(crate) fn is_target_external(f: &Function) -> bool {
    f.external.is_some()
        && (f.name.starts_with('$')
            || is_board_op(f)
            || console(f).is_some()
            || f.external.as_ref().is_some_and(|e| {
                external_name(e).is_ok_and(|(name, _)| name.starts_with("emela_board_"))
            }))
}

/// For the external functions of `Std.Console` (`console_log`, `console_error`), the board hook
/// their line's pieces go to: the emitter defines them, writing the newline itself.
pub(crate) fn console(f: &Function) -> Option<&'static str> {
    let ext = f.external.as_ref()?;
    if !f.name.starts_with("Std.Console.") {
        return None;
    }
    match external_name(ext).ok()?.0.as_str() {
        "emela_console_line" => Some("emela_board_console_write"),
        "emela_console_error_line" => Some("emela_board_console_error_write"),
        _ => None,
    }
}

/// An interrupt entry must be `fn(GpioEvent) -> ()`.
fn isr_shape(m: &Module, id: FnId, errors: &mut Vec<Error>) {
    let f = &m.functions[id];
    let values: Vec<TypeId> = f
        .params
        .iter()
        .map(|&p| f.vars[p].ty)
        .filter(|&t| !no_data(m, t))
        .collect();
    let ok =
        values.len() == 1 && is_gpio_event(m, values[0]) && matches!(m.core.ty(f.ret), Type::Unit);
    if !ok {
        errors.push(Error::new(format!(
            "the interrupt entry `{}` must have the type `fn(GpioEvent) -> ()`",
            f.name
        )));
    }
}

/// The functions reachable from `roots` through direct calls and closure records.
pub(crate) fn reach(m: &Module, roots: impl IntoIterator<Item = FnId>) -> Vec<FnId> {
    reach_except(m, roots, &FxHashSet::default())
}

/// [`reach`] without looking into the bodies of `opaque` (consts evaluated at compile time).
pub(crate) fn reach_except(
    m: &Module,
    roots: impl IntoIterator<Item = FnId>,
    opaque: &FxHashSet<FnId>,
) -> Vec<FnId> {
    let mut order = Vec::new();
    let mut seen = FxHashSet::default();
    let mut stack: Vec<FnId> = roots.into_iter().collect();
    stack.reverse();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        order.push(id);
        if opaque.contains(&id) {
            continue;
        }
        let mut next = callees(&m.functions[id]);
        next.reverse();
        stack.extend(next.into_iter().filter(|g| !seen.contains(g)));
    }
    order
}

/// The functions `f` calls directly or makes closure records of, in statement order.
fn callees(f: &Function) -> Vec<FnId> {
    let mut out = Vec::new();
    for (_, s) in f.stmts.iter() {
        if let Stmt::Let { value, .. } = s {
            match value {
                Value::Call {
                    callee: Callee::Direct(g),
                    ..
                } => out.push(*g),
                Value::MakeClosure { function, .. } => out.push(*function),
                _ => {}
            }
        }
    }
    out
}

struct Checker<'a> {
    m: &'a Module,
    errors: &'a mut Vec<Error>,
    /// Messages already reported (one per function and kind of problem).
    seen: FxHashSet<String>,
    /// Heap types already reported.
    heap_types: FxHashSet<TypeId>,
    /// Whether the program has a heap.
    heap: bool,
}

impl Checker<'_> {
    fn report(&mut self, message: String) {
        if self.seen.insert(message.clone()) {
            self.errors.push(Error::new(message));
        }
    }

    fn function(&mut self, f: &Function) {
        let name = display(&f.name);
        if is_target_external(f) {
            return;
        }
        if let Some(ext) = &f.external {
            self.external(f, name, ext);
            return;
        }

        for (_, v) in f.vars.iter() {
            // A heap type is reported where it is first met (callees that take it would repeat
            // it). Function values are reported where they are made.
            let boxed = matches!(
                self.m.core.ty(v.ty),
                Type::List(_) | Type::Tuple(_) | Type::Cell(_) | Type::Enum(..)
            );
            if v.repr == Repr::Ref
                && !matches!(self.m.core.ty(v.ty), Type::Fn(_))
                && !(self.heap && boxed)
                && self.heap_types.insert(v.ty)
            {
                let ty = emela_lir::core_ty(&self.m.core, v.ty);
                self.report(if boxed {
                    format!(
                        "`{name}` uses a value on the heap ({ty}): give the program a heap with `[embedded.memory] heap_bytes` in `Pome.toml` to use it outside interrupts"
                    )
                } else {
                    format!("`{name}` uses a value of {ty}, which the embedded C target does not support")
                });
            }
        }
        for (_, s) in f.stmts.iter() {
            if let Stmt::Let { value, .. } = s {
                self.value(name, value);
            }
        }
    }

    /// An external function needs an `embedded` binding to a C function name, and types that
    /// cross into C as they are.
    fn external(&mut self, f: &Function, name: &str, ext: &emela_core::External) {
        match external_name(ext) {
            Err(problem) => self.report(format!("the external function `{name}` {problem}")),
            Ok((c_name, _)) if !crate::names::is_c_identifier(&c_name) => self.report(format!(
                "the name `{c_name}` of the external function `{name}` is not a C identifier"
            )),
            Ok(_) => {}
        }
        let tys = f.params.iter().map(|&p| f.vars[p].ty).chain([f.ret]);
        for ty in tys {
            match self.m.core.ty(ty) {
                Type::Int
                | Type::Int64
                | Type::Float
                | Type::Bool
                | Type::Unit
                | Type::Never
                | Type::String => {}
                Type::Enum(id, _) if self.m.core.enums[*id].external => {}
                Type::Fn(_) => self.report(format!(
                    "the external function `{name}` passes a function (a callback), which the embedded C target does not support"
                )),
                _ => {
                    let ty = emela_lir::core_ty(&self.m.core, ty);
                    self.report(format!(
                        "the external function `{name}` passes {ty}: external functions can pass Int, Int64, Float, Bool, String, () and `@external type`s"
                    ));
                }
            }
        }
    }

    fn value(&mut self, name: &str, value: &Value) {
        match value {
            Value::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct(_) => {}
                    // Reported where the function value is made.
                    Callee::Indirect(_) => {}
                    Callee::Op { effect, .. } => {
                        let effect = &self.m.core.effects[*effect].name;
                        if board::split(effect).is_none() {
                            self.report(format!(
                                "`{name}` uses the effect `{}`: the embedded C target supports only board effects for now",
                                display(effect)
                            ));
                        }
                    }
                }
                let _ = args;
            }
            Value::Builtin { op, .. } => {
                if !builtin_supported(*op) {
                    self.report(format!(
                        "`{name}` uses the builtin `{}`, which the embedded C target does not support yet",
                        op.info().path()
                    ));
                }
            }
            Value::Runtime { op, .. } => match op {
                RuntimeOp::Defect | RuntimeOp::DebugPrint | RuntimeOp::ShowString => {}
                _ => self.report(format!(
                    "`{name}` uses the runtime operation `{}`, which the embedded C target does not support yet",
                    op.name()
                )),
            },
            Value::Fiber { .. } => self.report(format!(
                "`{name}` uses fibers, which the embedded C target does not support"
            )),
            Value::Handler { .. } | Value::EvExtend { .. } => self.report(format!(
                "`{name}` provides a handler: the embedded C target supports only board effects for now"
            )),
            Value::MakeClosure { captures, .. } if !captures.is_empty() => self.report(format!(
                "`{name}` makes a function value capturing variables: the embedded C target supports only functions without captures passed directly as arguments"
            )),
            Value::MakeClosure { .. } => self.report(format!(
                "`{name}` uses a function value other than by passing it directly as an argument, which is all the embedded C target supports"
            )),
            _ => {}
        }
    }
}

/// The C name of an external function and the file that defines it (if it names one), from its
/// `embedded` binding; or why it has none.
pub(crate) fn external_name(
    ext: &emela_core::External,
) -> Result<(String, Option<emela_core::HostModule>), &'static str> {
    use emela_core::{HostModule, HostRef};
    let Some(binding) = ext.binding("embedded") else {
        return Err("has no `@external(embedded, ...)`");
    };
    match &binding.host {
        HostRef::Global(name) => Ok((name.clone(), None)),
        HostRef::Import {
            module: module @ (HostModule::File(_) | HostModule::Bundled(_)),
            name,
        } => Ok((name.clone(), Some(module.clone()))),
        HostRef::Import {
            module: HostModule::Named(_),
            ..
        } => Err("must point to a C file (`./sensor.c`) or name a C function"),
    }
}

/// The builtins the emitter implements.
pub(crate) fn builtin_supported(op: Builtin) -> bool {
    matches!(
        op,
        Builtin::IntBitAnd
            | Builtin::IntBitOr
            | Builtin::IntBitXor
            | Builtin::IntBitNot
            | Builtin::IntShiftLeft
            | Builtin::IntShiftRight
            | Builtin::IntShiftRightUnsigned
            | Builtin::Int64FromInt
            | Builtin::Int64ToInt
            | Builtin::Int64BitAnd
            | Builtin::Int64BitOr
            | Builtin::Int64BitXor
            | Builtin::Int64BitNot
            | Builtin::Int64ShiftLeft
            | Builtin::Int64ShiftRight
            | Builtin::Int64ShiftRightUnsigned
            | Builtin::FloatFromInt
            | Builtin::FloatFloor
            | Builtin::FloatCeil
            | Builtin::FloatRound
            | Builtin::FloatTruncate
            | Builtin::CellNew
            | Builtin::CellGet
            | Builtin::CellSet
            | Builtin::StringLength
            | Builtin::StringByteSize
            | Builtin::StringAppend
            | Builtin::StringContains
            | Builtin::StringStartsWith
            | Builtin::StringEndsWith
            | Builtin::StringTrim
            | Builtin::StringSplit
            | Builtin::StringChars
            | Builtin::StringJoin
            | Builtin::StringCodePoints
            | Builtin::StringFromCodePoint
            | Builtin::IntToString
            | Builtin::Int64ToString
            | Builtin::FloatToString
            | Builtin::IntParse
            | Builtin::FloatParse
            | Builtin::IntCheckedAdd
    )
}

/// Reports the functions on a cycle of direct calls reachable from an interrupt entry.
fn recursion(m: &Module, isr_roots: &[FnId], errors: &mut Vec<Error>) {
    let reached = reach(m, isr_roots.iter().copied());
    for &id in &reached {
        let f = &m.functions[id];
        if let Some((_, v)) = f.vars.iter().find(|(_, v)| v.repr == Repr::Ref) {
            let ty = emela_lir::core_ty(&m.core, v.ty);
            errors.push(Error::new(format!(
                "`{}`, reached from an interrupt entry, uses a value on the heap ({ty}): interrupts cannot use the heap",
                display(&f.name)
            )));
        }
    }
    let edges: FxHashMap<FnId, Vec<FnId>> = reached
        .iter()
        .map(|&id| (id, callees(&m.functions[id])))
        .collect();
    let mut recursive: Vec<FnId> = reached
        .iter()
        .copied()
        .filter(|&id| on_cycle(id, &edges))
        .collect();
    recursive.sort_by_key(|&id| reached.iter().position(|&r| r == id));
    for &id in &reached {
        let f = &m.functions[id];
        if f.external.is_some() && f.suspends {
            errors.push(Error::new(format!(
                "an interrupt entry reaches `{}`, an external function that waits for completion (a suspend fn): interrupts cannot wait",
                display(&f.name)
            )));
        }
    }
    for id in recursive {
        errors.push(Error::new(format!(
            "`{}`, reached from an interrupt entry, is recursive: the stack use in an interrupt would be unbounded",
            display(&m.functions[id].name)
        )));
    }
}

/// Whether `id` reaches itself.
fn on_cycle(id: FnId, edges: &FxHashMap<FnId, Vec<FnId>>) -> bool {
    let mut seen = FxHashSet::default();
    let mut stack: Vec<FnId> = edges.get(&id).cloned().unwrap_or_default();
    while let Some(g) = stack.pop() {
        if g == id {
            return true;
        }
        if seen.insert(g) {
            stack.extend(edges.get(&g).into_iter().flatten().copied());
        }
    }
    false
}

/// A function name for messages: without the type arguments and specializations the compiler
/// adds (`Option.map[Int, Int, {}, {}]$inc` is `Option.map`).
pub(crate) fn display(name: &str) -> &str {
    let end = name.find(['[', '$']).unwrap_or(name.len());
    &name[..end]
}
