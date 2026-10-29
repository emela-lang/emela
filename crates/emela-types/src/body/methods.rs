//! Field access and method calls (`[type.rules]`).
//!
//! `x.f` is a field of the type of x, or an operation of a capability value. Called, `x.f(a)` can
//! also be a method call: `M.f(x, a)` of the module M that defines the type of x, or
//! `Trait.f(x, a)` of a trait visible by name in the calling module.
//!
//! The callee `x.f` of a method call is typed as a reference to the function it finds: it gets
//! that function's instantiated type, type arguments and sets (`exprs`, `type_args`,
//! `type_rows`), and `methods` names the function. The `calls` of the call put x first, as for a
//! pipe. E and R then flow as for `M.f(x, a)`: x is the callee's sub-expression and is evaluated
//! first, and the callee's function type brings the function's E and R.

use emela_resolve::hir::{
    Arg, DefId, DefKind, DefModule, ExprId, ExprKind, Item, Label, Program, Res, Span, TypeKind,
};
use smol_str::SmolStr;

use super::{BodyInfer, FieldTy, fn_parts};
use crate::FnTy;
use crate::diagnostic::DiagnosticKind;
use crate::methods::{candidate_traits, defining_module, erase_args, takes_self, trait_fns};
use crate::signature::param_labels;
use crate::traits::head_def;
use crate::ty::Ty;

/// What `x.f` stands for.
pub(super) enum Dot {
    /// A field, an operation of a capability value, or `Ty::Error` once a diagnostic is
    /// reported: the type of the value.
    Value(Ty),
    /// The function a method call calls. Only for a callee.
    Method(DefId),
}

/// The field of a type with a given name.
enum Field {
    /// The field index and type.
    Found(usize, Ty),
    /// An opaque type outside its module, whose fields cannot be accessed (E0315).
    Opaque,
    /// The type has fields, but none with that name.
    Missing,
    /// A type without fields.
    NotARecord,
}

