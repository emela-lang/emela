//! HIR の型の書き方（[`TypeRef`]）を [`Ty`] に変換する．
//!
//! type / enum / error の定義を [`TyCons`] に登録して型の表を作り，
//! 型の書き方をその表で引いて [`Ty`] にする．
//!
//! [`TypeRef`]: emela_resolve::hir::TypeRef

use emela_resolve::hir::{
    DefId, DefKind, EnumItem, Item, Program, Res, Span, TypeId, TypeItem, TypeKind, TypeParam,
    TypeRef,
};
use la_arena::ArenaMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ty::{Ty, TyConData, TyConId, TyConKind, TyCons, TyParam};

/// 名前付き型の表．type / enum / error の定義から作る．
pub struct TyTable {
    /// 登録した宣言．型の表示で名前を引くのに使う．
    pub cons: TyCons,
    /// 定義から，登録した宣言への対応．
    by_def: ArenaMap<DefId, TyConId>,
    /// Prelude の Bool の enum．TyCons に登録せず Ty::BOOL に写す．
    bool_def: Option<DefId>,
    /// 登録した宣言から，その定義への対応（`by_def` の逆）．
    def_of: ArenaMap<TyConId, DefId>,
}

impl TyTable {
    /// type / enum / error の定義を登録して表を作る．
    pub fn new(program: &Program) -> Self {
        let mut cons = TyCons::new();
        let mut by_def = ArenaMap::default();
        let mut def_of = ArenaMap::default();
        let bool_def = program.prelude_def("Bool");

        for (def, data) in program.defs.iter() {
            let kind = match data.kind {
                DefKind::Type => TyConKind::Type,
                DefKind::Enum => TyConKind::Enum,
                DefKind::Error => TyConKind::Error,
                _ => continue,
            };
            if bool_def == Some(def) {
                continue;
            }

            let id = cons.alloc(TyConData {
                name: data.name.clone(),
                kind,
                params: ty_params(program, &type_param_defs(program, def)),
            });

            by_def.insert(def, id);
            def_of.insert(id, def);
        }

        TyTable {
            cons,
            by_def,
            bool_def,
            def_of,
        }
    }

    /// type / enum / error の定義を登録した宣言．Bool と，それ以外の定義には `None`．
    pub fn con(&self, def: DefId) -> Option<TyConId> {
        self.by_def.get(def).copied()
    }

    /// 登録した宣言の定義（type / enum / error）．
    pub fn def(&self, id: TyConId) -> DefId {
        self.def_of[id]
    }

    /// 型の書き方を Ty にする．`params` は宣言の型引数で，`A` は位置 i の `Ty::Bound(i)` になる．
    pub fn lower(
        &self,
        program: &Program,
        params: &[DefId],
        ty: TypeId,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let type_ref = &program.types[ty];
        match &type_ref.kind {
            TypeKind::Missing => Ty::Error,
            TypeKind::Unit => Ty::UNIT,
            TypeKind::SelfType => {
                diags.push(Diagnostic {
                    span: type_ref.span,
                    kind: DiagnosticKind::Unsupported { what: "Self" },
                });
                Ty::Error
            }
            TypeKind::Tuple(elems) => {
                Ty::tuple(self.lower_all(program, params, elems.iter().copied(), diags))
            }
            TypeKind::Fn {
                params: fn_params,
                ret,
                ..
            } => {
                let ret_ty = self.lower_or_error(program, params, *ret, diags);
                let param_tys = self.lower_all(program, params, fn_params.iter().copied(), diags);
                Ty::func(param_tys, ret_ty)
            }
            TypeKind::Path { res, args } => {
                self.lower_path(program, params, type_ref, *res, args, diags)
            }
        }
    }

