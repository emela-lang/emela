//! 宣言の型．fn・構成子・const の型（[`Scheme`]）と，引数のラベル．
//!
//! 呼び出しの推論が，呼ばれる側の型とラベルをここから引く．戻り値の型を書いていない fn と，
//! `use` 節を書いていない本体のある fn と，型を書いていない const は，本体を推論した後に足す．
//! effect の操作の型もここに置く（能力の値を通した呼び出しが引く）．

use emela_resolve::hir::{
    DefId, DefKind, Fields, FnItem, FnSig, Item, Param, Program, Res, TypeId, TypeKind,
};
use la_arena::ArenaMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind, ImmediateSource};
use crate::lower::{
    Hidden, IMMEDIATE, Omitted, Scope, TyTable, classify_params, classify_sets, has_omitted_fails,
    has_omitted_uses, immediate_bounds, is_immediate_effect, ty_params, type_param_defs,
};
use crate::row::{Row, RowElem, SetKind};
use crate::scheme::Scheme;
use crate::ty::{ParamKind, Ty, TyParam};

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

    /// fn・構成子・const・effect の操作の宣言の型を集める．
    pub fn collect(program: &Program, ty_table: &TyTable, diags: &mut Vec<Diagnostic>) -> Self {
        let mut sigs = Signatures::new();

        for (def, _) in program.defs.iter() {
            let sig = match program.item(def) {
                Some(Item::Fn(item)) => {
                    check_bounds(program, ty_table, def, item, diags);
                    fn_signature(program, ty_table, item, diags)
                }
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
                Some(Item::Op(item)) => Some(op_signature(program, ty_table, &item.sig, diags)),
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
/// 戻り値の型の中に `fails` か `use` を省略した関数型があるときも，その E と R を本体から
/// 推論するので `None`．本体のある fn で `use` 節を書いていなければ，R を推論するので `None`
/// （9.3）．本体のない fn（`@external`，`@intrinsic`）の `use` を省略すれば R は空．
fn fn_signature(
    program: &Program,
    ty_table: &TyTable,
    item: &FnItem,
    diags: &mut Vec<Diagnostic>,
) -> Option<Signature> {
    let ret = item.sig.ret?;
    if has_omitted_fails(program, ret)
        || has_omitted_uses(program, ret)
        || (item.body.is_some() && item.sig.uses.is_none())
    {
        return None;
    }
    let mut params = FnParams::new(program, ty_table, item);
    let arg_tys = params.lower_params(program, ty_table, &item.sig.params, diags);
    let labels = param_labels(program, &item.sig.params);
    let mut scope = params.scope(Omitted::Empty);
    let ret_ty = ty_table.lower_in(program, &mut scope, ret, true, diags);
    let fails = params.declared_fails(program, ty_table, &item.sig, diags);
    let mut uses = params.declared_uses(program, ty_table, &item.sig, diags);
    // `@external suspend fn` は Async を要求する（12.2）．
    if item.sig.is_suspend
        && let Some(async_) = program.prelude_def("Async")
    {
        uses.insert(RowElem::Tag(async_));
    }

    Some(Signature {
        scheme: Scheme::new(
            params.all_params(),
            Ty::func_sets(arg_tys, ret_ty, fails, uses),
        ),
        labels,
    })
}

/// fn の型引数の制約を確かめる（9.5）．制約は Immediate だけを扱う．
///
/// - Immediate は効果の集合（`use` 節に書いた型引数）にだけ付けられる（E0322）．
/// - 効果の集合に Immediate 以外の制約（Trait）を付けたら「まだ扱えない」．ふつうの型の型引数の
///   Trait の制約は 0.21 まで検査しない（Prelude の `dbg[A: Show]`）．
/// - 外部関数の引数の関数型に書いた `use` 節は Immediate でなければならない（E0416）．省略した
///   `use` 節は見えない集合で，Immediate の制約を付ける（[`FnParams::new`]）．
fn check_bounds(
    program: &Program,
    ty_table: &TyTable,
    def: DefId,
    item: &FnItem,
    diags: &mut Vec<Diagnostic>,
) {
    let sig = &item.sig;
    let types: Vec<TypeId> = sig
        .params
        .iter()
        .filter_map(|p| p.ty)
        .chain(sig.ret)
        .collect();
    let immediate = program.prelude_def(IMMEDIATE);
    let mut constrained = Vec::new();
    let defs: Vec<DefId> = sig.type_params.iter().map(|tp| tp.def).collect();
    let sets = classify_sets(program, ty_table.decl_sets(), &defs, types.iter().copied());
    for (i, tp) in sig.type_params.iter().enumerate() {
        let in_uses = sets[i] == Some(SetKind::Effects)
            || sig
                .uses
                .iter()
                .flat_map(|u| &u.items)
                .any(|(res, _)| *res == Res::Def(tp.def));
        let spans = immediate_bounds(program, tp);
        if !in_uses {
            for span in spans {
                diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::ImmediateOnNonEffect {
                        param: program.defs[tp.def].name.clone(),
                    },
                });
            }
            continue;
        }
        if !spans.is_empty() {
            constrained.push(tp.def);
        }
        for &(res, span) in &tp.bounds {
            if matches!(res, Res::Def(d) if Some(d) != immediate) {
                diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::Unsupported {
                        what: "trait bounds on effect sets",
                    },
                });
            }
        }
    }
    if !item.is_external {
        return;
    }
    // 外部関数のコールバックの `use` 節．
    fn walk(
        program: &Program,
        ty: TypeId,
        constrained: &[DefId],
        effects: &mut Vec<SmolStr>,
        sets: &mut Vec<SmolStr>,
    ) {
        match &program.types[ty].kind {
            TypeKind::Path { args, .. } => {
                for &a in args {
                    walk(program, a, constrained, effects, sets);
                }
            }
            TypeKind::Tuple(elems) => {
                for &e in elems {
                    walk(program, e, constrained, effects, sets);
                }
            }
            TypeKind::Fn {
                params, ret, uses, ..
            } => {
                for &(res, _) in uses.iter().flat_map(|u| &u.items) {
                    let Res::Def(d) = res else {
                        continue;
                    };
                    let name = program.defs[d].name.clone();
                    let list = match program.defs[d].kind {
                        DefKind::Effect if !is_immediate_effect(program, d) => &mut *effects,
                        DefKind::TypeParam if !constrained.contains(&d) => &mut *sets,
                        _ => continue,
                    };
                    if !list.contains(&name) {
                        list.push(name);
                    }
                }
                for &p in params {
                    walk(program, p, constrained, effects, sets);
                }
                if let Some(r) = ret {
                    walk(program, *r, constrained, effects, sets);
                }
            }
            TypeKind::SelfType
            | TypeKind::Unit
            | TypeKind::Missing
            | TypeKind::ErrorSet(_)
            | TypeKind::EffectSet(_) => {}
        }
    }
    for p in &sig.params {
        let Some(ty) = p.ty else {
            continue;
        };
        let (mut effects, mut sets) = (Vec::new(), Vec::new());
        walk(program, ty, &constrained, &mut effects, &mut sets);
        if effects.is_empty() && sets.is_empty() {
            continue;
        }
        diags.push(Diagnostic {
            span: program.types[ty].span,
            kind: DiagnosticKind::NotImmediate {
                effects,
                sets,
                required: ImmediateSource::ExternalCallback {
                    callee: program.defs[def].name.clone(),
                },
            },
        });
    }
}

