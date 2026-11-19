//! Records and tuples that the WASM backend may hold by value in WASM locals (E1, step 1).
//!
//! A record (`type Point(x: Int, y: Int)`, an enum with exactly one variant) or a tuple whose
//! leaves are all `i32`, `i64` or `f64` can be represented as a **span of WASM locals**: one local
//! per scalar leaf of its layout, in field order. A nested record or tuple expands in place, so
//! the leaves are the slots a box of the value would have (`8` bytes each). The WASM Heap IR is
//! unchanged: these values still have [`Repr::Ref`] there; only the emitter's local layout differs
//! (the C backend's `layout.rs` makes the same decision with C structs).
//!
//! # What is never held by value here
//!
//! This is deliberately narrower than the C backend. The layout is only used for the first slice
//! (E1 step 2), which changes **locals only**: no function signature, descriptor, static, array,
//! list, enum payload, closure capture or suspension frame changes. A type keeps `Repr::Ref` when
//! the module uses it in any of those places:
//!
//! - a parameter or result of any function (including a lifted closure or operation
//!   implementation), a function type, or an effect operation;
//! - an element of `Array`, `List` or `Cell` (and, transitively, anything inside such an element);
//! - a field of an enum that is not itself a value (a multi-variant enum, an error, an external
//!   type, a nullary enum);
//! - a captured variable of a closure (`Function::env`, `Value::MakeClosure`);
//! - an argument of another value's construction that is itself stored in a box;
//! - a `Value::Coerce` (the `Any` boundary);
//! - any variable of a suspending function, or any variable saved across a suspension.
//!
//! A type that is excluded, or that contains an excluded field, has no layout, and the emitter
//! keeps its boxed representation everywhere. The exclusion is a whole-module property: a type is
//! held by value only if every use in the whole program is one the local-only slice converts.
//!
//! No proposal is needed: `[target.unspecified]` leaves the memory representation of values to
//! the implementation, and this changes only how many objects are allocated.
//!
//! # The layout
//!
//! Unlike C, padding after a 32-bit field is not a leaf: every WASM local has its own type, so
//! `Mixed(n: Int, f: Float)` is two locals (`i32`, `f64`) rather than a padded struct. The number
//! of leaves is still the number of 8-byte slots a box of the value would have, which is what a
//! later step (boxing at an escape boundary) needs.

