//! Trait functions: the impls of the Prelude traits, derived impls and the dispatch of a trait
//! function to the impl for a type.

use emela_resolve::hir::{DefId, DefKind, DefModule, Item, Res};
use emela_types::Ty;
use rustc_hash::FxHashMap;

use crate::ir::{FnId, Local, PreludeTraits, Type};

use super::{Lowerer, Specialization};

impl<'a> Lowerer<'a> {
    /// The function `name` of the Prelude's trait `trait_name`.
    pub(super) fn prelude_trait_fn(&self, trait_name: &str, name: &str) -> Option<DefId> {
        let hir = self.hir;
        let t = hir.prelude_def(trait_name)?;
        hir.defs
            .iter()
            .find(|(_, d)| d.kind == DefKind::TraitFn && d.parent == Some(t) && d.name == name)
            .map(|(def, _)| def)
    }

    /// Adds the compiler's impls of the Prelude's Eq, Ord and Show for the primitive types and
    /// tuples (spec 10.6) to the lowered trait functions' dispatch ([`builtin_impls`]).
    pub(super) fn builtin_impls(&mut self) {
        let lowered = |cx: &Self, t: &str, f: &str| {
            cx.prelude_trait_fn(t, f)
                .and_then(|def| cx.fns.get(&(def, 0)).copied())
        };
        let fns = PreludeTraits {
            eq: lowered(self, "Eq", "eq"),
            compare: lowered(self, "Ord", "compare"),
            show: lowered(self, "Show", "show"),
        };
        super::builtin_impls::add(&mut self.module, &fns);
        self.module.traits = fns;
    }

    /// The IR function of the Prelude's trait function `name` of `trait_name`, lowered if it is
    /// not yet. Lowering it puts its impls in `Module::dispatch`, where monomorphization looks
    /// for the impl of `==` and interpolation (spec 10.3).
    pub(super) fn prelude_trait_fn_id(&mut self, trait_name: &str, name: &str) -> Option<FnId> {
        let key = (trait_name.to_owned(), name.to_owned());
        let def = match self.prelude_trait_fns.get(&key) {
            Some(&d) => d,
            None => {
                let d = self.prelude_trait_fn(trait_name, name);
                self.prelude_trait_fns.insert(key, d);
                d
            }
        }?;
        Some(self.fn_id(def, 0))
    }

