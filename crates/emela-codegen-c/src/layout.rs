//! Records and tuples held by value in the C output: the prototype of `Repr::Agg`.
//!
//! A record (the one-variant enum that `type Body(…)` makes) or a tuple whose fields are all
//! 8-byte scalars, or such aggregates, is emitted as a C struct and held **by value**: a local is
//! the struct, a parameter and a result are passed by value, and an array of the type holds the
//! structs inline (one allocation for the whole array, no object per element). The alternative,
//! which every other value keeps, is a heap object with a header and a count.
//!
//! Whether a value is a pointer or its bytes is implementation-defined memory representation, so
//! this needs no proposal (`[target.unspecified]`); only the number of allocations and the timing
//! of running out of memory change, and those are implementation-defined too.
//!
//! # Where the layout is chosen
//!
//! Here, in the C backend, at the point of emission, and not in the LIR. The Heap IR keeps
//! saying `Repr::Ref` for these values and does not change; the wasm and JS backends, the
//! verifiers, the dumps and the Core and LIR passes do not change either. The reasons:
//!
//! - It is a C layout decision. The IR's `Repr` is shared by three backends whose layouts differ
//!   (wasm would use a stride in linear memory, JS would keep objects).
//! - `Repr` is `Copy`, small, and used for `em_word` slots, state-machine frames, boxing and
//!   builtin signatures. An aggregate cannot go in an `em_word`, so a LIR `Repr::Agg` would have
//!   to teach all of those places, in all three backends, what to do with it, to save what this
//!   module saves in one.
//! - The existing escape analysis and SRA are not reused because they are not needed: a value
//!   that escapes is boxed at the boundary where it becomes a reference (see below), so nothing
//!   has to deopt. SRA still runs on non-escaping tuples in the LIR, before this.
//!
//! # The boxed form
//!
//! A value of such a type still has to be a heap reference in three places: a list element, a
//! closure environment slot, and the field of an enum value (`Some(body)`, `Any`). There the
//! emitter boxes it: a heap object whose slots are the bytes of the struct, one 8-byte slot per 8
//! bytes. Every field is exactly 8 bytes wide (a 32-bit scalar is padded) or a nested struct of
//! 8-byte fields, so a box and a struct have the same layout, and boxing and unboxing are
//! `memcpy`. A box is made only where a reference is needed: a `Body` that lives in an `Array`,
//! in locals and in loops is never boxed.
//!
//! # What is not held by value
//!
//! A type is held by value only if the whole program uses it in ways the emitter converts. The
//! following keep the boxed representation for the type:
//!
//! - a value live across a suspension point: a state-machine frame slots values as words;
//! - an argument of a fiber or runtime operation, or of a `Cell`;
//! - the result of a `const fn`, whose cache is a word;
//! - a type with a field that is not a scalar or such an aggregate (`String`, `List`, a
//!   multi-variant enum, `Any`, …): that field is a reference.
//!
//! # The C shape
//!
//! ```c
//! typedef struct { double f0; double f1; } em_a3;
//! _Static_assert(sizeof(em_a3) == 16, "the layout of Body");
//! ```
//!
//! Fields are `f0`, `f1`, … in field-definition order; the padding after a 32-bit field is `p0`,
//! `p1`, …. The names the program uses for them are not in the C: they are in the comment of the
//! declaration.

use emela_core::{Builtin, Type, TypeId};
use emela_lir::{Callee, Module, Operand, Repr, Stmt, Value, repr_of};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::writer::Writer;

/// A type held by value, and the C struct that holds it.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    /// The C typedef name (`em_a3`).
    pub(crate) c_ty: String,
    /// The members of the struct, in order, including the padding after a 32-bit field.
    fields: Vec<Field>,
    /// The member of each field, in field-definition order.
    names: Vec<String>,
    /// The size in bytes: a multiple of 8, and `8 * slots()`.
    pub(crate) size: usize,
    /// The constructor tag of a box of this type (0: it has one variant).
    pub(crate) tag: u32,
    /// The name of a box of this type, for the allocation report.
    pub(crate) name: String,
}

#[derive(Debug, Clone)]
struct Field {
    /// `double`, `int64_t`, `int32_t`, or the typedef of a nested layout.
    c_ty: String,
    /// `f3`, or `p3` for the padding after `f3`.
    name: String,
}