    /// 型の書き方の並びを順に Ty にする．
    pub fn lower_all(
        &self,
        program: &Program,
        params: &[DefId],
        tys: impl IntoIterator<Item = TypeId>,
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Ty> {
        tys.into_iter()
            .map(|ty| self.lower(program, params, ty, diags))
            .collect()
    }

    /// 書いていないかもしれない型を Ty にする．書いていなければ `Ty::Error`．
    pub fn lower_or_error(
        &self,
        program: &Program,
        params: &[DefId],
        ty: Option<TypeId>,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        match ty {
            Some(ty) => self.lower(program, params, ty, diags),
            None => Ty::Error,
        }
    }

    /// type / enum / error の定義が名指す型．型引数は `Bound(0)`，`Bound(1)`，… で埋める．
    /// Bool は `Ty::BOOL`．それ以外の定義（handler など）は `None`．
    pub fn self_ty(&self, def: DefId) -> Option<Ty> {
        if self.bool_def == Some(def) {
            return Some(Ty::BOOL);
        }

        let id = self.con(def)?;
        let n = self.cons[id].params.len();

        Some(Ty::named(id, (0..n).map(|i| Ty::Bound(i as u32))))
    }

    /// 名前の付いた型（`Int`，`List[A]`，`Option[Int]`，`A`）を Ty にする．
    fn lower_path(
        &self,
        program: &Program,
        params: &[DefId],
        type_ref: &TypeRef,
        res: Res,
        args: &[TypeId],
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let Res::Def(def) = res else {
            return Ty::Error;
        };
        let data = &program.defs[def];
        let span = type_ref.span;

        let expected = match data.kind {
            DefKind::BuiltinType if data.name == "List" => 1,
            DefKind::Type | DefKind::Enum | DefKind::Error => match self.by_def.get(def) {
                Some(&id) => self.cons[id].params.len(),
                None => 0,
            },
            _ => 0,
        };

        if !check_arity(&data.name, expected, args, span, diags) {
            return Ty::Error;
        }

        if self.bool_def == Some(def) {
            return Ty::BOOL;
        }

        match data.kind {
            DefKind::TypeParam => params
                .iter()
                .position(|&p| p == def)
                .map_or(Ty::Error, |i| Ty::Bound(i as u32)),
            DefKind::BuiltinType => match data.name.as_str() {
                "Int" => Ty::INT,
                "Int64" => Ty::INT64,
                "Float" => Ty::FLOAT,
                "String" => Ty::STRING,
                "Never" => Ty::Never,
                "List" => Ty::list(self.lower(program, params, args[0], diags)),
                _ => {
                    diags.push(Diagnostic {
                        span,
                        kind: DiagnosticKind::Unsupported {
                            what: "builtin type",
                        },
                    });
                    Ty::Error
                }
            },
            DefKind::Type | DefKind::Enum | DefKind::Error => {
                let Some(&id) = self.by_def.get(def) else {
                    return Ty::Error;
                };
                Ty::named(
                    id,
                    self.lower_all(program, params, args.iter().copied(), diags),
                )
            }
            _ => Ty::Error,
        }
    }
}

/// 型引数の数が `expected` なら true．違えば E0304 を足して false．
fn check_arity(
    name: &SmolStr,
    expected: usize,
    args: &[TypeId],
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) -> bool {
    if args.len() != expected {
        diags.push(Diagnostic {
            span,
            kind: DiagnosticKind::WrongTypeArgCount {
                name: name.clone(),
                expected,
                found: args.len(),
            },
        });
        false
    } else {
        true
    }
}

/// type / enum の型引数の定義．それ以外は空．
pub(crate) fn type_param_defs(program: &Program, def: DefId) -> Vec<DefId> {
    let type_params: &[TypeParam] = match program.item(def) {
        Some(Item::Type(TypeItem { type_params, .. }))
        | Some(Item::Enum(EnumItem { type_params, .. })) => type_params,
        _ => &[],
    };

    type_params.iter().map(|tp| tp.def).collect()
}

/// 型引数の定義の並びから，スキームと型の宣言に付ける名前の並びを作る．
pub(crate) fn ty_params(program: &Program, defs: &[DefId]) -> Vec<TyParam> {
    defs.iter()
        .map(|&def| TyParam::new(program.defs[def].name.clone()))
        .collect()
}