use emela_core::{EnumId, Type, TypeId};
use emela_lir::{Const, Module, Operand, Repr, Stmt, Value, repr_of};
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
    pub(crate) fn new(m: &'m Module) -> Values<'m> {
        let mut values = Values {
            core: &m.core,
            layouts: Vec::new(),
            of: FxHashMap::default(),
            excluded: excluded(m),
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
/// A field of reference representation is rejected, directly or through a nested record. The
/// reason is not only that an inline element has no per-field reference map: on WASM a box of a
/// nested record stores a *reference* to the nested box, not the nested value's slots, until the
/// E1 step that boxes values flattened. Copying such a box's slots into an array would therefore
/// store a reference and break the no-references-inside rule. The nested type stays boxed, which
/// is correct (only an allocation is left for a later step).
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

/// Whether the shape of `ty` could be a value at all, ignoring the module's uses: a non-empty
/// tuple, or a single-variant non-error non-external enum with fields.
fn candidate(core: &emela_core::Module, ty: TypeId) -> bool {
    match core.ty(ty) {
        Type::Tuple(elems) => !elems.is_empty(),
        Type::Enum(id, _) => candidate_enum(core, *id),
        _ => false,
    }
}

fn candidate_enum(core: &emela_core::Module, id: EnumId) -> bool {
    let def = &core.enums[id];
    !def.error && !def.external && def.variants.len() == 1 && !def.variants[0].tys.is_empty()
}

/// Adds `ty` and every type argument reachable from it to `out`. Used where a value is stored in a
/// reference-shaped container (an array/list/cell element, an enum field of a boxed enum, a
/// function signature), so that nothing nested inside it can be held by value either.
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

/// The types of `m` that the local-only slice cannot convert.
///
/// The first pass marks the positions that always need a reference. The second repeats until it
/// is stable: a construction or read whose destination has no layout stores its operands in a
/// box, so those operand types cannot be held by value either.
fn excluded(m: &Module) -> FxHashSet<TypeId> {
    let core = &m.core;
    let mut out = FxHashSet::default();

    // Containers store references: the element of an array, a list or a cell, and every type
    // nested inside such an element.
    for ty in core.types.iter() {
        match ty {
            Type::Array(elem) | Type::List(elem) | Type::Cell(elem) => {
                mark_subtree(core, *elem, &mut out);
            }
            // The type arguments of a boxed enum are values it stores in its fields.
            Type::Enum(id, args) if !candidate_enum(core, *id) => {
                for &a in args {
                    mark_subtree(core, a, &mut out);
                }
            }
            // A function type's parameters and result cross a call boundary.
            Type::Fn(f) => {
                for &p in &f.params {
                    mark_subtree(core, p, &mut out);
                }
                mark_subtree(core, f.ret, &mut out);
            }
            _ => {}
        }
    }

    // The fields of an enum that is not a value are boxed slots.
    for (_, def) in core.enums.iter() {
        if def.error || def.external || def.variants.len() != 1 || def.variants[0].tys.is_empty() {
            for variant in &def.variants {
                for &t in &variant.tys {
                    mark_subtree(core, t, &mut out);
                }
            }
        }
    }

    // An operation's parameters and result cross the handler ABI.
    for (_, effect) in core.effects.iter() {
        for op in &effect.ops {
            for &t in &op.params {
                mark_subtree(core, t, &mut out);
            }
            mark_subtree(core, op.ret, &mut out);
        }
    }

    for (_, f) in m.functions.iter() {
        // A parameter or result is part of a wasm signature; a capture is a closure slot.
        for &p in &f.params {
            out.insert(f.vars[p].ty);
        }
        out.insert(f.ret);
        if let Some(env) = &f.env {
            for &t in env {
                out.insert(t);
            }
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
            // A destination is buildable in place only from these values. Everything else (a
            // call, a builtin, a runtime operation, `Any`, a read out of a box) needs a
            // reference, so the type stays boxed.
            let direct = matches!(
                value,
                Value::Make { .. }
                    | Value::Tuple(_)
                    | Value::Field { .. }
                    | Value::TupleGet { .. }
                    // A unit constant bound to a record-typed variable is test scaffolding (the
                    // Core IR never produces one: `()` has no representation).
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
                Value::Prim { args, .. }
                | Value::Call { args, .. }
                | Value::Builtin { args, .. }
                | Value::Fiber { args, .. }
                | Value::Runtime { args, .. } => {
                    for a in args {
                        mark(a);
                    }
                }
                Value::List { elems, tail } => {
                    for a in elems {
                        mark(a);
                    }
                    if let Some(t) = tail {
                        mark(t);
                    }
                }
                Value::MakeClosure { captures, .. } => {
                    for &v in captures {
                        out.insert(f.vars[v].ty);
                    }
                }
                Value::Handler { ops, .. } => {
                    for &v in ops {
                        out.insert(f.vars[v].ty);
                    }
                }
                Value::Coerce(v) => {
                    out.insert(f.vars[*v].ty);
                    if let Some(d) = dst {
                        out.insert(f.vars[*d].ty);
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
                // A construction whose operands may be stored in a box is handled by the fixed
                // point below, which knows whether the destination has a layout.
                Value::Make { .. } | Value::Tuple(_) => {}
                // `Field`/`TupleGet` read a base; the fixed point handles a boxed base below.
                Value::Field { .. } | Value::Tag(_) | Value::TupleGet { .. } => {}
                Value::Const(_) | Value::FnRef(_) | Value::Closure(_) => {}
            }
        }
    }

    // A construction whose destination has no layout stores its arguments in a box; a read from
    // a base with no layout unboxes one. Mark those operand types and repeat, because excluding
    // them can make another destination unbuildable.
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
                let Stmt::Let { dst, value, .. } = stmt else {
                    continue;
                };
                match value {
                    Value::Make { args, .. } | Value::Tuple(args) => {
                        let usable = dst.is_some_and(|d| probe.layout(f.vars[d].ty).is_some());
                        if !usable {
                            for a in args {
                                if let Operand::Var(v) = a {
                                    extra.insert(f.vars[*v].ty);
                                }
                            }
                        }
                    }
                    Value::Field { base, .. } | Value::TupleGet { base, .. } => {
                        if let Some(d) = dst
                            && candidate(core, f.vars[*d].ty)
                            && probe.layout(f.vars[*base].ty).is_none()
                        {
                            extra.insert(f.vars[*d].ty);
                        }
                    }
                    // A list head or a closure capture is always boxed.
                    Value::ListHead(_) | Value::EnvGet { .. } => {
                        if let Some(d) = dst
                            && candidate(core, f.vars[*d].ty)
                        {
                            extra.insert(f.vars[*d].ty);
                        }
                    }
                    _ => {}
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
        Values::new(m)
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
    fn a_value_in_a_list_keeps_the_box() {
        // A list cell's head is a reference, so the whole program holds the record boxed.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let list = core.types.list(rec);
        let values = values_of(core, |b| {
            b.assign("p", rec, Value::Const(Const::Unit));
            b.assign("xs", list, Value::Const(Const::Unit));
        });
        assert_eq!(values.index(rec), None);
    }

    #[test]
    fn a_function_result_keeps_the_box() {
        // A wasm function has one result; a record result is a later step.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        let values = values_of(core, |b| {
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
        // The local-only E1 slice (and the boxed build generally) boxes a nested record as a
        // *reference* in its parent, not as inline leaves. Copying that box's slots into an array
        // element would store a reference, so an array of a nested record stays boxed until the
        // boxing step flattens values (E1 step 5 / E2 step 3).
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
        // The array predicate is pure: it does not inherit the local-only exclusion of a
        // function parameter, so an array of the type can still store it inline.
        let mut core = emela_core::Module::new();
        let rec = record(&mut core, "Point", vec![("x", TypeId::INT)]);
        assert_eq!(all_scalar(&core, rec).expect("a pure layout").slots(), 1);
        let values = values_of(core, |b| {
            b.param("p", rec);
        });
        assert_eq!(
            values.index(rec),
            None,
            "the local-only predicate keeps it boxed"
        );
    }
}