impl Layout {
    /// The number of 8-byte slots the bytes of a value take: the slots of its box.
    pub(crate) fn slots(&self) -> usize {
        self.size / 8
    }

    /// The C member that holds field `i`.
    pub(crate) fn member(&self, index: usize) -> &str {
        &self.names[index]
    }

    /// `{a, b, …}`, the initializer of the struct from the values of its fields in order (the
    /// padding after a 32-bit field is 0).
    pub(crate) fn init(&self, values: impl Iterator<Item = String>) -> String {
        let mut out = String::from("{");
        let mut values = values;
        for (i, f) in self.fields.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            if f.name.starts_with('p') {
                out.push('0');
            } else {
                out.push_str(&values.next().unwrap_or_else(|| "0".to_owned()));
            }
        }
        out.push('}');
        out
    }
}

/// The types of a module that are held by value, with their layouts.
#[derive(Debug)]
pub(crate) struct Values<'m> {
    core: &'m emela_core::Module,
    layouts: Vec<Layout>,
    /// The layout of each type that was asked for (`None`: held as it was).
    of: FxHashMap<TypeId, Option<usize>>,
    /// The types the module uses in a way that cannot be converted.
    excluded: FxHashSet<TypeId>,
    /// The types whose layout is being built, to stop at a type that would contain itself.
    building: FxHashSet<TypeId>,
}

impl<'m> Values<'m> {
    /// The layouts of `m`, for the types that the whole module uses in a way the emitter
    /// converts (see the module documentation).
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

    /// No layouts: every value keeps its boxed representation (a test compiles a program this way
    /// to compare the two).
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

    /// The C declarations of the struct types.
    pub(crate) fn declare(&self, w: &mut Writer) {
        if self.layouts.is_empty() {
            return;
        }
        w.comment(
            "Records and tuples held by value (spec 16.4: the representation is \
             implementation-defined).",
        );
        for l in &self.layouts {
            let members: Vec<String> = l
                .fields
                .iter()
                .map(|f| format!("{} {};", f.c_ty, f.name))
                .collect();
            w.raw(&format!(
                "typedef struct {{ {} }} {};",
                members.join(" "),
                l.c_ty
            ));
            let name: String = l
                .name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            w.raw(&format!(
                "_Static_assert(sizeof({}) == {}, \"the layout of {name}\");",
                l.c_ty, l.size
            ));
        }
        w.blank();
    }

    /// The layout of `ty`, built on first use. It is not held by value if the module uses it in a
    /// way the emitter cannot convert, or if a field has no 8-byte form.
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
        let (name, fields) = match core.ty(ty) {
            Type::Tuple(elems) => ("tuple".to_owned(), elems.clone()),
            Type::Enum(id, args) => {
                let def = &core.enums[*id];
                // A record: one variant with fields. An enum with several variants needs a tag,
                // which this prototype does not give a value form.
                if def.error || def.external || def.variants.len() != 1 {
                    return None;
                }
                let variant = &def.variants[0];
                if variant.fields.is_empty() {
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
                (variant.name.clone(), resolved)
            }
            _ => return None,
        };
        let mut members = Vec::new();
        let mut names = Vec::new();
        let mut size = 0;
        for (at, field) in fields.into_iter().enumerate() {
            let (c_ty, pad, width) = self.field(field)?;
            // Every field starts at a multiple of 8 (a scalar is 8 bytes, or 4 with the padding
            // after it; a nested layout is a multiple of 8), and C needs no padding before one.
            debug_assert_eq!(size % 8, 0);
            debug_assert_eq!(width % 8, 0);
            names.push(format!("f{at}"));
            members.push(Field {
                c_ty,
                name: format!("f{at}"),
            });
            if pad {
                members.push(Field {
                    c_ty: "int32_t".to_owned(),
                    name: format!("p{at}"),
                });
            }
            size += width;
        }
        if members.is_empty() {
            return None;
        }
        let index = self.layouts.len();
        Some(Layout {
            c_ty: format!("em_a{index}"),
            fields: members,
            names,
            size,
            tag: 0,
            name,
        })
    }

