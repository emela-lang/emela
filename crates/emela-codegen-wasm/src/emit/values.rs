//! Records and tuples that the WASM backend may hold by value in WASM locals (E1, steps 1–5).
//!
//! A record (`type Point(x: Int, y: Int)`, an enum with exactly one variant) or a tuple whose
//! leaves are all `i32`, `i64` or `f64` can be represented as a **span of WASM locals**: one local
//! per scalar leaf of its layout, in field order. A nested record or tuple expands in place, so
//! the leaves are the slots a box of the value would have (`8` bytes each). The WASM Heap IR is
//! unchanged: these values still have [`Repr::Ref`] there; only the emitter's local layout differs
//! (the C backend's `layout.rs` makes the same decision with C structs).
//!
//! # Escape boundaries
//!
//! A value held in locals is boxed where a reference is stored and unboxed where one is read
//! back (E1 step 5): a list head, an enum payload field, a closure capture and environment slot,
//! `Any`, and an array element. A box is an ordinary heap object whose slots are the value's
//! scalar leaves, with a descriptor whose `refs` are all false (the C backend's
//! `em_box_value`/`layout_desc`). The box is a reference owned by the containing list, enum,
//! closure or array; the local copy of the value owns nothing, so `dup`/`drop` stay no-ops for it.
//! A read of an inline all-scalar array element whose destination is held by value needs no box
//! at all: `ArrayGetOr`/`ArrayLoadUnchecked` load its leaves directly (E2 step 3).
//!
//! # What is never held by value here
//!
//! A type keeps `Repr::Ref` when the module uses it in a way that cannot be boxed soundly:
//!
//! - a parameter or result of a function whose wasm signature is reached through another calling
//!   convention (an exported, external or const function, a suspending function, the entry, an
//!   error `show` function), or an argument or result of a suspending effect operation;
//! - an element of `Cell` (its content is one word and is excluded with the type it contains);
//! - an argument of a builtin or a runtime operation (except `Array.set`, whose value is boxed),
//!   and a function value passed to a fiber operation (its thunk passes the result as one word,
//!   selected by representation);
//! - the element type of an array whose module uses `Array.map_range`, whose loop calls the
//!   closure with one `em_word` result;
//! - any variable of a suspending function, or any variable saved across a suspension.
//!
//! A type that is excluded, or that contains an excluded field, has no layout, and the emitter
//! keeps its boxed representation everywhere. The exclusion is a whole-module property: a type is
//! held by value only if every use in the whole program is one the converted slices handle.
//!
//! # The call ABI
//!
//! Every call of a function type uses one signature. A function whose wasm signature no other
//! calling convention sees (not exported, external, a const, suspending, the entry or an error
//! `show`) follows the layouts: an `Agg` parameter is its leaves, and an `Agg` result is zero or
//! several wasm results. A function used as a value (`MakeClosure`) follows the same rule, so a
//! direct call and a `call_indirect` of its type agree. A lifted closure keeps its environment as
//! one reference parameter; only its other parameters and its result may flatten.
//!
//! A non-suspending effect operation also uses the layouts of its parameters and result, because
//! its implementation is a closure of the same function type. A suspending operation, a
//! suspending function and everything a suspension frame saves stay boxed in this step: the
//! trampoline returns one `em_word`-shaped result, selected by representation.
//! No proposal is needed: `[target.unspecified]` leaves the memory representation of values to
//! the implementation, and this changes only how many objects are allocated.
//!
//! # The layout
//!
//! Unlike C, padding after a 32-bit field is not a leaf: every WASM local has its own type, so
//! `Mixed(n: Int, f: Float)` is two locals (`i32`, `f64`) rather than a padded struct. The number
//! of leaves is still the number of 8-byte slots a box of the value has, which is what the escape
//! boundaries store (E1 step 5).

use emela_core::{Builtin, Type, TypeId};
use emela_lir::{Callee, Const, FnId, Function, Module, Operand, Repr, Stmt, Value, repr_of};
use rustc_hash::{FxHashMap, FxHashSet};

/// The scalar leaves of a value, in field order.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    /// The record, tuple or variant name (for diagnostics and tests).
    pub(crate) name: String,
    /// The flattened leaves. A nested value expands in place.
    pub(crate) leaves: Vec<Repr>,
    /// The leaf range of each field, in field-definition order.
    pub(crate) fields: Vec<FieldSpan>,
}

impl Layout {
    /// The number of 8-byte slots the value would occupy in a box: one per leaf.
    pub(crate) fn slots(&self) -> usize {
        self.leaves.len()
    }