impl BodyInfer<'_> {
    /// Looks up `x.f` (`expr`), where x is `base` of type `base_ty`. `call` tells whether `x.f`
    /// is the callee of a call. In order (`[type.rules]`): (1) a field; (2) the fns of the module
    /// that defines the type whose first parameter has that type at its head; (3) the functions
    /// that take `self` of the traits that the type implements, among those visible by name. A
    /// field is taken when it is not called, without looking further. More than one candidate is
    /// E0335; a function found but not called is E0312.
    pub(super) fn infer_dot(
        &mut self,
        expr: ExprId,
        base: ExprId,
        base_ty: &Ty,
        name: &Label,
        call: bool,
    ) -> Dot {
        let base_ty = self.ctx.shallow_resolve(base_ty);
        match &base_ty {
            Ty::Cap(effect) => return Dot::Value(self.infer_op(expr, effect, name)),
            Ty::Error | Ty::Never => return Dot::Value(Ty::Error),
            Ty::Var(_) => {
                let kind = DiagnosticKind::FieldOnUnknownType {
                    field: name.name.clone(),
                    call,
                };
                return Dot::Value(self.field_error(name, kind));
            }
            _ => {}
        }
        let field = self.lookup_field(&base_ty, &name.name);
        if let Field::Found(index, ty) = &field {
            let (index, ty) = (*index, ty.clone());
            // E0334 keeps the fns of the module apart from the fields, so only traits can also
            // be found here.
            let traits = if call {
                self.trait_methods(&base_ty, &name.name)
            } else {
                Vec::new()
            };
            if traits.is_empty() {
                self.fields.insert(expr, index);
                return Dot::Value(ty);
            }
            return Dot::Value(self.ambiguous_method(base, &base_ty, name, true, &traits));
        }
        let mut found = self.module_methods(&base_ty, &name.name);
        found.extend(self.trait_methods(&base_ty, &name.name));
        let ty = self.ctx.resolve(&base_ty);
        let kind = match (found.as_slice(), &field) {
            ([def], _) if call => return Dot::Method(*def),
            ([_], _) => DiagnosticKind::MethodNotCalled {
                ty,
                name: name.name.clone(),
                receiver: self.receiver_name(base),
            },
            ([_, _, ..], _) => {
                return Dot::Value(self.ambiguous_method(base, &base_ty, name, false, &found));
            }
            ([], Field::Opaque) => DiagnosticKind::OpaqueField {
                ty,
                field: name.name.clone(),
            },
            ([], Field::Missing) => DiagnosticKind::NoFieldOrFunction {
                ty,
                name: name.name.clone(),
            },
            ([], _) => DiagnosticKind::NoFields {
                ty,
                field: name.name.clone(),
            },
        };
        Dot::Value(self.field_error(name, kind))
    }

    /// Types the call `x.f(args)` (`expr`) of the function `def` found for the callee `x.f`
    /// (`callee`) as `M.f(x, args)`. x (`base`) has already been inferred as `base_ty`, and is
    /// checked against the first parameter: only the head of its type was needed to find `def`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_method_call(
        &mut self,
        expr: ExprId,
        callee: ExprId,
        base: ExprId,
        base_ty: &Ty,
        def: DefId,
        name: &Label,
        args: &[Arg],
        span: Span,
    ) -> Ty {
        let program = self.program;
        self.check_external(def, name.span);
        let (ty, tys, rows) = self.instantiate_def(def, name.span);
        if !tys.is_empty() {
            self.type_args.insert(callee, tys);
        }
        if !rows.is_empty() {
            self.type_rows.insert(callee, rows);
        }
        let ty = self.open_fails(ty);
        self.exprs.insert(callee, ty.clone());
        self.methods.insert(callee, def);
        let f = match self.ctx.shallow_resolve(&ty) {
            Ty::Fn(f) if !f.params.is_empty() => *f,
            _ => {
                for arg in args {
                    self.infer_expr(arg.value);
                }
                return Ty::Error;
            }
        };
        let FnTy { params, ret, .. } = f;
        self.coerce_at(program.exprs[base].span, &params[0], base_ty);
        // A fn of the same group without a declared type has no signature yet; its labels are
        // the names of its parameters.
        let labels: Option<Vec<Option<SmolStr>>> = match self.sigs.get(def) {
            Some(sig) => Some(sig.labels.iter().skip(1).cloned().collect()),
            None => fn_parts(program, def).map(|p| param_labels(program, &p.params[1..])),
        };
        let slots = self.check_args(
            Some(&name.name),
            labels.as_deref(),
            &params[1..],
            args,
            span,
        );
        self.calls
            .insert(expr, std::iter::once(Some(base)).chain(slots).collect());
        ret
    }

    /// The field `name` of `ty`, if `ty` is a type with fields that can be accessed here.
    fn lookup_field(&mut self, ty: &Ty, name: &SmolStr) -> Field {
        let Ty::Named(id, args) = ty else {
            return Field::NotARecord;
        };
        let type_def = self.ty_table.def(*id);
        let data = &self.program.defs[type_def];
        let opaque = data.is_opaque && data.module != self.module;
        match self.field_ty(type_def, args, name) {
            FieldTy::NotARecord => Field::NotARecord,
            _ if opaque => Field::Opaque,
            FieldTy::Found(index, ty) => Field::Found(index, ty),
            FieldTy::NoSuchField => Field::Missing,
        }
    }

    /// The fns `name` of the module that defines the head of `ty`, whose first parameter has
    /// that head (step 2 of `[type.rules]`). Outside that module, only pub fns. The scope of the
    /// calling module is not looked at.
    fn module_methods(&mut self, ty: &Ty, name: &SmolStr) -> Vec<DefId> {
        let program = self.program;
        let Some(head) = head_def(program, self.ty_table, ty) else {
            return Vec::new();
        };
        let Some(module) = defining_module(program, head) else {
            return Vec::new();
        };
        let here = self.module == DefModule::Source(module);
        let mut found = Vec::new();
        for &def in &program.modules[module].items {
            let data = &program.defs[def];
            if data.kind == DefKind::Fn
                && data.name == *name
                && (data.is_pub || here)
                && self.first_param_head(def) == Some(head)
            {
                found.push(def);
            }
        }
        found
    }

    /// The head of the type of the first parameter of the fn `def`. For a fn whose type is not
    /// known yet, the type written in its declaration. Only `x.f` written without a call can
    /// meet such a fn: a method call gives the fns without a declared type an edge in the order
    /// of inference (`check::groups`).
    fn first_param_head(&mut self, def: DefId) -> Option<DefId> {
        let ty = match self.sigs.get(def) {
            Some(sig) => sig.scheme.ty.clone(),
            None => match self.pending_ty(def) {
                Some((ty, _)) => ty,
                None => return written_first_param(self.program, def),
            },
        };
        let Ty::Fn(f) = self.ctx.shallow_resolve(&ty) else {
            return None;
        };
        let first = self.ctx.shallow_resolve(f.params.first()?);
        head_def(self.program, self.ty_table, &first)
    }

    /// The functions `name` that take `self` of the traits that `ty` implements (step 3 of
    /// `[type.rules]`): among the traits visible by name in the calling module and, for a type
    /// parameter, the traits of its bounds and their supertraits (a bound includes its
    /// supertraits, `[trait.call]`).
    fn trait_methods(&self, ty: &Ty, name: &SmolStr) -> Vec<DefId> {
        let program = self.program;
        let DefModule::Source(module) = self.module else {
            return Vec::new();
        };
        let env = self.current.map(|d| self.def_params(d)).unwrap_or_default();
        let head = erase_args(ty);
        // The trait function is looked for before the impl: checking the impl marks a derived
        // impl as used (W0303).
        candidate_traits(program, self.traits, module, ty, &env)
            .into_iter()
            .filter_map(|t| {
                let f = trait_fns(program, t)
                    .into_iter()
                    .find(|&f| program.defs[f].name == *name)?;
                takes_self(self.sigs, f).then_some((t, f))
            })
            .filter(|&(t, _)| {
                self.traits
                    .satisfies(program, self.ty_table, &head, t, &env)
                    .is_ok()
            })
            .map(|(_, f)| f)
            .collect()
    }

    /// Reports more than one candidate for a method call (E0335). `field` tells whether a field
    /// is among them, besides the functions `fns`.
    fn ambiguous_method(
        &mut self,
        base: ExprId,
        ty: &Ty,
        name: &Label,
        field: bool,
        fns: &[DefId],
    ) -> Ty {
        let program = self.program;
        let qualified = fns
            .iter()
            .map(|&f| {
                let data = &program.defs[f];
                let owner = match (data.kind, data.parent, &data.module) {
                    (DefKind::TraitFn, Some(t), _) => program.defs[t].name.clone(),
                    (_, _, DefModule::Source(m)) => program.modules[*m].name.last().clone(),
                    _ => SmolStr::default(),
                };
                SmolStr::new(format!("{owner}.{}", data.name))
            })
            .collect();
        let kind = DiagnosticKind::AmbiguousMethod {
            ty: self.ctx.resolve(ty),
            name: name.name.clone(),
            field,
            qualified,
            receiver: self.receiver_name(base),
        };
        self.field_error(name, kind)
    }

    /// The definition a callee refers to: the definition of a `Path`, or the function that a
    /// method call calls.
    pub(super) fn callee_def(&self, callee: ExprId) -> Option<DefId> {
        match self.program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => Some(def),
            _ => self.methods.get(callee).copied(),
        }
    }

    /// The name of x in `x.f`, when it is a variable, for the help of a diagnostic.
    fn receiver_name(&self, base: ExprId) -> Option<SmolStr> {
        let program = self.program;
        match program.exprs[base].kind {
            ExprKind::Path(Res::Local(local)) => Some(program.locals[local].name.clone()),
            _ => None,
        }
    }
}

/// The definition of the type written for the first parameter of the fn `def`, if it is a
/// named type.
fn written_first_param(program: &Program, def: DefId) -> Option<DefId> {
    let Some(Item::Fn(item)) = program.item(def) else {
        return None;
    };
    let ty = item.sig.params.first()?.ty?;
    match program.types[ty].kind {
        TypeKind::Path {
            res: Res::Def(d), ..
        } if program.defs[d].kind != DefKind::TypeParam => Some(d),
        _ => None,
    }
}