    /// The C type of one field, whether it needs padding after it, and the bytes it takes (a
    /// multiple of 8); `None` if the field has no such form.
    fn field(&mut self, ty: TypeId) -> Option<(String, bool, usize)> {
        match repr_of(self.core, ty) {
            Repr::I32 => Some(("int32_t".to_owned(), true, 8)),
            Repr::I64 => Some(("int64_t".to_owned(), false, 8)),
            Repr::F64 => Some(("double".to_owned(), false, 8)),
            // A nested record or tuple, held inline. Anything else is a reference, which has no
            // place in a value.
            Repr::Ref => {
                let index = self.layout(ty)?;
                let nested = &self.layouts[index];
                Some((nested.c_ty.clone(), false, nested.size))
            }
            Repr::Void => None,
        }
    }
}

/// The types of `m` that the emitter cannot convert, so they keep the boxed representation (see
/// the module documentation).
fn excluded(m: &Module) -> FxHashSet<TypeId> {
    let mut out = FxHashSet::default();
    for (_, f) in m.functions.iter() {
        if f.constant {
            out.insert(f.ret);
        }
        let var = |v: &Operand, out: &mut FxHashSet<TypeId>| {
            if let Operand::Var(v) = v {
                out.insert(f.vars[*v].ty);
            }
        };
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
            // A word comes back from a fiber operation, a runtime operation, a suspension and a
            // suspending effect operation; none of them can hold a value.
            let word_result = matches!(
                value,
                Value::Fiber { .. } | Value::Runtime { .. } | Value::Call { suspends: true, .. }
            );
            if word_result && let Some(d) = dst {
                out.insert(f.vars[*d].ty);
            }
            if let Value::Call {
                callee: Callee::Op { effect, op, .. },
                ..
            } = value
                && m.core.effects[*effect].ops[*op].suspend
            {
                out.insert(m.core.effects[*effect].ops[*op].ret);
            }
            let args: &[Operand] = match value {
                Value::Fiber { args, .. } | Value::Runtime { args, .. } => args,
                Value::Builtin {
                    op: Builtin::CellNew | Builtin::CellSet,
                    args,
                } => args,
                _ => continue,
            };
            for o in args {
                var(o, &mut out);
                // A fiber operation calls the closure it is given through the runtime, which
                // passes the result as a word and a representation.
                if let Operand::Var(v) = o
                    && let Type::Fn(ft) = m.core.ty(f.vars[*v].ty)
                {
                    out.insert(ft.ret);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_core::{EnumDef, VariantDef};
    use emela_lir::{Const, FnBuilder, Term};

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
        assert_eq!(l.c_ty, "em_a0");
        assert_eq!(l.size, 24);
        assert_eq!(l.slots(), 3);
        assert_eq!(l.name, "Body");
    }

    #[test]
    fn a_32_bit_field_is_padded_to_a_slot() {
        let (values, types) = values_with(|core| {
            vec![record(
                core,
                "Mixed",
                vec![("n", TypeId::INT), ("f", TypeId::FLOAT)],
            )]
        });
        let l = values.get(values.index(types[0]).expect("a value"));
        assert_eq!(l.size, 16);
        let mut w = Writer::new();
        values.declare(&mut w);
        let text = w.finish();
        assert!(
            text.contains("typedef struct { int32_t f0; int32_t p0; double f1; } em_a0;"),
            "{text}"
        );
        assert!(
            text.contains("_Static_assert(sizeof(em_a0) == 16, \"the layout of Mixed\");"),
            "{text}"
        );
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
        assert_eq!(l.size, 40, "two 16-byte points and a padded Int");
        let mut w = Writer::new();
        values.declare(&mut w);
        let text = w.finish();
        assert!(
            text.contains("typedef struct { em_a0 f0; em_a0 f1; int32_t f2; int32_t p2; } em_a1;"),
            "{text}"
        );
    }

    #[test]
    fn a_tuple_is_held_by_value() {
        let (values, types) =
            values_with(|core| vec![core.tuple(vec![TypeId::INT, TypeId::FLOAT])]);
        let l = values.get(values.index(types[0]).expect("a value"));
        assert_eq!((l.size, l.name.as_str()), (16, "tuple"));
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
                    op: Builtin::CellNew,
                    args: vec![Operand::Var(p)],
                },
            );
        });
        assert_eq!(values.index(rec), None);
    }
}
