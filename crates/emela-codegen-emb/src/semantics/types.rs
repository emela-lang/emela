//! Canonical type interning and substitution over the independent backend catalog.
use crate::contract::ir::*;
use std::collections::HashMap;

impl Default for TypeTable {
    fn default() -> Self {
        let mut table = TypeTable {
            types: Vec::new(),
            has_param: Vec::new(),
            ids: HashMap::default(),
        };
        for t in [
            Type::Int,
            Type::Int64,
            Type::Float,
            Type::Bool,
            Type::String,
            Type::Unit,
            Type::Never,
            Type::Evidence,
            Type::FiberScope,
            Type::Any,
            Type::Bytes,
        ] {
            table.intern(t);
        }
        debug_assert_eq!(table.types[TypeId::ANY.index()], Type::Any);
        debug_assert_eq!(table.types[TypeId::BYTES.index()], Type::Bytes);
        table
    }
}

impl TypeTable {
    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(&id) = self.ids.get(&ty) {
            return id;
        }
        let id = TypeId::from_index(self.types.len());
        let has_param = match &ty {
            Type::Param(_) => true,
            Type::Tuple(ts) | Type::Enum(_, ts) => ts.iter().any(|&t| self.has_param(t)),
            Type::List(t) | Type::Cell(t) => self.has_param(*t),
            Type::Fn(f) => {
                f.params.iter().any(|&t| self.has_param(t))
                    || self.has_param(f.ret)
                    || f.sets.is_some()
            }
            Type::Set(s) => !s.params.is_empty(),
            _ => false,
        };
        self.ids.insert(ty.clone(), id);
        self.types.push(ty);
        self.has_param.push(has_param);
        id
    }
    pub fn enum_type(&mut self, id: EnumId, args: Vec<TypeId>) -> TypeId {
        self.intern(Type::Enum(id, args))
    }
    pub fn has_param(&self, ty: TypeId) -> bool {
        self.has_param[ty.index()]
    }
    pub fn subst(&mut self, ty: TypeId, args: &[TypeId]) -> TypeId {
        subst_type(self, ty, args)
    }
    pub fn erase_sets(&mut self, ty: TypeId) -> TypeId {
        let new = match self[ty].clone() {
            Type::Set(_) => return TypeId::UNIT,
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|&t| self.erase_sets(t)).collect()),
            Type::List(t) => Type::List(self.erase_sets(t)),
            Type::Enum(id, ts) => Type::Enum(id, ts.iter().map(|&t| self.erase_sets(t)).collect()),
            Type::Cell(t) => Type::Cell(self.erase_sets(t)),
            Type::Fn(f) => Type::Fn(FnType {
                params: f.params.iter().map(|&t| self.erase_sets(t)).collect(),
                ret: self.erase_sets(f.ret),
                sets: None,
                ..f
            }),
            _ => return ty,
        };
        self.intern(new)
    }
}

impl SetSummary {
    pub fn unknown() -> Self {
        SetSummary {
            fails: true,
            suspends: true,
            ev: true,
            params: Vec::new(),
        }
    }
    pub fn merge(&mut self, other: &SetSummary) {
        self.fails |= other.fails;
        self.suspends |= other.suspends;
        self.ev |= other.ev;
        self.params.extend_from_slice(&other.params);
    }
}

pub trait TypeStore {
    fn table(&self) -> &TypeTable;
    fn intern_type(&mut self, ty: Type) -> TypeId;
    /// Whether `ty` may change under substitution. By default, whether it contains a
    /// `Type::Param`.
    fn may_change(&mut self, ty: TypeId) -> bool {
        self.table().has_param(ty)
    }
    /// The effect that a capability type `Type::Handler(effect)` becomes. By default, the same.
    fn map_effect(&mut self, effect: EffectId) -> EffectId {
        effect
    }
}

impl TypeStore for TypeTable {
    fn table(&self) -> &TypeTable {
        self
    }