/// effect の操作の型とラベル（8.1）．宣言の中なので，引数の関数型の `fails` と `use` を
/// 省略すれば空（9.2）．操作の `fails` 節はその操作の E．操作の型の R は空で，能力の値を通して
/// 呼ぶときにそのエフェクトを足す．操作に `use` 節は書けない（構文の段の E0137）．
fn op_signature(
    program: &Program,
    ty_table: &TyTable,
    sig: &FnSig,
    diags: &mut Vec<Diagnostic>,
) -> Signature {
    let mut scope = Scope {
        params: &[],
        kinds: &[],
        omitted: Omitted::Empty,
        sets: false,
    };
    let params: Vec<Ty> = sig
        .params
        .iter()
        .map(|p| match p.ty {
            Some(ty) => ty_table.lower_in(program, &mut scope, ty, true, diags),
            None => param_ty(program, ty_table, &[], p, diags),
        })
        .collect();
    let ret = match sig.ret {
        Some(ret) => ty_table.lower_in(program, &mut scope, ret, true, diags),
        None => Ty::UNIT,
    };
    let fails = match &sig.fails {
        Some(set) => ty_table.lower_error_set(program, &scope, set, diags),
        None => Row::empty(),
    };
    // 操作の `use` 節は構文の段の E0137（8.1）．ここでは読まない．
    Signature {
        scheme: Scheme::mono(Ty::func_fails(params, ret, fails)),
        labels: param_labels(program, &sig.params),
    }
}

/// 引数の名前のラベル．
pub(crate) fn param_labels(program: &Program, params: &[Param]) -> Vec<Option<SmolStr>> {
    params
        .iter()
        .map(|p| Some(program.locals[p.local].name.clone()))
        .collect()
}

/// fn の型引数．書いた型引数（種類は使われた位置で決める）と，引数の型の中の見えない集合の型引数．
pub(crate) struct FnParams {
    /// 書いた型引数の定義．
    pub(crate) defs: Vec<DefId>,
    pub(crate) kinds: Vec<ParamKind>,
    /// 書いた型引数．
    declared: Vec<TyParam>,
    pub(crate) hidden: Hidden,
}

