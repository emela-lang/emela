use super::*;
use crate::contract::auxiliary::DerivationPlan as Plan;
pub(in crate::compiler::plan) trait Preparation: Sized {
    fn new(
        flow: &crate::contract::ir::Program,
        emitted: &HashSet<FnId>,
        layouts: &TypeLayouts<'_>,
    ) -> Result<Self, Vec<Error>>;
    fn needs_float(&self, layouts: &TypeLayouts<'_>) -> bool;
    fn require_runtime(
        &self,
        layouts: &TypeLayouts<'_>,
        requirements: &mut crate::contract::runtime::RuntimeRequirements,
    );
}
impl Preparation for Plan {
    fn new(
        flow: &crate::contract::ir::Program,
        emitted: &HashSet<FnId>,
        layouts: &TypeLayouts<'_>,
    ) -> Result<Self, Vec<Error>> {
        let mut roots = Vec::new();
        let mut show = false;
        let mut eq = false;
        for (id, f) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
            let Some(kind) = f.derivation else {
                continue;
            };
            let ty = f
                .params
                .first()
                .map(|&p| f.vars[p].ty)
                .ok_or_else(|| invalid_flow_ir(&f.name, "derived operation has no argument"))?;
            let valid = !f.suspends
                && !f.fails
                && f.body.is_none()
                && !f.exported
                && crate::semantics::derived::supported(&flow.core, ty, kind)
                && match kind {
                    DeriveKind::Show => f.params.len() == 1 && f.ret == TypeId::STRING,
                    DeriveKind::Eq => {
                        f.params.len() == 2 && f.vars[f.params[1]].ty == ty && f.ret == TypeId::BOOL
                    }
                };
            if !valid {
                return Err(unsupported(
                    &f.name,
                    "invalid or unsupported embedded structural derivation signature",
                ));
            }
            show |= kind == DeriveKind::Show;
            eq |= kind == DeriveKind::Eq;
            let _ = id;
            roots.push((kind, ty));
        }
        let mut show_types = HashSet::new();
        let mut eq_types = HashSet::new();
        fn collect(layouts: &TypeLayouts<'_>, ty: TypeId, seen: &mut HashSet<TypeId>) {
            if !seen.insert(ty) {
                return;
            }
            match layouts.shapes.types[ty].clone() {
                Type::List(t) => collect(layouts, t, seen),
                _ => {
                    if let Some(&index) = layouts.shapes.aggregate_ids.get(&ty) {
                        let variants = match &layouts.shapes.aggregates[index].kind {
                            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                            AggregateKind::Enum(variants) => {
                                variants.iter().map(Vec::as_slice).collect()
                            }
                        };
                        for (variant, fields) in variants.into_iter().enumerate() {
                            for (field, &t) in fields.iter().enumerate() {
                                if !layouts.is_recursive_box_field(ty, variant, field) {
                                    collect(layouts, t, seen);
                                }
                            }
                        }
                    }
                }
            }
        }
        for (kind, ty) in roots {
            let seen = if kind == DeriveKind::Show {
                &mut show_types
            } else {
                &mut eq_types
            };
            collect(layouts, ty, seen);
        }
        for (kind, enabled) in [(DeriveKind::Show, show), (DeriveKind::Eq, eq)] {
            if !enabled {
                continue;
            }
            let seen = if kind == DeriveKind::Show {
                &mut show_types
            } else {
                &mut eq_types
            };
            for &ty in &layouts.shapes.recursive_box_types {
                if crate::semantics::derived::supported(&flow.core, ty, kind) {
                    collect(layouts, ty, seen);
                }
            }
        }
        let mut types: Vec<_> = show_types
            .union(&eq_types)
            .copied()
            .filter(|&t| layouts.type_repr(t) != Repr::Void)
            .collect();
        types.sort_by_key(|t| t.index());
        Ok(Self {
            show,
            eq,
            types,
            show_types,
            eq_types,
        })
    }

    fn needs_float(&self, layouts: &TypeLayouts<'_>) -> bool {
        self.show
            && self
                .show_types
                .iter()
                .any(|&t| matches!(layouts.shapes.types[t], Type::Float))
    }

    fn require_runtime(
        &self,
        layouts: &TypeLayouts<'_>,
        requirements: &mut crate::contract::runtime::RuntimeRequirements,
    ) {
        if self.show {
            requirements.require(crate::contract::runtime::RuntimeHelper::StringShow);
            requirements.require(crate::contract::runtime::RuntimeHelper::IntToString);
        }
        if self.needs_float(layouts) {
            requirements.require(crate::contract::runtime::RuntimeHelper::FloatToString);
        }
    }
}

use crate::DerivedOperation as DeriveKind;
