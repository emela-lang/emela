//! HIR の型の書き方（[`TypeRef`]）を [`Ty`] に変換する．
//!
//! type / enum / error の定義を [`TyCons`] に登録して型の表を作り，
//! 型の書き方をその表で引いて [`Ty`] にする．
//!
//! [`TypeRef`]: emela_resolve::hir::TypeRef

use emela_resolve::hir::{
    DefId, DefKind, EnumItem, ErrorSet, Item, Program, Res, Span, TypeId, TypeItem, TypeKind,
    TypeParam, TypeRef,
};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::InferCtx;
use crate::row::{Row, RowElem};
use crate::ty::{ParamKind, Ty, TyConData, TyConId, TyConKind, TyCons, TyParam};

/// 関数型の `fails` 節を省略したときの意味．書いた位置で決まる（9.2）．
pub(crate) enum Omitted<'a> {
    /// 宣言の中（フィールド，const など）．空．
    Empty,
    /// トップレベルの fn の引数の型の中．見えない集合の型引数を足す．
    Hidden(&'a mut Hidden),
    /// 戻り値と束縛の型注釈，ラムダの引数．本体から推論するので，新しい集合変数を置く．
    Fresh(&'a mut InferCtx),
}

/// 引数の型の中の見えない集合の型引数（9.2）．書いた型引数の後ろに並べる．
pub(crate) struct Hidden {
    /// 書いた型引数の数．見えない集合の型引数の添字はこの後ろから振る．
    base: usize,
    /// 足した見えない集合の型引数．
    pub(crate) params: Vec<TyParam>,
    /// 今の引数の名前．見えない集合の型引数の名前（`E(f)`）に使う．
    param: SmolStr,
    /// 今の引数で足した数．
    in_param: usize,
    /// 引数の型の中の正の位置（本体が呼べる関数）に置いた見えない集合．宣言の E に含める．
    pub(crate) positive: Row,
}

impl Hidden {
    pub(crate) fn new(base: usize) -> Self {
        Hidden {
            base,
            params: Vec::new(),
            param: SmolStr::default(),
            in_param: 0,
            positive: Row::empty(),
        }
    }

    /// これから lower する引数の名前．
    pub(crate) fn start_param(&mut self, name: SmolStr) {
        self.param = name;
        self.in_param = 0;
    }

    fn fresh(&mut self, positive: bool) -> Row {
        self.in_param += 1;
        let name = match self.in_param {
            1 => format!("E({})", self.param),
            n => format!("E({}, {n})", self.param),
        };
        let i = (self.base + self.params.len()) as u32;
        self.params.push(TyParam::set(name));
        let row = Row::from_elems([RowElem::Bound(i)]);
        if positive {
            self.positive.extend(&row);
        }
        row
    }
}

/// 型の書き方を読む文脈．型引数（`params[i]` が `Bound(i)`）とその種類，`fails` を省略したときの意味．
pub(crate) struct Scope<'a> {
    pub(crate) params: &'a [DefId],
    pub(crate) kinds: &'a [ParamKind],
    pub(crate) omitted: Omitted<'a>,
    /// 集合の型引数を書けるか．type などの宣言の型引数は集合にできない（まだ扱わない）．
    pub(crate) sets: bool,
}

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
                DefKind::Error => {
                    cons.set_tag_name(def, data.name.clone());
                    TyConKind::Error
                }
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

    /// error の名前付き型 → その error の定義．
    pub(crate) fn error_tags(&self) -> FxHashMap<TyConId, DefId> {
        self.cons
            .iter()
            .filter(|(_, data)| data.kind == TyConKind::Error)
            .map(|(id, _)| (id, self.def_of[id]))
            .collect()
    }

    /// 型の書き方を Ty にする．`params` は宣言の型引数で，`A` は位置 i の `Ty::Bound(i)` になる．
    /// 宣言の中の型として読む（関数型の `fails` を省略すれば空，型引数はふつうの型）．
    pub fn lower(
        &self,
        program: &Program,
        params: &[DefId],
        ty: TypeId,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let kinds = vec![ParamKind::Type; params.len()];
        let mut scope = Scope {
            params,
            kinds: &kinds,
            omitted: Omitted::Empty,
            sets: false,
        };
        self.lower_in(program, &mut scope, ty, true, diags)
    }

    /// 型の書き方を，文脈 `scope` で Ty にする．`positive` は本体から見て値を受け取る位置か
    /// （関数型の引数の中で反転する）．見えない集合の型引数を宣言の E に含めるかに使う．
    pub(crate) fn lower_in(
        &self,
        program: &Program,
        scope: &mut Scope<'_>,
        ty: TypeId,
        positive: bool,
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
            TypeKind::Tuple(elems) => Ty::tuple(
                elems
                    .iter()
                    .map(|&e| self.lower_in(program, scope, e, positive, diags))
                    .collect::<Vec<_>>(),
            ),
            TypeKind::Fn {
                params: fn_params,
                ret,
                fails,
                ..
            } => {
                // 見えない集合の型引数は外側の関数型から振る（`f: fn(fn() -> A) -> B` の
                // f 自身の E が `E(f)`）．
                let fails = match fails {
                    Some(set) => self.lower_error_set(program, scope, set, diags),
                    None => match &mut scope.omitted {
                        Omitted::Empty => Row::empty(),
                        Omitted::Hidden(hidden) => hidden.fresh(positive),
                        Omitted::Fresh(ctx) => Row::var(ctx.new_row_var()),
                    },
                };
                let ret_ty = match ret {
                    Some(ret) => self.lower_in(program, scope, *ret, positive, diags),
                    None => Ty::Error,
                };
                let param_tys: Vec<Ty> = fn_params
                    .iter()
                    .map(|&p| self.lower_in(program, scope, p, !positive, diags))
                    .collect();
                Ty::func_fails(param_tys, ret_ty, fails)
            }
            TypeKind::Path { res, args } => {
                self.lower_path(program, scope, type_ref, *res, args, positive, diags)
            }
        }
    }

    /// `fails` 節の集合．エラーはタグ，集合の型引数は `RowElem::Bound`，値にも使う型引数は
    /// そのエラーの型（`RowElem::Of`）．
    pub(crate) fn lower_error_set(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        set: &ErrorSet,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let mut row = Row::empty();
        for &(res, span) in &set.items {
            let Res::Def(def) = res else {
                continue;
            };
            match program.defs[def].kind {
                DefKind::Error => row.insert(RowElem::Tag(def)),
                DefKind::TypeParam => {
                    let Some(i) = scope.params.iter().position(|&p| p == def) else {
                        continue;
                    };
                    match scope.kinds.get(i) {
                        // type などの宣言の型引数は集合にできない（まだ扱わない）．
                        Some(ParamKind::Set) if !scope.sets => {
                            diags.push(Diagnostic {
                                span: Some(span),
                                kind: DiagnosticKind::Unsupported {
                                    what: "error set type parameters on type declarations",
                                },
                            });
                        }
                        Some(ParamKind::Set) => row.insert(RowElem::Bound(i as u32)),
                        _ => row.insert(RowElem::Of(Ty::Bound(i as u32))),
                    }
                }
                _ => {}
            }
        }
        row
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
    #[allow(clippy::too_many_arguments)]
    fn lower_path(
        &self,
        program: &Program,
        scope: &mut Scope<'_>,
        type_ref: &TypeRef,
        res: Res,
        args: &[TypeId],
        positive: bool,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let params = scope.params;
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
                "List" => Ty::list(self.lower_in(program, scope, args[0], positive, diags)),
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
                let args: Vec<Ty> = args
                    .iter()
                    .map(|&a| self.lower_in(program, scope, a, positive, diags))
                    .collect();
                Ty::named(id, args)
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

/// 型引数の種類を，使われた位置から決める（9.2）．`fails` 節にだけ書いた型引数はエラーの集合，
/// それ以外はふつうの型（値の型と `fails` の両方に書けば1つのエラーの型，7.2）．
/// `types` は型引数が現れうる型の書き方（引数，戻り値，フィールドの型）．
pub(crate) fn classify_params(
    program: &Program,
    params: &[DefId],
    types: impl IntoIterator<Item = TypeId>,
) -> Vec<ParamKind> {
    let mut in_type = vec![false; params.len()];
    let mut in_set = vec![false; params.len()];
    for ty in types {
        mark_params(program, params, ty, &mut in_type, &mut in_set);
    }
    in_type
        .iter()
        .zip(&in_set)
        .map(|(&t, &s)| {
            if s && !t {
                ParamKind::Set
            } else {
                ParamKind::Type
            }
        })
        .collect()
}

fn mark_params(
    program: &Program,
    params: &[DefId],
    ty: TypeId,
    in_type: &mut [bool],
    in_set: &mut [bool],
) {
    let go = |ty: TypeId, in_type: &mut [bool], in_set: &mut [bool]| {
        mark_params(program, params, ty, in_type, in_set)
    };
    match &program.types[ty].kind {
        TypeKind::Path { res, args } => {
            if let Res::Def(def) = res
                && let Some(i) = params.iter().position(|p| p == def)
            {
                in_type[i] = true;
            }
            for &a in args {
                go(a, in_type, in_set);
            }
        }
        TypeKind::Tuple(elems) => {
            for &e in elems {
                go(e, in_type, in_set);
            }
        }
        TypeKind::Fn {
            params: fn_params,
            ret,
            fails,
            ..
        } => {
            for &p in fn_params {
                go(p, in_type, in_set);
            }
            if let Some(ret) = ret {
                go(*ret, in_type, in_set);
            }
            for (res, _) in fails.iter().flat_map(|f| &f.items) {
                if let Res::Def(def) = res
                    && let Some(i) = params.iter().position(|p| p == def)
                {
                    in_set[i] = true;
                }
            }
        }
        TypeKind::SelfType | TypeKind::Unit | TypeKind::Missing => {}
    }
}

/// 型の書き方の中に，`fails` 節を省略した関数型があるか．戻り値の型にあれば，
/// その E は本体から推論する（9.2）．
pub(crate) fn has_omitted_fails(program: &Program, ty: TypeId) -> bool {
    match &program.types[ty].kind {
        TypeKind::Path { args, .. } => args.iter().any(|&a| has_omitted_fails(program, a)),
        TypeKind::Tuple(elems) => elems.iter().any(|&e| has_omitted_fails(program, e)),
        TypeKind::Fn { fails: None, .. } => true,
        TypeKind::Fn { params, ret, .. } => {
            params.iter().any(|&p| has_omitted_fails(program, p))
                || ret.is_some_and(|r| has_omitted_fails(program, r))
        }
        TypeKind::SelfType | TypeKind::Unit | TypeKind::Missing => false,
    }
}