impl FnParams {
    /// fn の型引数．外部関数なら，引数の型の中の見えない効果の集合に `Immediate` の制約を付ける
    /// （コールバックは Immediate でなければならない，9.5）．
    pub(crate) fn new(program: &Program, ty_table: &TyTable, item: &FnItem) -> Self {
        let sig = &item.sig;
        let defs: Vec<DefId> = sig.type_params.iter().map(|tp| tp.def).collect();
        let types = sig.params.iter().filter_map(|p| p.ty).chain(sig.ret);
        let kinds = classify_params(program, ty_table.decl_sets(), &defs, types);
        // 制約は Immediate だけを持つ（Trait は 0.21．名前も持たない）．効果の集合でない型引数に
        // 付けた Immediate は [`check_bounds`] が E0322 にする．
        let declared = ty_params(program, &defs)
            .into_iter()
            .zip(&kinds)
            .zip(&sig.type_params)
            .map(|((p, &kind), tp)| {
                let mut p = TyParam { kind, ..p };
                if kind == ParamKind::Set && !immediate_bounds(program, tp).is_empty() {
                    p.bounds.push(SmolStr::new(IMMEDIATE));
                }
                p
            })
            .collect();
        let mut hidden = Hidden::new(defs.len());
        hidden.immediate = item.is_external;
        FnParams {
            defs,
            kinds,
            declared,
            hidden,
        }
    }

    pub(crate) fn scope<'a>(&'a self, omitted: Omitted<'a>) -> Scope<'a> {
        Scope {
            params: &self.defs,
            kinds: &self.kinds,
            omitted,
            sets: true,
        }
    }

    /// 引数の型．`fails` を省略した関数型には見えない集合の型引数を置く．
    /// 型を書いていない引数は E0316（6.1）を積んで `Ty::Error`．
    pub(crate) fn lower_params(
        &mut self,
        program: &Program,
        ty_table: &TyTable,
        params: &[Param],
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Ty> {
        params
            .iter()
            .map(|p| {
                let Some(ty) = p.ty else {
                    return param_ty(program, ty_table, &self.defs, p, diags);
                };
                self.hidden
                    .start_param(program.locals[p.local].name.clone());
                let mut scope = Scope {
                    params: &self.defs,
                    kinds: &self.kinds,
                    omitted: Omitted::Hidden(&mut self.hidden),
                    sets: true,
                };
                ty_table.lower_in(program, &mut scope, ty, true, diags)
            })
            .collect()
    }

    /// 宣言の E．書いた `fails` 節（なければ空）と，引数の型の中の正の位置の見えない集合の和．
    /// 見えない集合は書けないので，引数の関数を呼んだときのエラーはいつも伝えてよい（9.2）．
    pub(crate) fn declared_fails(
        &self,
        program: &Program,
        ty_table: &TyTable,
        sig: &FnSig,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let written = match &sig.fails {
            Some(set) => ty_table.lower_error_set(program, &self.scope(Omitted::Empty), set, diags),
            None => Row::empty(),
        };
        written.union(&self.hidden.positive)
    }

    /// 宣言の R．書いた `use` 節と，引数の型の中の正の位置の見えない効果の集合の和．
    /// `use` 節を書いていなければ R は本体から推論するので，これは使わない．
    pub(crate) fn declared_uses(
        &self,
        program: &Program,
        ty_table: &TyTable,
        sig: &FnSig,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let written = match &sig.uses {
            Some(set) => {
                ty_table.lower_effect_set(program, &self.scope(Omitted::Empty), set, diags)
            }
            None => Row::empty(),
        };
        written.union(&self.hidden.positive_uses)
    }

    /// 書いた型引数と見えない集合の型引数を並べたもの．
    pub(crate) fn all_params(&self) -> Vec<TyParam> {
        self.declared
            .iter()
            .chain(&self.hidden.params)
            .cloned()
            .collect()
    }
}

/// 構成子の型とラベル．`owner` は構成子が作る型（type / enum / error / handler）．
/// handler の構成子はハンドラの値を作る（8.4）．使えるのはその handler の init の中だけ（E0418）．
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
    // フィールドの関数型の `fails` と `use` を省略すれば空（9.2）．型引数の種類（集合か）は
    // 型の表が決めたもの（`type Handler[E, R]`）．
    let params: Vec<TyParam> = match ty_table.con(owner) {
        Some(id) => ty_table.cons[id].params.clone(),
        None => ty_params(program, &defs),
    };
    let kinds: Vec<ParamKind> = params.iter().map(|p| p.kind).collect();
    let mut scope = Scope {
        params: &defs,
        kinds: &kinds,
        omitted: Omitted::Empty,
        sets: true,
    };
    let ty = match fields {
        Fields::None => ret_ty,
        _ => Ty::func(
            field_tys
                .iter()
                .map(|&t| ty_table.lower_in(program, &mut scope, t, true, diags))
                .collect::<Vec<_>>(),
            ret_ty,
        ),
    };

    Some(Signature {
        scheme: Scheme::new(params, ty),
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