    /// Parameters with the types of the declared type, named after its labels. For a trait
    /// function without a body, whose parameters were not inferred (or have no source, in the
    /// Prelude).
    pub(super) fn declared_params(&mut self, def: DefId) -> Vec<Local> {
        let types = self.types;
        let Some(sig) = types.sigs.get(def) else {
            return Vec::new();
        };
        let Ty::Fn(f) = &sig.scheme.ty else {
            return Vec::new();
        };
        f.params
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let ty = self.ty(t);
                let hint = sig.labels.get(i).cloned().flatten().unwrap_or_default();
                self.module.local(&hint, ty)
            })
            .collect()
    }

    /// Records the impls of trait function `def` (copy `spec`) in `Module::dispatch`, and puts
    /// each impl function on the worklist: which one a call needs is known only once
    /// monomorphization knows `Self`. `default` tells whether the trait function has a body.
    pub(super) fn dispatch(&mut self, def: DefId, spec: Specialization, default: bool) {
        let hir = self.hir;
        let types = self.types;
        let Some(trait_def) = hir.defs[def].parent else {
            return;
        };
        let mut impls = Vec::new();
        let mut structural = Vec::new();
        for impl_def in types.traits.impls_of(trait_def) {
            let (Some(Item::Impl(item)), Some(head)) =
                (hir.item(impl_def), types.traits.head(impl_def))
            else {
                continue;
            };
            let Some(f) = item
                .fns
                .iter()
                .find(|f| f.trait_fn == Res::Def(def) && types.sigs.get(f.def).is_some())
            else {
                continue;
            };
            // The impl function's set type parameters follow the impl's type parameters instead
            // of `Self`.
            let impl_spec = (spec >> 1) << head.params.len();
            let pattern = self.ty(&head.self_ty);
            let f = self.fn_id(f.def, impl_spec);
            impls.push((pattern, f));
            // The impls of core for `List` and `Option` give what the structure gives (spec
            // 10.6). Other impls in core and std, such as Duration's Show, do not.
            let in_core = match hir.defs[impl_def].module {
                DefModule::Source(m) => self.modules.is_core(m),
                DefModule::Prelude | DefModule::Builtin(_) => true,
            };
            let list_or_option = match self.module.ty(pattern) {
                Type::List(_) => true,
                Type::Enum(id, _) => self.module.option == Some(*id),
                _ => false,
            };
            if in_core && list_or_option {
                structural.push(f);
            }
        }
        // Encode, Decode, ObjectKey and Shaped of std: generic functions, like written impls.
        if spec == 0 {
            impls.extend(self.std_derived_impls(def, trait_def));
        }
        let derived = self.derived(trait_def, def);
        let id = self.fns[&(def, spec)];
        self.module.dispatch.insert(
            id,
            crate::ir::Dispatch {
                impls,
                structural,
                default,
                derived,
            },
        );
    }

    /// Fills `Module::trait_impls` and `Module::show_trait`: every trait's impls, written and
    /// derived, with the constraints of their type parameters (`impl Wrap[A: Eq]: Show`), so that
    /// monomorphization can tell whether a type satisfies Show (spec 10.6).
    pub(super) fn trait_impls(&mut self) {
        use crate::ir::{DeriveOp, TraitImpls};
        let hir = self.hir;
        let types = self.types;
        let traits: Vec<DefId> = hir
            .defs
            .iter()
            .filter(|(_, d)| d.kind == DefKind::Trait)
            .map(|(def, _)| def)
            .collect();
        let index: FxHashMap<DefId, usize> =
            traits.iter().enumerate().map(|(i, &d)| (d, i)).collect();
        let mut table = Vec::with_capacity(traits.len());
        for &trait_def in &traits {
            let name = hir.defs[trait_def].name.as_str();
            let builtin = if hir.prelude_def(name) == Some(trait_def) {
                match name {
                    "Eq" => Some(DeriveOp::Eq),
                    "Ord" => Some(DeriveOp::Compare),
                    "Show" => Some(DeriveOp::Show),
                    _ => None,
                }
            } else {
                None
            };
            let heads: Vec<_> = types
                .traits
                .impls_of(trait_def)
                .into_iter()
                .filter_map(|i| types.traits.head(i).cloned())
                .chain(
                    types
                        .traits
                        .derived_of(trait_def)
                        .into_iter()
                        .filter_map(|ty| types.traits.derived(trait_def, ty))
                        .map(|d| d.head.clone()),
                )
                .collect();
            let impls = heads
                .iter()
                .map(|head| {
                    let pattern = self.ty(&head.self_ty);
                    let bounds = head
                        .params
                        .iter()
                        .map(|p| {
                            p.traits
                                .iter()
                                .filter_map(|t| index.get(t).copied())
                                .collect()
                        })
                        .collect();
                    (pattern, bounds)
                })
                .collect();
            table.push(TraitImpls { builtin, impls });
        }
        self.module.show_trait = hir.prelude_def("Show").and_then(|d| index.get(&d).copied());
        self.module.trait_impls = table;
    }

    /// For a function of a derivable trait of the Prelude (`Eq.eq`, `Ord.compare`, `Show.show`),
    /// the enums whose `derive` lists the trait (spec 10.5). Bool is `TypeId::BOOL`, not an enum.
    pub(super) fn derived(&self, trait_def: DefId, def: DefId) -> Option<crate::ir::Derived> {
        use crate::ir::DeriveOp;
        let hir = self.hir;
        if hir.prelude_def(&hir.defs[trait_def].name) != Some(trait_def) {
            return None;
        }
        let op = match (
            hir.defs[trait_def].name.as_str(),
            hir.defs[def].name.as_str(),
        ) {
            ("Eq", "eq") => DeriveOp::Eq,
            ("Ord", "compare") => DeriveOp::Compare,
            ("Show", "show") => DeriveOp::Show,
            _ => return None,
        };
        let enums = self
            .types
            .traits
            .derived_of(trait_def)
            .into_iter()
            .filter_map(|ty| self.enums.get(ty).copied())
            .collect();
        Some(crate::ir::Derived { op, enums })
    }
}