    /// The first leaf and the number of leaves of field `index`.
    pub(crate) fn field(&self, index: usize) -> FieldSpan {
        self.fields[index]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FieldSpan {
    pub(crate) start: usize,
    pub(crate) len: usize,
}

/// The types of a module that can be held by value, with their layouts.
#[derive(Debug)]
pub(crate) struct Values<'m> {
    core: &'m emela_core::Module,
    layouts: Vec<Layout>,
    /// The layout of each type that was asked for (`None`: held as it was).
    of: FxHashMap<TypeId, Option<usize>>,
    /// The types the module uses in a way that this local-only slice cannot convert.
    excluded: FxHashSet<TypeId>,
    /// The types whose layout is being built, to stop at a type that would contain itself.
    building: FxHashSet<TypeId>,
}

impl<'m> Values<'m> {
    /// The layouts of `m`, for the types the whole module uses in a way the emitter converts.
    ///
    /// `roots` are the functions the entry calls directly before any IR body (the monomorphized
    /// `main`, or the test functions): their signatures are not flattened, because the entry
    /// thunk calls them with the boxed form.
    pub(crate) fn new(m: &'m Module, roots: &[FnId]) -> Values<'m> {
        let mut values = Values {
            core: &m.core,
            layouts: Vec::new(),
            of: FxHashMap::default(),
            excluded: excluded(m, roots),
            building: FxHashSet::default(),
        };
        // The variables and the results are every type the emitter has to name.
        for (_, f) in m.functions.iter() {
            for (_, var) in f.vars.iter() {
                values.layout(var.ty);
            }
            values.layout(f.ret);
        }
        values
    }

    /// No layouts: every value keeps its boxed representation.
    pub(crate) fn none(m: &'m Module) -> Values<'m> {
        Values {
            core: &m.core,
            layouts: Vec::new(),
            of: FxHashMap::default(),
            excluded: FxHashSet::default(),
            building: FxHashSet::default(),
        }
    }

    /// The index of the layout of `ty`, if it is held by value.
    pub(crate) fn index(&self, ty: TypeId) -> Option<usize> {
        self.of.get(&ty).copied().flatten()
    }

    pub(crate) fn get(&self, index: usize) -> &Layout {
        &self.layouts[index]
    }

    /// Whether any type of the module is held by value. A function with no such type needs no
    /// scratch local for a box.
    pub(crate) fn any(&self) -> bool {
        !self.layouts.is_empty()
    }

    /// The layout of `ty`, built on first use. It has none if the module uses it in an
    /// unconvertible way, or if a field has no scalar form.
    fn layout(&mut self, ty: TypeId) -> Option<usize> {
        if let Some(&known) = self.of.get(&ty) {
            return known;
        }
        if self.excluded.contains(&ty) || self.building.contains(&ty) {
            self.of.insert(ty, None);
            return None;
        }
        self.building.insert(ty);
        let layout = self.build(ty);
        self.building.remove(&ty);
        let index = layout.map(|l| {
            self.layouts.push(l);
            self.layouts.len() - 1
        });
        self.of.insert(ty, index);
        index
    }

    /// The layout of `ty`, or `None` if it is not a record or tuple that can be held by value.
    fn build(&mut self, ty: TypeId) -> Option<Layout> {
        let core = self.core;
        build_layout(core, ty, true, |field| {
            let index = self.layout(field)?;
            Some(self.layouts[index].leaves.clone())
        })
    }
}

/// The all-scalar layout of `ty` as an array element that can be stored inline (E2), ignoring
/// how the module uses it.
///
/// This is the pure part of [`Values`]'s predicate: a non-empty tuple or a single-variant,
/// non-error, non-external enum whose fields are scalars. It does not apply the local-only
/// exclusions (a function parameter, an array element and so on), so an array of the type can
/// store it inline even though the local-only slice would keep it boxed (E2).
///
/// A field of reference representation is rejected, directly or through a nested record. An
/// inline element is copied slot for slot and has no per-field reference map, so it holds only
/// scalar leaves. A nested record stays a box of its own: the array stores a reference to it (E1
/// step 5), or an inline element of several leaves once E2's element layout is widened (a later
/// step).
pub(crate) fn all_scalar(core: &emela_core::Module, ty: TypeId) -> Option<Layout> {
    build_layout(core, ty, false, |_| None)
}

/// The name and field types of a candidate aggregate, resolving a field's type parameter.
fn fields_of(core: &emela_core::Module, ty: TypeId) -> Option<(String, Vec<TypeId>)> {
    match core.ty(ty) {
        Type::Tuple(elems) => Some(("tuple".to_owned(), elems.clone())),
        Type::Enum(id, args) => {
            let def = &core.enums[*id];
            // A record: one variant with fields. A multi-variant enum needs a tag, which this
            // prototype does not give a value form; an error must be distinguishable by pointer,
            // and an external type is a host handle.
            if def.error || def.external || def.variants.len() != 1 {
                return None;
            }
            let variant = &def.variants[0];
            if variant.tys.is_empty() {
                return None;
            }
            let resolved: Vec<TypeId> = variant
                .tys
                .iter()
                .map(|&t| match core.ty(t) {
                    Type::Param(i) if *i < args.len() => args[*i],
                    _ => t,
                })
                .collect();
            Some((variant.name.clone(), resolved))
        }
        _ => None,
    }
}

/// Builds the layout of a candidate aggregate, flattening a nested record or tuple with `nested`.
fn build_layout(
    core: &emela_core::Module,
    ty: TypeId,
    allow_nested: bool,
    mut nested: impl FnMut(TypeId) -> Option<Vec<Repr>>,
) -> Option<Layout> {
    let (name, fields) = fields_of(core, ty)?;
    let mut leaves = Vec::new();
    let mut spans = Vec::new();
    for field in fields {
        let start = leaves.len();
        match repr_of(core, field) {
            Repr::I32 => leaves.push(Repr::I32),
            Repr::I64 => leaves.push(Repr::I64),
            Repr::F64 => leaves.push(Repr::F64),
            // A nested record or tuple, held inline. Anything else is a reference, which has no
            // place in a value. An array element rejects every reference field (see
            // [`all_scalar`]).
            Repr::Ref => {
                if !allow_nested {
                    return None;
                }
                leaves.extend_from_slice(&nested(field)?);
            }
            Repr::Void => return None,
        }
        spans.push(FieldSpan {
            start,
            len: leaves.len() - start,
        });
    }
    if leaves.is_empty() {
        return None;
    }
    Some(Layout {
        name,
        leaves,
        fields: spans,
    })
}

/// Adds `ty` and every type argument reachable from it to `out`. Used where a value is stored in a
/// position that cannot box it (a cell's content, a boxed function signature, a suspension frame,
/// a fiber thunk), so that nothing nested inside it can be held by value either.
fn mark_subtree(core: &emela_core::Module, ty: TypeId, out: &mut FxHashSet<TypeId>) {
    if !out.insert(ty) {
        return;
    }
    match core.ty(ty) {
        Type::Tuple(ts) => {
            for &t in ts {
                mark_subtree(core, t, out);
            }
        }
        Type::Enum(_, args) => {
            for &t in args {
                mark_subtree(core, t, out);
            }
        }
        Type::Array(t) | Type::List(t) | Type::Cell(t) => mark_subtree(core, *t, out),
        Type::Fn(f) => {
            for &t in &f.params {
                mark_subtree(core, t, out);
            }
            mark_subtree(core, f.ret, out);
        }
        Type::Set(s) => {
            for &t in &s.params {
                mark_subtree(core, t, out);
            }
        }
        _ => {}
    }
}

/// Whether the wasm signature of `f` follows the layouts of its parameters and result.
///
/// The signature is flattened only when no other calling convention can see it: the function is
/// not exported, external, a const, suspending, a root the entry thunk calls, or an error's `show`
/// function (the descriptor calls it through the table). Otherwise its parameters and result keep
/// the boxed form, so the entry, the external hosts, the state machines, the fiber thunks and the
/// table need no change.
///
/// A function used as a value is not excluded: an indirect call now uses the same layouts as a
/// direct call, so `call_indirect`, a handler operation and every direct call of the type agree.
/// A lifted closure's environment parameter stays one reference (its type is not a value).
fn flat_function(m: &Module, id: FnId, f: &Function, roots: &FxHashSet<FnId>) -> bool {
    !f.exported
        && f.external.is_none()
        && !f.constant
        && !f.suspends
        && !roots.contains(&id)
        && !m.error_shows.iter().any(|&(_, show)| show == id)
        && !f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    on_suspend: Some(_),
                    ..
                }
            )
        })
}

