//! Types, enums and signatures: how the checker's types become IR types (`Lowerer::ty`), the
//! enums and their constructors, and the parts of a function's signature.

use emela_resolve::hir::{DefId, ExprId, Fields, Item};
use emela_types::{Prim, Row, RowElem, Ty};

use crate::ir::{
    CtorRef, EnumDef, EnumId, FnSets, FnType, SetSummary, Type, TypeId, VariantDef, VariantFields,
};

use super::{Lowerer, is_stateful, match_bound};

impl<'a> Lowerer<'a> {
    /// Allocates IDs and names for every type / enum / error. Constructors are added later.
    pub(super) fn declare_enums(&mut self) {
        let bool_def = self.hir.prelude_def("Bool");
        let option_def = self.hir.prelude_def("Option");
        for (_, items) in self.hir.modules.iter() {
            // Inner handlers (`[effect.handler]`) are not items of the module.
            let mut defs = items.items.clone();
            let mut k = 0;
            while k < defs.len() {
                if let Some(Item::Handler(h)) = self.hir.item(defs[k]) {
                    defs.extend(h.inner.iter().copied());
                }
                k += 1;
            }
            for def in defs {
                match self.hir.item(def) {
                    // Bool is a builtin type in the IR (`TypeId::BOOL`).
                    Some(Item::Type(_) | Item::Enum(_)) if Some(def) == bool_def => {}
                    // Prelude's Option refers to the same definition as the builtins
                    // (`Module::option_enum`).
                    Some(Item::Enum(_)) if Some(def) == option_def => {
                        let id = self.module.option_enum();
                        self.enums.insert(def, id);
                    }
                    // A built-in type is `Type::Bytes` or `Type::Array` (`Lowerer::ty`).
                    Some(Item::Type(t)) if t.is_intrinsic => {}
                    Some(Item::Type(t)) => {
                        let id = self.declare_enum(def, t.type_params.len());
                        // An `@external type` has no constructor (spec 12.3).
                        self.module.enums[id].external = t.ctor.is_none();
                        self.enums.insert(def, id);
                    }
                    Some(Item::Enum(e)) => {
                        let id = self.declare_enum(def, e.type_params.len());
                        self.enums.insert(def, id);
                    }
                    // An error defines a type and a constructor of the same name, like `type`
                    // (spec 7.1). It has no type parameters.
                    Some(Item::Error(_)) => {
                        let id = self.declare_enum(def, 0);
                        self.module.enums[id].error = true;
                        self.enums.insert(def, id);
                    }
                    // A stateful handler's value (self) is an enum with the handler's one
                    // constructor (spec 8.4).
                    // It takes the handler's type parameters (`[effect.handler]`).
                    Some(Item::Handler(h)) if is_stateful(h) => {
                        let id = self.declare_enum(def, self.handler_type_params(def));
                        self.enums.insert(def, id);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Adds constructors to enums and builds the constructor table (`ctors`).
    ///
    /// Constructor field types come from the type checker's constructor declarations. A
    /// constructor's type parameters are in the same order as the constructed type's, so
    /// `Ty::Bound(i)` maps directly to `Type::Param(i)`.
    pub(super) fn define_ctors(&mut self) {
        // `self.module` cannot be mutated while `self.enums` is borrowed, so copy the pairs first.
        let enums: Vec<(DefId, EnumId)> = self.enums.iter().map(|(d, &e)| (d, e)).collect();
        for (def, enum_id) in enums {
            let ctors = match self.hir.item(def) {
                Some(Item::Type(t)) => t.ctor.into_iter().collect(),
                Some(Item::Enum(e)) => e.variants.clone(),
                Some(Item::Error(_)) => vec![def],
                Some(Item::Handler(h)) => h.ctor.into_iter().collect(),
                _ => Vec::new(),
            };
            for (variant, ctor) in ctors.into_iter().enumerate() {
                self.ctors.insert(ctor, CtorRef { enum_id, variant });
                // Option's constructors were added by `EnumDef::option`.
                if Some(enum_id) != self.module.option {
                    let v = self.variant_def(ctor);
                    self.module.enums[enum_id].variants.push(v);
                }
            }
        }
    }

    pub(super) fn variant_def(&mut self, ctor: DefId) -> VariantDef {
        let fields = match self.hir.item(ctor) {
            Some(Item::Ctor(c)) => &c.fields,
            Some(Item::Error(e)) => &e.fields,
            _ => &Fields::None,
        };
        let fields = match fields {
            Fields::None => VariantFields::Unit,
            Fields::Positional(tys) => VariantFields::Positional(tys.len()),
            Fields::Named(fs) => {
                VariantFields::Named(fs.iter().map(|f| f.name.to_string()).collect())
            }
        };
        // A constructor without fields has the constructed type itself, not a function type.
        let types = self.types;
        let tys = match types.sigs.get(ctor).map(|sig| &sig.scheme.ty) {
            Some(Ty::Fn(f)) => f.params.iter().map(|t| self.ty(t)).collect(),
            _ => Vec::new(),
        };
        VariantDef {
            name: self.hir.defs[ctor].name.to_string(),
            fields,
            tys,
        }
    }

    /// Converts a type checker type to an IR type.
    pub(super) fn ty(&mut self, ty: &Ty) -> TypeId {
        let t = match ty {
            Ty::Prim(p) => {
                return match p {
                    Prim::Int => TypeId::INT,
                    Prim::Int64 => TypeId::INT64,
                    Prim::Float => TypeId::FLOAT,
                    Prim::Bool => TypeId::BOOL,
                    Prim::String => TypeId::STRING,
                    Prim::Unit => TypeId::UNIT,
                };
            }
            Ty::Never => return TypeId::NEVER,
            // A type variable generalized in the definition being lowered is its type parameter.
            // One left undetermined (an unused `None`'s `Option[?]` and the like) can take any
            // representation. `Ty::Error` only appears when there are type errors, and then
            // lowering is not called.
            Ty::Var(v) => match self
                .current_def
                .and_then(|def| self.types.generalized_tys.get(def))
                .and_then(|vars| vars.iter().find(|(w, _)| w == v))
            {
                Some(&(_, i)) => Type::Param(i as usize),
                None => return TypeId::UNIT,
            },
            Ty::Error => return TypeId::UNIT,
            // Set type arguments of named types (`Handler[E, R]`) have no run-time representation,
            // but what they decide about calling conventions is kept.
            Ty::Set(_, row) => return self.set_ty(row),
            // A label's capability is a handler of its base (`EffectDef::label_of`). A capability
            // of several effects is the tuple of their handlers, in the order of `Ty::Cap`.
            Ty::Cap(tags, _) => {
                let mut elems: Vec<TypeId> = tags
                    .iter()
                    .map(|tag| {
                        let key = self.key_here(tag);
                        let id = self.effect_id(&key);
                        let id = self.module.handler_effect(id);
                        self.module.handler(id)
                    })
                    .collect();
                if elems.len() == 1 {
                    return elems.pop().expect("one effect");
                }
                Type::Tuple(elems)
            }
            Ty::Bound(i) => Type::Param(*i as usize),
            Ty::Fn(f) => {
                let params = f.params.iter().map(|t| self.ty(t)).collect();
                let ret = self.ty(&f.ret);
                Type::Fn(FnType {
                    params,
                    ret,
                    fails: !f.fails.is_empty(),
                    suspends: self.row_suspends(&f.uses),
                    // Function values always take the evidence table (`ty_takes_ev`).
                    ev: true,
                    uses: self.row_needs_ev(&f.uses),
                    sets: self.fn_sets(&f.fails, &f.uses),
                })
            }
            Ty::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.ty(t)).collect()),
            Ty::List(t) => Type::List(self.ty(t)),
            Ty::Named(con, args) => {
                let def = self.types.ty_table.def(*con);
                // The built-in types of `@intrinsic` declarations (`[api.std.bytes]`,
                // `[api.std.array]`), told apart by their type parameter: `Bytes` has none, and
                // `Array[A]` has the element type.
                if matches!(self.hir.item(def), Some(Item::Type(t)) if t.is_intrinsic) {
                    return match args.first() {
                        None => TypeId::BYTES,
                        Some(elem) => {
                            let elem = self.ty(elem);
                            self.module.intern(Type::Array(elem))
                        }
                    };
                }
                // The hidden capability sets of a declaration (`[type.rules]`) have no run-time
                // representation and decide nothing about calls.
                let visible = self
                    .types
                    .ty_table
                    .decl_caps(def)
                    .map_or(args.len(), |c| c.visible);
                match self.enums.get(def) {
                    Some(&id) => {
                        Type::Enum(id, args[..visible].iter().map(|t| self.ty(t)).collect())
                    }
                    // `self` of a stateless handler. The value is `()` (`Lowerer::path`).
                    None => return TypeId::UNIT,
                }
            }
        };
        self.module.intern(t)
    }

    /// The summary of a set inside the definition being lowered ([`SetSummary`]): set type
    /// parameters (`RowElem::Bound`, and set variables generalized by inference) are listed as
    /// `Type::Param`, and the flags come from the other elements by the same rules as the marks
    /// (a non-empty E fails, `row_needs_ev`, `row_suspends`). Error tags only make it fail and
    /// effect tags only set `ev` and `suspends`; a set variable left undetermined, whose kind is
    /// not known here, fails and needs the evidence table.
    pub(super) fn set_ty(&mut self, row: &Row) -> TypeId {
        let hir = self.hir;
        let mut set = SetSummary::default();
        for e in row.elems() {
            match e {
                RowElem::Bound(i) => set.params.push(self.module.type_param(*i as usize)),
                RowElem::Var(v) => match self.generalized_row_param(*v) {
                    Some(i) => set.params.push(self.module.type_param(i as usize)),
                    None => {
                        set.fails = true;
                        set.ev = true;
                    }
                },
                // The host provides a host effect and External, so they have no evidence
                // (`[effect.host]`, `[effect.external]`).
                RowElem::Tag(t) if matches!(hir.item(t.def), Some(Item::Effect(_))) => {
                    set.ev |= !hir.is_provided_by_host(t.def);
                    set.suspends |= !self.is_immediate_effect(t.def);
                }
                RowElem::Tag(_) | RowElem::Of(_) => set.fails = true,
                // A capability is reached through its value, not the evidence table; whether a
                // call may suspend is decided by its effect (`[effect.use]`). Its effect is kept
                // as needing the evidence table, as before capabilities were told apart.
                RowElem::Cap(c) => {
                    if matches!(hir.item(c.tag.def), Some(Item::Effect(_))) {
                        set.ev |= !hir.is_provided_by_host(c.tag.def);
                        set.suspends |= !self.is_immediate_effect(c.tag.def);
                    }
                }
            }
        }
        set.params.sort();
        set.params.dedup();
        self.module.intern(Type::Set(set))
    }

    /// The type parameter that the set variable `v` was generalized to in the definition being
    /// lowered, if any.
    pub(super) fn generalized_row_param(&self, v: emela_types::RowVar) -> Option<u32> {
        self.current_def
            .and_then(|def| self.types.generalized_rows.get(def))
            .and_then(|rows| rows.iter().find(|(w, _)| *w == v))
            .map(|&(_, i)| i)
    }

    /// The E and R of a function type, if they contain set type parameters (`FnType::sets`).
    pub(super) fn fn_sets(&mut self, fails: &Row, uses: &Row) -> Option<FnSets> {
        let errors = self.set_ty(fails);
        let effects = self.set_ty(uses);
        let open = |t: TypeId| matches!(self.module.ty(t), Type::Set(s) if !s.params.is_empty());
        (open(errors) || open(effects)).then_some(FnSets { errors, effects })
    }

    /// Whether the declared (or generalized) type's E is not empty. Opaque sets (set type
    /// parameters, generalized sets) count as not empty.
    pub(super) fn declared_fails(&self, def: DefId) -> bool {
        matches!(self.types.sigs.get(def).map(|s| &s.scheme.ty), Some(Ty::Fn(f)) if !f.fails.is_empty())
    }

    /// The IR type arguments of the reference `expr` to the generic definition `def`
    /// (`Callee::Direct::ty_args`): the type checker's instantiation (`ProgramTypes::type_args`),
    /// with set type parameters as summaries of the instantiated sets (`ProgramTypes::type_rows`,
    /// [`Lowerer::set_ty`]).
    ///
    /// A reference within a group being inferred is not instantiated (a self reference, or a
    /// reference to a definition whose scheme was not known yet). There the two definitions share
    /// the type variables that become type parameters, so each parameter of `def` is its generalized
    /// variable seen from the definition being lowered, and a self reference passes on its own
    /// parameters.
    /// The number of type arguments a constructor `def` gives its type at run time: the hidden
    /// capability sets of the type (`[type.rules]`) are left out.
    pub(super) fn visible_params(&self, def: DefId) -> usize {
        let owner = match self.hir.item(def) {
            Some(Item::Ctor(c)) => c.owner,
            _ => def,
        };
        match self.types.ty_table.decl_caps(owner) {
            Some(caps) => caps.visible,
            None => usize::MAX,
        }
    }

    pub(super) fn ty_args(&mut self, def: DefId, expr: ExprId) -> Vec<TypeId> {
        let types = self.types;
        let n = types.sigs.get(def).map_or(0, |s| s.scheme.params.len());
        if n == 0 {
            return Vec::new();
        }
        let params = &types
            .sigs
            .get(def)
            .expect("a generic definition has a signature")
            .scheme
            .params;
        let is_set = |i: usize| matches!(params[i].kind, emela_types::ParamKind::Set);
        let recorded = types.type_args.get(expr).map_or(&[][..], Vec::as_slice);
        if recorded.is_empty() && self.current_def == Some(def) {
            return (0..n)
                .map(|i| {
                    let p = self.module.type_param(i);
                    if is_set(i) {
                        self.module.intern(Type::Set(SetSummary {
                            params: vec![p],
                            ..SetSummary::default()
                        }))
                    } else {
                        p
                    }
                })
                .collect();
        }
        let mut out: Vec<TypeId> = recorded.iter().map(|t| self.ty(t)).collect();
        out.resize(n, TypeId::UNIT);
        for &(v, i) in types.generalized_tys.get(def).into_iter().flatten() {
            let i = i as usize;
            if i >= recorded.len() && i < n {
                out[i] = self.ty(&Ty::Var(v));
            }
        }
        // Set type parameters get the summary of the instantiated set (`ProgramTypes::type_rows`),
        // or, for a reference within a group being inferred, of the shared set variable.
        let rows = types.type_rows.get(expr);
        let generalized = types.generalized_rows.get(def);
        for (i, slot) in out.iter_mut().enumerate() {
            if !is_set(i) {
                continue;
            }
            let row = match rows.and_then(|rows| rows.get(i)) {
                Some(row) => row.clone(),
                None => match generalized
                    .into_iter()
                    .flatten()
                    .find(|&&(_, j)| j as usize == i)
                {
                    Some(&(v, _)) => Row::var(v),
                    // Not known here: may fail and need the evidence table, and suspends as the
                    // copy called does.
                    None => {
                        let spec = self.specialization_of(def, Some(expr));
                        let suspends = spec.checked_shr(i as u32).is_some_and(|b| b & 1 != 0);
                        *slot = self.module.intern(Type::Set(SetSummary {
                            suspends,
                            ..SetSummary::unknown()
                        }));
                        continue;
                    }
                },
            };
            *slot = self.set_ty(&row);
        }
        out
    }

    /// The type arguments of the const `def` compared with a value of type `ty` (a const pattern
    /// has no reference expression with recorded type arguments): read off by matching the
    /// const's declared type against `ty`.
    pub(super) fn const_ty_args(&mut self, def: DefId, ty: &Ty) -> Vec<TypeId> {
        let types = self.types;
        let Some(sig) = types.sigs.get(def) else {
            return Vec::new();
        };
        let mut found: Vec<Option<&Ty>> = vec![None; sig.scheme.params.len()];
        match_bound(&sig.scheme.ty, ty, &mut found);
        found
            .into_iter()
            .map(|t| t.map_or(TypeId::UNIT, |t| self.ty(t)))
            .collect()
    }

    pub(super) fn declare_enum(&mut self, def: DefId, params: usize) -> EnumId {
        let name = self.ir_name(def);
        self.module.add_enum(EnumDef {
            name,
            params,
            variants: Vec::new(),
            error: false,
            external: false,
        })
    }
}