    fn intern_type(&mut self, ty: Type) -> TypeId {
        self.intern(ty)
    }
}

/// Replaces `Type::Param(i)` with `args[i]` in `ty`. A parameter without an argument is left as
/// is (only in malformed IR, which [`crate::verify`] reports).
///
/// Set type parameters are replaced inside [`Type::Set`]s: a parameter whose argument is a set
/// summary is merged into it, and one whose argument is something else (the `()` placeholder) is
/// read as an unknown set ([`SetSummary::unknown`]). A function type whose E and R become fully
/// known gets exact flags from them and no [`FnType::sets`]; one still with set type parameters
/// keeps its flags, raised by what became known ([`fn_with_sets`]).
pub fn subst_type<S: TypeStore + ?Sized>(s: &mut S, ty: TypeId, args: &[TypeId]) -> TypeId {
    if !s.may_change(ty) {
        return ty;
    }
    let new = match s.table()[ty].clone() {
        Type::Param(i) => return args.get(i).copied().unwrap_or(ty),
        Type::Tuple(ts) => Type::Tuple(subst_all(s, &ts, args)),
        Type::List(t) => Type::List(subst_type(s, t, args)),
        Type::Enum(id, ts) => Type::Enum(id, subst_all(s, &ts, args)),
        Type::Cell(t) => Type::Cell(subst_type(s, t, args)),
        Type::Handler(e) => Type::Handler(s.map_effect(e)),
        Type::Set(set) => Type::Set(subst_set(s.table(), &set, args)),
        Type::Fn(f) => {
            let params = subst_all(s, &f.params, args);
            let ret = subst_type(s, f.ret, args);
            match f.sets {
                None => Type::Fn(FnType { params, ret, ..f }),
                Some(sets) => {
                    let errors = subst_type(s, sets.errors, args);
                    let effects = subst_type(s, sets.effects, args);
                    Type::Fn(fn_with_sets(s.table(), params, ret, &f, errors, effects))
                }
            }
        }
        _ => return ty,
    };
    s.intern_type(new)
}

fn subst_all<S: TypeStore + ?Sized>(s: &mut S, tys: &[TypeId], args: &[TypeId]) -> Vec<TypeId> {
    tys.iter().map(|&t| subst_type(s, t, args)).collect()
}

fn subst_set(types: &TypeTable, set: &SetSummary, args: &[TypeId]) -> SetSummary {
    let mut out = SetSummary {
        params: Vec::new(),
        ..set.clone()
    };
    for &p in &set.params {
        let arg = match types[p] {
            Type::Param(i) => args.get(i).copied().unwrap_or(p),
            _ => p,
        };
        match &types[arg] {
            Type::Set(a) => out.merge(a),
            Type::Param(_) => out.params.push(arg),
            _ => out.merge(&SetSummary::unknown()),
        }
    }
    out.params.sort();
    out.params.dedup();
    out
}

/// A function type like `old` with parameters `params`, return type `ret`, E `errors` and R
/// `effects` (both `Type::Set`). If neither set has set type parameters left, the flags come from
/// them; otherwise `old`'s flags are kept, raised by the known parts.
pub fn fn_with_sets(
    types: &TypeTable,
    params: Vec<TypeId>,
    ret: TypeId,
    old: &FnType,
    errors: TypeId,
    effects: TypeId,
) -> FnType {
    let (Type::Set(e), Type::Set(r)) = (&types[errors], &types[effects]) else {
        unreachable!("the E and R of a function type are sets");
    };
    if e.params.is_empty() && r.params.is_empty() {
        FnType {
            params,
            ret,
            fails: e.fails,
            suspends: r.suspends,
            ev: r.ev,
            sets: None,
        }
    } else {
        FnType {
            params,
            ret,
            fails: old.fails || e.fails,
            suspends: old.suspends || r.suspends,
            ev: old.ev || r.ev,
            sets: Some(FnSets { errors, effects }),
        }
    }
}