/// The types of `m` that the converted slices cannot hold by value.
///
/// The first pass marks the positions that always need a reference. The loop repeats until it is
/// stable: a direct call whose parameter type has no layout keeps its argument boxed.
fn excluded(m: &Module, roots: &[FnId]) -> FxHashSet<TypeId> {
    let core = &m.core;
    let mut out = FxHashSet::default();
    let roots: FxHashSet<FnId> = roots.iter().copied().collect();
    let flat = |id: FnId| flat_function(m, id, &m.functions[id], &roots);

    // A cell stores its content as one word. It cannot box a value (the content is excluded with
    // the type it contains), so `Cell` keeps the boxed form.
    for ty in core.types.iter() {
        if let Type::Cell(elem) = ty {
            mark_subtree(core, *elem, &mut out);
        }
    }

    // A suspending operation returns through the trampoline, which carries one word selected by
    // representation, and its implementation may be a state machine. Keep its parameters and
    // result boxed in this step. A non-suspending operation uses the same layouts as its
    // implementation closure, so its parameters and result may flatten.
    for (_, effect) in core.effects.iter() {
        for op in &effect.ops {
            if !op.suspend {
                continue;
            }
            for &t in &op.params {
                mark_subtree(core, t, &mut out);
            }
            mark_subtree(core, op.ret, &mut out);
        }
    }

    for (id, f) in m.functions.iter() {
        // A parameter or result of a function whose signature is not flattened is part of a
        // boxed wasm signature, and its call sites pass a reference. A signature that is
        // flattened keeps the layout of each parameter and the result (E1 step 3); a parameter
        // whose own type is excluded still crosses the flattened signature as a reference.
        if !flat(id) {
            for &p in &f.params {
                out.insert(f.vars[p].ty);
            }
            out.insert(f.ret);
        }
        // A state machine stores every live variable in a word-shaped frame; keep all of a
        // suspending function's types boxed in this slice.
        if f.suspends
            || f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        on_suspend: Some(_),
                        ..
                    }
                )
            })
        {
            for (_, var) in f.vars.iter() {
                out.insert(var.ty);
            }
        }
        for (_, stmt) in f.stmts.iter() {
            let Stmt::Let {
                dst,
                value,
                on_suspend,
                ..
            } = stmt
            else {
                continue;
            };
            if let Some(edge) = on_suspend {
                for &v in &edge.saved {
                    out.insert(f.vars[v].ty);
                }
            }
            // A call returns its leaves when the callee's signature follows the layouts, so its
            // destination is buildable in place like a construction. A value read out of a box
            // (`Field`, `ListHead`, `EnvGet`, `Coerce`, an array element) is unboxed into locals
            // (E1 step 5). A construction whose destination has no layout stores each field in a
            // box (an `Agg` field is itself boxed). Everything else (a builtin, a runtime
            // operation, a fiber operation) needs one reference, so the type stays boxed.
            let call = matches!(value, Value::Call { .. });
            let direct = call
                || matches!(
                    value,
                    Value::Make { .. }
                        | Value::Tuple(_)
                        | Value::Field { .. }
                        | Value::TupleGet { .. }
                        | Value::ListHead(_)
                        | Value::EnvGet { .. }
                        | Value::Coerce(_)
                        | Value::Builtin {
                            op: Builtin::ArrayGetOr | Builtin::ArrayLoadUnchecked,
                            ..
                        }
                        // A unit constant bound to a record-typed variable is test scaffolding
                        // (the Core IR never produces one: `()` has no representation).
                        | Value::Const(Const::Unit)
                );
            if let (Some(d), false) = (dst, direct) {
                out.insert(f.vars[*d].ty);
            }
            // Operands that cross into a reference-shaped position.
            let mut mark = |o: &Operand| {
                if let Operand::Var(v) = o {
                    out.insert(f.vars[*v].ty);
                }
            };
            match value {
                // A primitive takes words and cannot read a value held in locals. A builtin takes
                // references or words and the runtime duplicates what it keeps.
                Value::Prim { args, .. } => {
                    for a in args {
                        mark(a);
                    }
                }
                // `Array.set` consumes the array and borrows the index and value; `Array.GetOr`
                // takes the array, the index and a default value of the element type. The emitter
                // boxes a value held in locals (E1 step 5) or copies its leaves, so the element
                // type may stay by value.
                Value::Builtin {
                    op: Builtin::ArraySet | Builtin::ArrayGetOr,
                    args,
                } => {
                    for a in &args[..2.min(args.len())] {
                        mark(a);
                    }
                }
                // The `map_range` loop calls the closure through a one-word result, so the
                // element type keeps the boxed form (E2's inline form copies a box).
                Value::Builtin {
                    op: Builtin::ArrayMapRange,
                    ..
                } => {
                    if let Some(d) = dst
                        && let Type::Array(elem) = core.ty(f.vars[*d].ty)
                    {
                        mark_subtree(core, *elem, &mut out);
                    }
                }
                Value::Builtin { args, .. } => {
                    for a in args {
                        mark(a);
                    }
                }
                // A fiber operation's thunk passes the result as one word selected by
                // representation, and `Attempt` calls the closure with one result. Exclude the
                // whole function value, including its parameters and result.
                Value::Fiber { args, .. } | Value::Runtime { args, .. } => {
                    for a in args {
                        if let Operand::Var(v) = a {
                            mark_subtree(core, f.vars[*v].ty, &mut out);
                        }
                    }
                }
                // A direct call uses the callee's layout-derived signature. An indirect or
                // operation call uses the layout of each parameter, exactly as the closure it
                // enters does. A parameter that has no layout is already boxed, so nothing needs
                // to be marked here; a direct call whose signature stays boxed has its parameter
                // types excluded above (the function is not flat).
                Value::Call { callee, args, .. } => {
                    let flattened = matches!(callee, Callee::Direct(g) if flat(*g))
                        || matches!(callee, Callee::Indirect(_) | Callee::Op { .. });
                    if !flattened {
                        for a in args {
                            mark(a);
                        }
                    }
                }
                // A list head, a capture, a field of a boxed construction and `Any` are boxed at
                // the boundary (E1 step 5), so their element types may stay by value.
                Value::List { .. }
                | Value::MakeClosure { .. }
                | Value::Coerce(_)
                | Value::Make { .. }
                | Value::Tuple(_) => {}
                Value::Handler { ops, .. } => {
                    for &v in ops {
                        out.insert(f.vars[v].ty);
                    }
                }
                Value::IsError { error, .. } | Value::AsError { error, .. } => {
                    out.insert(f.vars[*error].ty);
                }
                Value::ListIsEmpty(v) | Value::ListHead(v) | Value::ListTail(v) => {
                    out.insert(f.vars[*v].ty);
                }
                Value::EvLookup { ev, .. } => {
                    out.insert(f.vars[*ev].ty);
                }
                Value::EvExtend { base, bindings } => {
                    if let Some(b) = base {
                        out.insert(f.vars[*b].ty);
                    }
                    for &(_, v) in bindings {
                        out.insert(f.vars[v].ty);
                    }
                }
                Value::EnvGet { env, .. } => {
                    out.insert(f.vars[*env].ty);
                }
                // A `Field`/`TupleGet` reads a base; a tag and a constant have no operands.
                Value::Field { .. } | Value::Tag(_) | Value::TupleGet { .. } => {}
                Value::Const(_) | Value::FnRef(_) | Value::Closure(_) => {}
            }
        }
    }

    // A flattened call passes a parameter the boxed form when the layout of the parameter type
    // is not available; a call's argument has the parameter's type, so this only completes a
    // type the signature already keeps boxed. Repeat, because excluding it can make another
    // parameter unbuildable.
    loop {
        let mut extra = FxHashSet::default();
        let mut probe = Values {
            core,
            layouts: Vec::new(),
            of: FxHashMap::default(),
            excluded: out.clone(),
            building: FxHashSet::default(),
        };
        for (_, f) in m.functions.iter() {
            for (_, stmt) in f.stmts.iter() {
                let Stmt::Let { value, .. } = stmt else {
                    continue;
                };
                if let Value::Call {
                    callee: Callee::Direct(g),
                    args,
                    ..
                } = value
                    && flat(*g)
                {
                    let callee = &m.functions[*g];
                    for (a, &p) in args.iter().zip(&callee.params) {
                        if probe.layout(callee.vars[p].ty).is_none()
                            && let Operand::Var(v) = a
                        {
                            extra.insert(f.vars[*v].ty);
                        }
                    }
                }
            }
        }
        let before = out.len();
        out.extend(extra);
        if out.len() == before {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_core::{EnumDef, VariantDef};
    use emela_lir::{Const, FnBuilder, Term, Value};

    /// The layouts of a module with one function, whose body `build` writes. The module is
    /// leaked, so that the layouts can borrow it for the length of a test.
    fn values_of(
        core: emela_core::Module,
        build: impl FnOnce(&mut FnBuilder<'_>),
    ) -> Values<'static> {
        let mut m = Module::new(core);
        let mut b = FnBuilder::new(&m.core, "f");
        build(&mut b);
        b.term(Term::Return(None));
        let f = b.finish();
        m.add(f);
        let m: &'static Module = Box::leak(Box::new(m));
        Values::new(m, &[])
    }

    /// A module with one function holding a value of each type `make` returns.
    fn values_with(
        make: impl FnOnce(&mut emela_core::Module) -> Vec<TypeId>,
    ) -> (Values<'static>, Vec<TypeId>) {
        let mut core = emela_core::Module::new();
        let types = make(&mut core);
        let values = values_of(core, |b| {
            for &ty in &types {
                b.assign("v", ty, Value::Const(Const::Unit));
            }
        });
        (values, types)
    }

    fn record(core: &mut emela_core::Module, name: &str, fields: Vec<(&str, TypeId)>) -> TypeId {
        let def = EnumDef {
            name: name.to_owned(),
            params: 0,
            variants: vec![VariantDef::named(name, fields)],
            error: false,
            external: false,
        };
        let id = core.add_enum(def);
        core.types.enum_type(id, vec![])
    }

    #[test]
    fn a_record_of_floats_is_held_by_value() {
        let (values, types) = values_with(|core| {
            vec![record(
                core,
                "Body",
                vec![
                    ("x", TypeId::FLOAT),
                    ("y", TypeId::FLOAT),
                    ("mass", TypeId::FLOAT),
                ],
            )]
        });
        let i = values.index(types[0]).expect("a value");
        let l = values.get(i);
        assert_eq!(l.name, "Body");
        assert_eq!(l.leaves, [Repr::F64, Repr::F64, Repr::F64]);
        assert_eq!(l.slots(), 3);
        assert_eq!(l.field(2), FieldSpan { start: 2, len: 1 });
    }

    #[test]
    fn a_32_bit_field_is_its_own_leaf() {
        // Unlike C, the padding after an `Int` is not a leaf: a wasm local has its own type.
        let (values, types) = values_with(|core| {
            vec![record(
                core,
                "Mixed",
                vec![("n", TypeId::INT), ("f", TypeId::FLOAT)],
            )]
        });
        let l = values.get(values.index(types[0]).expect("a value"));
        assert_eq!(l.leaves, [Repr::I32, Repr::F64]);
        assert_eq!(l.slots(), 2);
        assert_eq!(l.field(0), FieldSpan { start: 0, len: 1 });
        assert_eq!(l.field(1), FieldSpan { start: 1, len: 1 });
    }

    #[test]
    fn a_nested_record_is_held_inline() {
        let (values, types) = values_with(|core| {
            let inner = record(core, "Point", vec![("x", TypeId::INT), ("y", TypeId::INT)]);
            vec![record(
                core,
                "Pair",
                vec![("left", inner), ("right", inner), ("n", TypeId::INT)],
            )]
        });
        let l = values.get(values.index(types[0]).expect("a value"));
        assert_eq!(
            l.leaves,
            [Repr::I32, Repr::I32, Repr::I32, Repr::I32, Repr::I32]
        );
        assert_eq!(l.slots(), 5, "two two-leaf points and one Int");
        assert_eq!(l.field(0), FieldSpan { start: 0, len: 2 });
        assert_eq!(l.field(1), FieldSpan { start: 2, len: 2 });
        assert_eq!(l.field(2), FieldSpan { start: 4, len: 1 });
    }

    #[test]
    fn a_tuple_is_held_by_value() {
        let (values, types) =
            values_with(|core| vec![core.tuple(vec![TypeId::INT, TypeId::FLOAT])]);
        let l = values.get(values.index(types[0]).expect("a value"));
        assert_eq!(l.name, "tuple");
        assert_eq!(l.leaves, [Repr::I32, Repr::F64]);
    }

    #[test]
    fn a_field_that_is_a_reference_keeps_the_box() {
        let (values, types) =
            values_with(|core| vec![record(core, "Named", vec![("name", TypeId::STRING)])]);
        assert_eq!(values.index(types[0]), None);
    }

    #[test]
    fn an_enum_with_several_variants_keeps_the_box() {
        let (values, types) = values_with(|core| {
            let def = EnumDef {
                name: "Shape".to_owned(),
                params: 0,
                variants: vec![
                    VariantDef::positional("Circle", vec![TypeId::FLOAT]),
                    VariantDef::unit("Empty"),
                ],
                error: false,
                external: false,
            };
            let id = core.add_enum(def);
            vec![core.types.enum_type(id, vec![])]
        });
        assert_eq!(values.index(types[0]), None);
    }

    #[test]
    fn a_value_in_a_cell_keeps_the_box() {
        // A cell stores its content as a word, so the whole program holds the record boxed.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let cell = core.types.cell(rec);
        let values = values_of(core, |b| {
            let p = b.assign("p", rec, Value::Const(Const::Unit));
            b.assign(
                "c",
                cell,
                Value::Builtin {
                    op: emela_core::Builtin::CellNew,
                    args: vec![Operand::Var(p)],
                },
            );
        });
        assert_eq!(values.index(rec), None);
    }

    #[test]
    fn a_value_in_a_list_is_held_by_value() {
        // A list cell's head is boxed at the boundary, so the record is still a value (E1
        // step 5).
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let list = core.types.list(rec);
        let values = values_of(core, |b| {
            let p = b.assign("p", rec, Value::Const(Const::Unit));
            b.assign(
                "xs",
                list,
                Value::List {
                    elems: vec![Operand::Var(p)],
                    tail: None,
                },
            );
        });
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn a_value_in_an_enum_payload_is_held_by_value() {
        // A field of a boxed enum is boxed; a read of it into a value is unboxed.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let def = EnumDef {
            name: "Maybe".to_owned(),
            params: 0,
            variants: vec![
                VariantDef::positional("Some", vec![rec]),
                VariantDef::unit("None"),
            ],
            error: false,
            external: false,
        };
        let id = core.add_enum(def);
        let maybe = core.types.enum_type(id, vec![]);
        let values = values_of(core, |b| {
            let p = b.assign("p", rec, Value::Const(Const::Unit));
            let m = b.assign(
                "m",
                maybe,
                Value::Make {
                    ctor: emela_core::CtorRef {
                        enum_id: id,
                        variant: 0,
                    },
                    args: vec![Operand::Var(p)],
                },
            );
            b.assign(
                "q",
                rec,
                Value::Field {
                    base: m,
                    ctor: emela_core::CtorRef {
                        enum_id: id,
                        variant: 0,
                    },
                    index: 0,
                },
            );
        });
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn a_value_captured_by_a_closure_is_held_by_value() {
        // A capture is a closure slot; the capture is boxed at construction and unboxed by
        // `env_get` (E1 step 5).
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let fty = core.fn_type(Vec::new(), TypeId::INT);
        let mut m = Module::new(core);
        let mut g = FnBuilder::new(&m.core, "g");
        g.term(Term::Return(None));
        let g = m.add(g.finish());
        let mut f = FnBuilder::new(&m.core, "f");
        let p = f.assign("p", rec, Value::Const(Const::Unit));
        f.assign(
            "c",
            fty,
            Value::MakeClosure {
                function: g,
                captures: vec![p],
            },
        );
        f.term(Term::Return(None));
        m.add(f.finish());
        let m: &'static Module = Box::leak(Box::new(m));
        let values = Values::new(m, &[]);
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn a_value_crossing_any_is_held_by_value() {
        // `Any` boxes the value into a one-slot reference; a conversion back unboxes it (E1
        // step 5).
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let values = values_of(core, |b| {
            let p = b.assign("p", rec, Value::Const(Const::Unit));
            b.assign("a", TypeId::ANY, Value::Coerce(p));
        });
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn a_value_round_trips_through_any_in_the_emitter() {
        // A value boxed for `Any` and unboxed back: the emitter's `Coerce` path in both
        // directions (E1 step 5). The source language has no value of `Any`, so this exercises
        // the lowering with a hand-built module.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let ctor = emela_core::CtorRef {
            enum_id: match core.ty(rec) {
                Type::Enum(id, _) => *id,
                _ => unreachable!(),
            },
            variant: 0,
        };
        let mut m = Module::new(core);
        let mut f = FnBuilder::new(&m.core, "main");
        let p = f.assign(
            "p",
            rec,
            Value::Make {
                ctor,
                args: vec![Operand::Const(Const::Int(3))],
            },
        );
        let a = f.assign("a", TypeId::ANY, Value::Coerce(p));
        let q = f.assign("q", rec, Value::Coerce(a));
        f.assign(
            "n",
            TypeId::INT,
            Value::Field {
                base: q,
                ctor,
                index: 0,
            },
        );
        f.term(Term::Return(None));
        let main = m.add(f.finish());
        emela_lir::to_heap_with_options(&mut m, false, emela_lir::opt::Options::default());
        if let Err(errors) = emela_lir::verify(&m) {
            panic!("{errors:?}");
        }
        let out = crate::emit::emit(
            &m,
            &crate::emit::Run::Main(Some(main)),
            crate::WasmOptions {
                values: true,
                ..crate::WasmOptions::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        wasmparser::validate(&out.module).expect("the module is valid");
        let boxed = crate::emit::emit(
            &m,
            &crate::emit::Run::Main(Some(main)),
            crate::WasmOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        wasmparser::validate(&boxed.module).expect("the boxed module is valid");
        assert_ne!(
            out.module, boxed.module,
            "the value must cross `Any` as a box of its leaves"
        );
    }

    #[test]
    fn a_value_in_an_array_is_held_by_value() {
        // An array element is boxed per element until E2's inline form (E1 step 5).
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let array = core.types.intern(Type::Array(rec));
        let values = values_of(core, |b| {
            b.assign("p", rec, Value::Const(Const::Unit));
            b.assign("xs", array, Value::Const(Const::Unit));
        });
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn a_value_mapped_into_an_array_keeps_the_box() {
        // The `map_range` loop calls the closure through a one-word result, so the element type
        // keeps the boxed form.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let array = core.types.intern(Type::Array(rec));
        let values = values_of(core, |b| {
            let c = b.assign("c", TypeId::ANY, Value::Const(Const::Unit));
            b.assign("p", rec, Value::Const(Const::Unit));
            b.assign(
                "xs",
                array,
                Value::Builtin {
                    op: emela_core::Builtin::ArrayMapRange,
                    args: vec![
                        Operand::Const(Const::Int(0)),
                        Operand::Const(Const::Int(3)),
                        Operand::Var(c),
                    ],
                },
            );
        });
        assert_eq!(values.index(rec), None);
    }

    #[test]
    fn a_direct_function_result_is_held_by_value() {
        // The function's value is never taken, so its wasm signature is flattened and the
        // result is a span (E1 step 3).
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let values = values_of(core, |b| {
            b.set_ret(rec);
            b.assign("p", rec, Value::Const(Const::Unit));
        });
        assert_eq!(
            values.get(values.index(rec).expect("a value")).leaves,
            [Repr::I32]
        );
    }

    #[test]
    fn a_direct_function_parameter_is_held_by_value() {
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let values = values_of(core, |b| {
            b.param("p", rec);
        });
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn an_argument_of_a_direct_call_is_held_by_value() {
        // The record crosses the flattened signature as its leaf, not as a reference.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let mut m = Module::new(core);
        let mut g = FnBuilder::new(&m.core, "g");
        g.param("p", rec);
        g.term(Term::Return(None));
        let g = m.add(g.finish());
        let mut f = FnBuilder::new(&m.core, "f");
        let p = f.assign("p", rec, Value::Const(Const::Unit));
        f.let_(
            None,
            Value::Call {
                callee: Callee::Direct(g),
                args: vec![Operand::Var(p)],
                suspends: false,
                on_fail: None,
            },
        );
        f.term(Term::Return(None));
        m.add(f.finish());
        let m: &'static Module = Box::leak(Box::new(m));
        let values = Values::new(m, &[]);
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn a_function_whose_value_is_taken_keeps_the_value_abi() {
        // A `MakeClosure` target is entered by `call_indirect`, which now uses the same layouts
        // as a direct call: the function type does not box the record.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let fty = core.fn_type(vec![rec], TypeId::UNIT);
        let mut m = Module::new(core);
        let mut g = FnBuilder::new(&m.core, "g");
        g.param("p", rec);
        g.term(Term::Return(None));
        let g = m.add(g.finish());
        let mut f = FnBuilder::new(&m.core, "f");
        f.assign(
            "c",
            fty,
            Value::MakeClosure {
                function: g,
                captures: Vec::new(),
            },
        );
        f.term(Term::Return(None));
        m.add(f.finish());
        let m: &'static Module = Box::leak(Box::new(m));
        let values = Values::new(m, &[]);
        assert_eq!(
            values.get(values.index(rec).expect("a value")).leaves,
            [Repr::I32]
        );
    }

    #[test]
    fn an_indirect_call_parameter_is_held_by_value() {
        // The closure takes the record, so both the closure's signature and the call's type are
        // its leaf.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let fty = core.fn_type(vec![rec], TypeId::UNIT);
        let mut m = Module::new(core);
        let mut g = FnBuilder::new(&m.core, "g");
        g.param("p", rec);
        g.term(Term::Return(None));
        let g = m.add(g.finish());
        let mut f = FnBuilder::new(&m.core, "f");
        let c = f.assign(
            "c",
            fty,
            Value::MakeClosure {
                function: g,
                captures: Vec::new(),
            },
        );
        let p = f.assign("p", rec, Value::Const(Const::Unit));
        f.let_(
            None,
            Value::Call {
                callee: Callee::Indirect(c),
                args: vec![Operand::Var(p)],
                suspends: false,
                on_fail: None,
            },
        );
        f.term(Term::Return(None));
        m.add(f.finish());
        let m: &'static Module = Box::leak(Box::new(m));
        let values = Values::new(m, &[]);
        assert!(values.index(rec).is_some());
    }

    #[test]
    fn a_suspending_operation_result_keeps_the_box() {
        // The trampoline returns one word selected by representation.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let mut op = emela_core::OpDef::new("tick", Vec::new(), rec);
        op.suspend = true;
        core.add_effect(emela_core::EffectDef {
            name: "Tick".to_owned(),
            decl: "Tick".to_owned(),
            args: Vec::new(),
            ops: vec![op],
            label_of: None,
            display: None,
        });
        let values = values_of(core, |b| {
            b.assign("p", rec, Value::Const(Const::Unit));
        });
        assert_eq!(values.index(rec), None);
    }

    #[test]
    fn an_exported_function_keeps_the_box() {
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let values = values_of(core, |b| {
            b.set_exported();
            b.param("p", rec);
        });
        assert_eq!(values.index(rec), None);
    }

    #[test]
    fn a_const_function_keeps_the_box() {
        // The const wrapper stores its result in one word of a static cell.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let values = values_of(core, |b| {
            b.set_constant();
            b.set_ret(rec);
            b.assign("p", rec, Value::Const(Const::Unit));
        });
        assert_eq!(values.index(rec), None);
    }

    // ---- the pure all-scalar predicate used for inline array elements (E2) ----

    #[test]
    fn an_all_scalar_record_has_an_array_layout() {
        let mut core = emela_core::Module::new();
        let rec = record(
            &mut core,
            "Point",
            vec![("x", TypeId::INT), ("y", TypeId::INT)],
        );
        let l = all_scalar(&core, rec).expect("an all-scalar record");
        assert_eq!(l.leaves, [Repr::I32, Repr::I32]);
        assert_eq!(l.slots(), 2);
    }

    #[test]
    fn an_all_scalar_tuple_has_an_array_layout() {
        let mut core = emela_core::Module::new();
        let t = core.tuple(vec![TypeId::INT64, TypeId::FLOAT]);
        let l = all_scalar(&core, t).expect("an all-scalar tuple");
        assert_eq!(l.leaves, [Repr::I64, Repr::F64]);
    }

    #[test]
    fn a_scalar_is_not_an_inline_element() {
        // A scalar array keeps its one-word element (stride 1); only a record or tuple has an
        // inline layout.
        let core = emela_core::Module::new();
        assert!(all_scalar(&core, TypeId::INT).is_none());
        assert!(all_scalar(&core, TypeId::FLOAT).is_none());
    }

    #[test]
    fn a_reference_field_has_no_array_layout() {
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Named", vec![("name", TypeId::STRING)]);
        assert!(all_scalar(&core, rec).is_none());
    }

    #[test]
    fn an_empty_record_has_no_array_layout() {
        let mut core = emela_core::Module::new();
        let def = EnumDef {
            name: "Empty".to_owned(),
            params: 0,
            variants: vec![VariantDef::unit("Empty")],
            error: false,
            external: false,
        };
        let id = core.add_enum(def);
        let ty = core.types.enum_type(id, vec![]);
        assert!(all_scalar(&core, ty).is_none());
    }

    #[test]
    fn a_multi_variant_enum_has_no_array_layout() {
        let mut core = emela_core::Module::new();
        let def = EnumDef {
            name: "Shape".to_owned(),
            params: 0,
            variants: vec![
                VariantDef::positional("Circle", vec![TypeId::FLOAT]),
                VariantDef::unit("Empty"),
            ],
            error: false,
            external: false,
        };
        let id = core.add_enum(def);
        let ty = core.types.enum_type(id, vec![]);
        assert!(all_scalar(&core, ty).is_none());
    }

    #[test]
    fn a_nested_record_has_no_inline_array_layout_yet() {
        // An inline array element has no per-field reference map, so it holds only scalar leaves.
        // A nested record is still boxed (E1 step 5) and a wider element layout is E2 step 3.
        let mut core = emela_core::Module::new();
        let inner = record(
            &mut core,
            "Point",
            vec![("x", TypeId::INT), ("y", TypeId::INT)],
        );
        let outer = record(&mut core, "Line", vec![("a", inner), ("b", inner)]);
        assert!(all_scalar(&core, outer).is_none());
    }

    #[test]
    fn a_type_used_as_a_parameter_still_has_an_array_layout() {
        // The array predicate is pure: it does not inherit the exclusion of a function whose
        // signature is not flattened, so an array of the type can still store it inline.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        assert_eq!(all_scalar(&core, rec).expect("a pure layout").slots(), 1);
        let values = values_of(core, |b| {
            b.set_exported();
            b.param("p", rec);
        });
        assert_eq!(
            values.index(rec),
            None,
            "the signature of an exported function keeps it boxed"
        );
    }
}
