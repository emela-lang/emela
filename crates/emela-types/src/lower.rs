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

pub struct TyTable {
    pub cons: TyCons,
    by_def: ArenaMap<DefId, TyConId>,
    bool_def: Option<DefId>,
}

impl TyTable {
    pub fn new(program: &Program) -> Self {
        let mut cons = TyCons::new();
        let mut by_def = ArenaMap::default();
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

            let type_params: &[TypeParam] = match program.item(def) {
                Some(Item::Type(TypeItem { type_params, .. }))
                | Some(Item::Enum(EnumItem { type_params, .. })) => type_params,
                _ => &[],
            };

            let params = type_params
                .iter()
                .map(|tp| TyParam::new(program.defs[tp.def].name.clone()))
                .collect();

            let id = cons.alloc(TyConData {
                name: data.name.clone(),
                kind,
                params,
            });

            by_def.insert(def, id);
        }

        TyTable {
            cons,
            by_def,
            bool_def,
        }
    }

    pub fn lower(
        &self,
        program: &Program,
        params: &[DefId],
        ty: TypeId,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let type_def = &program.types[ty];
        match &type_def.kind {
            TypeKind::Missing => Ty::Error,
            TypeKind::Unit => Ty::UNIT,
            TypeKind::SelfType => {
                diags.push(Diagnostic {
                    span: type_def.span,
                    kind: DiagnosticKind::Unsupported { what: "Self" },
                });
                Ty::Error
            }
            TypeKind::Tuple(elems) => Ty::tuple(
                elems
                    .iter()
                    .map(|&elem| self.lower(program, params, elem, diags)),
            ),
            TypeKind::Fn {
                params: fn_params,
                ret,
                ..
            } => {
                let ret_ty = match ret {
                    Some(ty) => self.lower(program, params, *ty, diags),
                    None => Ty::Error,
                };
                Ty::func(
                    fn_params
                        .iter()
                        .map(|&p| self.lower(program, params, p, diags)),
                    ret_ty,
                )
            }
            TypeKind::Path { res, args } => {
                self.lower_path(program, params, type_def, *res, args, diags)
            }
        }
    }

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
                    args.iter()
                        .map(|&elem| self.lower(program, params, elem, diags)),
                )
            }
            _ => Ty::Error,
        }
    }
}

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
