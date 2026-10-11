//! 宣言の型．fn・構成子・const の型（[`Scheme`]）と，引数のラベル．
//!
//! 呼び出しの推論が，呼ばれる側の型とラベルをここから引く．戻り値の型を書いていない fn と，
//! 型を書いていない const は，本体を推論した後に足す．

use emela_resolve::hir::{DefId, Fields, FnItem, Item, Param, Program, TypeId};
use la_arena::ArenaMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::lower::{TyTable, ty_params, type_param_defs};
use crate::scheme::Scheme;
use crate::ty::Ty;

/// 定義1つの型と引数のラベル．
pub struct Signature {
    pub scheme: Scheme,
    /// 引数ごとのラベル．位置だけのフィールドは `None`．関数でない定義では空．
    pub labels: Vec<Option<SmolStr>>,
}

/// fn・構成子・const の型の表．
pub struct Signatures {
    by_def: ArenaMap<DefId, Signature>,
}

impl Signatures {
    pub fn new() -> Self {
        Signatures {
            by_def: ArenaMap::default(),
        }
    }

    pub fn insert(&mut self, def: DefId, signature: Signature) {
        self.by_def.insert(def, signature);
    }

    pub fn get(&self, def: DefId) -> Option<&Signature> {
        self.by_def.get(def)
    }

    /// fn・構成子・const の宣言の型を集める．
    pub fn collect(program: &Program, ty_table: &TyTable, diags: &mut Vec<Diagnostic>) -> Self {
        let mut sigs = Signatures::new();

        for (def, _) in program.defs.iter() {
            let sig = match program.item(def) {
                Some(Item::Fn(item)) => fn_signature(program, ty_table, item, diags),
                Some(Item::Ctor(item)) => {
                    ctor_signature(program, ty_table, item.owner, &item.fields, diags)
                }
                Some(Item::Error(item)) => {
                    ctor_signature(program, ty_table, def, &item.fields, diags)
                }
                Some(Item::Const(item)) => item.ty.map(|ty| Signature {
                    scheme: Scheme::mono(ty_table.lower(program, &[], ty, diags)),
                    labels: Vec::new(),
                }),
                _ => None,
            };

            if let Some(sig) = sig {
                sigs.insert(def, sig);
            }
        }

        sigs
    }
}

impl Default for Signatures {
    fn default() -> Self {
        Self::new()
    }
}
/// fn の型とラベル．戻り値の型を書いていなければ `None`（本体から推論する）．
fn fn_signature(
    program: &Program,
    ty_table: &TyTable,
    item: &FnItem,
    diags: &mut Vec<Diagnostic>,
) -> Option<Signature> {
    let ret = item.sig.ret?;
    let defs: Vec<DefId> = item.sig.type_params.iter().map(|tp| tp.def).collect();

    let arg_tys: Vec<Ty> = item
        .sig
        .params
        .iter()
        .map(|p| param_ty(program, ty_table, &defs, p, diags))
        .collect();
    let labels = item
        .sig
        .params
        .iter()
        .map(|p| Some(program.locals[p.local].name.clone()))
        .collect();
    let ret_ty = ty_table.lower(program, &defs, ret, diags);

    Some(Signature {
        scheme: Scheme::new(ty_params(program, &defs), Ty::func(arg_tys, ret_ty)),
        labels,
    })
}

/// 構成子の型とラベル．`owner` は構成子が作る型（type / enum / error）．
/// handler の構成子は `None`（alpha.2）．
///
/// フィールドのない構成子（`None`，`Empty`）は関数でなく値なので，型は作る型そのものになる．
fn ctor_signature(
    program: &Program,
    ty_table: &TyTable,
    owner: DefId,
    fields: &Fields,
    diags: &mut Vec<Diagnostic>,
) -> Option<Signature> {
    let ret_ty = ty_table.self_ty(owner)?;
    let defs = type_param_defs(program, owner);

    // フィールドの型と，そのラベル．
    let (field_tys, labels): (Vec<TypeId>, Vec<Option<SmolStr>>) = match fields {
        Fields::None => (Vec::new(), Vec::new()),
        Fields::Positional(tys) => (tys.clone(), vec![None; tys.len()]),
        Fields::Named(fs) => fs.iter().map(|f| (f.ty, Some(f.name.clone()))).unzip(),
    };
    let ty = match fields {
        Fields::None => ret_ty,
        _ => Ty::func(ty_table.lower_all(program, &defs, field_tys, diags), ret_ty),
    };

    Some(Signature {
        scheme: Scheme::new(ty_params(program, &defs), ty),
        labels,
    })
}

/// トップレベルの fn の引数の型．書いていなければ E0316（6.1）を積んで `Ty::Error`．
pub(crate) fn param_ty(
    program: &Program,
    ty_table: &TyTable,
    type_params: &[DefId],
    param: &Param,
    diags: &mut Vec<Diagnostic>,
) -> Ty {
    match param.ty {
        Some(ty) => ty_table.lower(program, type_params, ty, diags),
        None => {
            let local = &program.locals[param.local];
            diags.push(Diagnostic {
                span: Some(local.span),
                kind: DiagnosticKind::MissingParamType {
                    param: local.name.clone(),
                },
            });
            Ty::Error
        }
    }
}
