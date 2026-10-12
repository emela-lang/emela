//! HIR の型の書き方（[`TypeRef`]）を [`Ty`] に変換する．
//!
//! type / enum / error の定義を [`TyCons`] に登録して型の表を作り，
//! 型の書き方をその表で引いて [`Ty`] にする．
//!
//! [`TypeRef`]: emela_resolve::hir::TypeRef

use emela_resolve::hir::{
    DefId, DefKind, EffectSet, EnumItem, ErrorSet, Fields, Item, Program, Res, Span, TypeId,
    TypeItem, TypeKind, TypeParam, TypeRef,
};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::InferCtx;
use crate::row::{Row, RowElem, SetKind};
use crate::ty::{ParamKind, Ty, TyConData, TyConId, TyConKind, TyCons, TyParam};

/// 関数型の `fails` 節と `use` 節を省略したときの意味．書いた位置で決まる（9.2）．
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
    /// 今の引数の名前．見えない集合の型引数の名前（`E(f)`，`R(f)`）に使う．
    param: SmolStr,
    /// 今の引数で足したエラーの集合と効果の集合の数．
    in_param: [usize; 2],
    /// 引数の型の中の正の位置（本体が呼べる関数）に置いた見えないエラーの集合．宣言の E に含める．
    pub(crate) positive: Row,
    /// 同じく，見えない効果の集合．宣言の R に含める．
    pub(crate) positive_uses: Row,
    /// 見えない効果の集合に `Immediate` の制約を付けるか．外部関数のコールバックは
    /// Immediate でなければならない（9.5）．
    pub(crate) immediate: bool,
}

impl Hidden {
    pub(crate) fn new(base: usize) -> Self {
        Hidden {
            base,
            params: Vec::new(),
            param: SmolStr::default(),
            in_param: [0, 0],
            positive: Row::empty(),
            positive_uses: Row::empty(),
            immediate: false,
        }
    }

    /// これから lower する引数の名前．
    pub(crate) fn start_param(&mut self, name: SmolStr) {
        self.param = name;
        self.in_param = [0, 0];
    }

    /// 見えない集合の型引数を足す．`effects` なら効果の集合（`R(f)`），でなければエラーの集合（`E(f)`）．
    fn fresh(&mut self, positive: bool, effects: bool) -> Row {
        let (prefix, count) = if effects {
            ("R", &mut self.in_param[1])
        } else {
            ("E", &mut self.in_param[0])
        };
        *count += 1;
        let name = match *count {
            1 => format!("{prefix}({})", self.param),
            n => format!("{prefix}({}, {n})", self.param),
        };
        let i = (self.base + self.params.len()) as u32;
        let mut param = TyParam::set(name);
        if effects && self.immediate {
            param.bounds.push(SmolStr::new(IMMEDIATE));
        }
        self.params.push(param);
        let row = Row::from_elems([RowElem::Bound(i)]);
        if positive {
            if effects {
                self.positive_uses.extend(&row);
            } else {
                self.positive.extend(&row);
            }
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
    /// effect の操作の宣言の中も同じ．
    pub(crate) sets: bool,
}

impl Scope<'_> {
    /// 省略した `fails` 節（`effects` なら `use` 節）の集合．
    fn omitted_row(&mut self, positive: bool, effects: bool) -> Row {
        match &mut self.omitted {
            Omitted::Empty => Row::empty(),
            Omitted::Hidden(hidden) => hidden.fresh(positive, effects),
            Omitted::Fresh(ctx) => Row::var(ctx.new_row_var()),
        }
    }
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
    /// type / enum の型引数の集合の種類（9.2 の `type Handler[E, R]`）．
    sets: DeclSets,
}

impl TyTable {
    /// type / enum / error の定義を登録して表を作る．
    pub fn new(program: &Program) -> Self {
        let mut cons = TyCons::new();
        let mut by_def = ArenaMap::default();
        let mut def_of = ArenaMap::default();
        let bool_def = program.prelude_def("Bool");
        let sets = decl_sets(program);

        for (def, data) in program.defs.iter() {
            let kind = match data.kind {
                DefKind::Type => TyConKind::Type,
                DefKind::Enum => TyConKind::Enum,
                DefKind::Error => {
                    cons.set_tag_name(def, data.name.clone());
                    TyConKind::Error
                }
                DefKind::Effect => {
                    cons.set_tag_name(def, data.name.clone());
                    continue;
                }
                DefKind::Handler => TyConKind::Handler,
                _ => continue,
            };
            if bool_def == Some(def) {
                continue;
            }

            let id = cons.alloc(TyConData {
                name: data.name.clone(),
                kind,
                params: decl_params(program, def, sets.get(def).map_or(&[], Vec::as_slice)),
            });

            by_def.insert(def, id);
            def_of.insert(id, def);
        }

        TyTable {
            cons,
            by_def,
            bool_def,
            def_of,
            sets,
        }
    }

    /// type / enum の型引数の集合の種類．
    pub(crate) fn decl_sets(&self) -> &DeclSets {
        &self.sets
    }

    /// type / enum の `i` 番目の型引数が集合なら，その種類．
    fn set_kind(&self, def: DefId, i: usize) -> Option<SetKind> {
        self.sets.get(def).and_then(|s| s.get(i).copied().flatten())
    }

    /// type / enum / error / handler の定義を登録した宣言．Bool と，それ以外の定義には `None`．
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
            // 集合は名前付き型の集合の型引数の位置にだけ書ける（`lower_set_arg`）．
            TypeKind::ErrorSet(_) | TypeKind::EffectSet(_) => {
                diags.push(Diagnostic {
                    span: type_ref.span,
                    kind: DiagnosticKind::SetAsType,
                });
                Ty::Error
            }
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
                uses,
            } => {
                // 見えない集合の型引数は外側の関数型から振る（`f: fn(fn() -> A) -> B` の
                // f 自身の E が `E(f)`）．
                let fails = match fails {
                    Some(set) => self.lower_error_set(program, scope, set, diags),
                    None => scope.omitted_row(positive, false),
                };
                let uses = match uses {
                    Some(set) => self.lower_effect_set(program, scope, set, diags),
                    None => scope.omitted_row(positive, true),
                };
                let ret_ty = match ret {
                    Some(ret) => self.lower_in(program, scope, *ret, positive, diags),
                    None => Ty::Error,
                };
                let param_tys: Vec<Ty> = fn_params
                    .iter()
                    .map(|&p| self.lower_in(program, scope, p, !positive, diags))
                    .collect();
                Ty::func_sets(param_tys, ret_ty, fails, uses)
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

    /// `use` 節の集合．エフェクトはタグ，集合の型引数は `RowElem::Bound`．
    pub(crate) fn lower_effect_set(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        set: &EffectSet,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let mut row = Row::empty();
        for &(res, span) in &set.items {
            let Res::Def(def) = res else {
                continue;
            };
            match program.defs[def].kind {
                DefKind::Effect => row.insert(RowElem::Tag(def)),
                DefKind::TypeParam => {
                    let Some(i) = scope.params.iter().position(|&p| p == def) else {
                        continue;
                    };
                    match scope.kinds.get(i) {
                        Some(ParamKind::Set) if scope.sets => row.insert(RowElem::Bound(i as u32)),
                        Some(ParamKind::Set) => diags.push(Diagnostic {
                            span: Some(span),
                            kind: DiagnosticKind::Unsupported {
                                what: "effect set type parameters on type declarations",
                            },
                        }),
                        // 値の型にも書いた型引数は効果の集合にならない．
                        _ => diags.push(Diagnostic {
                            span: Some(span),
                            kind: DiagnosticKind::Unsupported {
                                what: "type parameters used both as a type and in `use`",
                            },
                        }),
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

    /// type / enum / error / handler の定義が名指す型．型引数は `Bound(0)`，`Bound(1)`，… で埋める．
    /// Bool は `Ty::BOOL`．それ以外の定義は `None`．
    pub fn self_ty(&self, def: DefId) -> Option<Ty> {
        if self.bool_def == Some(def) {
            return Some(Ty::BOOL);
        }

        let id = self.con(def)?;
        let n = self.cons[id].params.len();

        Some(Ty::named(
            id,
            (0..n).map(|i| match self.set_kind(def, i) {
                Some(kind) => Ty::Set(kind, Row::from_elems([RowElem::Bound(i as u32)])),
                None => Ty::Bound(i as u32),
            }),
        ))
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
        if data.kind == DefKind::Effect {
            diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::EffectAsType {
                    name: data.name.clone(),
                },
            });
            return Ty::Error;
        }

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
                    .enumerate()
                    .map(|(i, &a)| match self.set_kind(def, i) {
                        Some(kind) => self.lower_set_arg(program, scope, def, i, kind, a, diags),
                        None => self.lower_in(program, scope, a, positive, diags),
                    })
                    .collect();
                Ty::named(id, args)
            }
            _ => Ty::Error,
        }
    }
}

/// 集合の型引数に合わないものを書いたときの診断（E0323）を積む．位置と，書いたものの説明を受ける．
type ReportMismatch<'a> = dyn Fn(Option<Span>, SmolStr, &mut Vec<Diagnostic>) + 'a;

impl TyTable {
    /// 名前付き型の集合の型引数に書いたものを集合にする（`Handler[NotFound, Db]`，
    /// `Handler[NotFound | Timeout, { Db, Log }]`，`Handler[E, R]`，`Handler[{}, {}]`）．
    /// 要素に書けるのは，その種類のタグ（error か effect の名前）と集合の型引数．エラーの集合は
    /// `|` でつなぎ，効果の集合は波括弧で囲む．空の `{}` はどちらの位置にも書ける．
    /// それ以外は E0323 で，その要素を除いて続ける．
    #[allow(clippy::too_many_arguments)]
    fn lower_set_arg(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        decl: DefId,
        index: usize,
        kind: SetKind,
        arg: TypeId,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let mismatch = |span: Option<Span>, found: SmolStr, diags: &mut Vec<Diagnostic>| {
            diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::SetArgMismatch {
                    param: type_param_defs(program, decl)
                        .get(index)
                        .map(|&p| program.defs[p].name.clone())
                        .unwrap_or_default(),
                    ty: program.defs[decl].name.clone(),
                    effects: kind == SetKind::Effects,
                    found,
                },
            });
        };
        let type_ref = &program.types[arg];
        let items: &[(Res, Span)] = match (&type_ref.kind, kind) {
            (TypeKind::ErrorSet(set), SetKind::Errors) => &set.items,
            (TypeKind::EffectSet(set), SetKind::Effects) => &set.items,
            (TypeKind::EffectSet(set), SetKind::Errors) if set.items.is_empty() => &[],
            (TypeKind::ErrorSet(_), SetKind::Effects) => {
                mismatch(type_ref.span, "an error set".into(), diags);
                return Ty::Set(kind, Row::empty());
            }
            (TypeKind::EffectSet(_), SetKind::Errors) => {
                mismatch(type_ref.span, "an effect set".into(), diags);
                return Ty::Set(kind, Row::empty());
            }
            (TypeKind::Missing, _) => return Ty::Set(kind, Row::empty()),
            (TypeKind::Path { res, args }, _) if args.is_empty() => {
                return Ty::Set(
                    kind,
                    self.set_elem(program, scope, kind, *res, type_ref.span, &mismatch, diags),
                );
            }
            (
                TypeKind::Path {
                    res: Res::Def(def), ..
                },
                _,
            ) => {
                let found = format!("type `{}`", program.defs[*def].name);
                mismatch(type_ref.span, found.into(), diags);
                return Ty::Set(kind, Row::empty());
            }
            _ => {
                mismatch(type_ref.span, "a type".into(), diags);
                return Ty::Set(kind, Row::empty());
            }
        };
        let mut row = Row::empty();
        for &(res, span) in items {
            row.extend(&self.set_elem(program, scope, kind, res, Some(span), &mismatch, diags));
        }
        Ty::Set(kind, row)
    }

    /// 集合の要素1つ．その種類のタグか集合の型引数．
    #[allow(clippy::too_many_arguments)]
    fn set_elem(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        kind: SetKind,
        res: Res,
        span: Option<Span>,
        mismatch: &ReportMismatch<'_>,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        // 名前解決が誤りを出している．
        let Res::Def(def) = res else {
            return Row::empty();
        };
        let data = &program.defs[def];
        match (data.kind, kind) {
            (DefKind::Error, SetKind::Errors) | (DefKind::Effect, SetKind::Effects) => {
                Row::tag(def)
            }
            (DefKind::TypeParam, _) => match scope.params.iter().position(|p| *p == def) {
                Some(i) if scope.kinds.get(i) == Some(&ParamKind::Set) && scope.sets => {
                    Row::from_elems([RowElem::Bound(i as u32)])
                }
                Some(i) if scope.kinds.get(i) == Some(&ParamKind::Set) => {
                    diags.push(Diagnostic {
                        span,
                        kind: DiagnosticKind::Unsupported {
                            what: "set type parameters here",
                        },
                    });
                    Row::empty()
                }
                _ => {
                    mismatch(
                        span,
                        format!("type parameter `{}`", data.name).into(),
                        diags,
                    );
                    Row::empty()
                }
            },
            (DefKind::Effect, _) => {
                mismatch(span, format!("effect `{}`", data.name).into(), diags);
                Row::empty()
            }
            (DefKind::Error, _) => {
                mismatch(span, format!("error `{}`", data.name).into(), diags);
                Row::empty()
            }
            _ => {
                mismatch(span, format!("type `{}`", data.name).into(), diags);
                Row::empty()
            }
        }
    }
}

/// type / enum の宣言の型引数．集合の型引数は `ParamKind::Set` にし，効果の集合に Immediate の
/// 制約があれば付ける（9.5）．
fn decl_params(program: &Program, def: DefId, sets: &[Option<SetKind>]) -> Vec<TyParam> {
    let type_params: &[TypeParam] = match program.item(def) {
        Some(Item::Type(TypeItem { type_params, .. }))
        | Some(Item::Enum(EnumItem { type_params, .. })) => type_params,
        _ => &[],
    };
    type_params
        .iter()
        .enumerate()
        .map(|(i, tp)| {
            let name = program.defs[tp.def].name.clone();
            match sets.get(i).copied().flatten() {
                Some(kind) => {
                    let mut p = TyParam::set(name);
                    if kind == SetKind::Effects && !immediate_bounds(program, tp).is_empty() {
                        p.bounds.push(SmolStr::new(IMMEDIATE));
                    }
                    p
                }
                None => TyParam::new(name),
            }
        })
        .collect()
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

/// 組み込みの制約 Immediate の名前（9.5）．
pub(crate) const IMMEDIATE: &str = "Immediate";

/// 型引数の定義の並びから，スキームと型の宣言に付ける名前の並びを作る．
pub(crate) fn ty_params(program: &Program, defs: &[DefId]) -> Vec<TyParam> {
    defs.iter()
        .map(|&def| TyParam::new(program.defs[def].name.clone()))
        .collect()
}

/// 型引数の制約のうち，組み込みの Immediate を指す位置．
pub(crate) fn immediate_bounds(program: &Program, tp: &TypeParam) -> Vec<Span> {
    let immediate = program.prelude_def(IMMEDIATE);
    tp.bounds
        .iter()
        .filter(|(res, _)| matches!(res, Res::Def(d) if Some(*d) == immediate))
        .map(|&(_, span)| span)
        .collect()
}

/// エフェクトが Immediate か（9.5）．suspend fn を1つも持たず，Async でもないとき．
pub(crate) fn is_immediate_effect(program: &Program, effect: DefId) -> bool {
    if program.prelude_def("Async") == Some(effect) {
        return false;
    }
    match program.item(effect) {
        Some(Item::Effect(e)) => e
            .ops
            .iter()
            .all(|&op| !matches!(program.item(op), Some(Item::Op(o)) if o.sig.is_suspend)),
        _ => true,
    }
}

/// 名前付き型の宣言の型引数ごとの集合の種類（集合でなければ `None`）．type / enum の定義で引く．
pub(crate) type DeclSets = ArenaMap<DefId, Vec<Option<SetKind>>>;

/// 型引数の種類を，使われた位置から決める（9.2）．`fails` 節か `use` 節にだけ書いた型引数は
/// 集合，それ以外はふつうの型（値の型と `fails` の両方に書けば1つのエラーの型，7.2）．
/// 名前付き型の集合の型引数の位置（`Handler[E, R]` の `E`）に書いた型引数も集合．
/// `types` は型引数が現れうる型の書き方（引数，戻り値，フィールドの型）．
pub(crate) fn classify_params(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    types: impl IntoIterator<Item = TypeId>,
) -> Vec<ParamKind> {
    classify_sets(program, decls, params, types)
        .into_iter()
        .map(|set| match set {
            Some(_) => ParamKind::Set,
            None => ParamKind::Type,
        })
        .collect()
}

/// [`classify_params`] と同じく決め，集合ならその種類（エラーの集合か効果の集合か）を返す．
/// 両方に書いた型引数は，先に見つけた方にする．
pub(crate) fn classify_sets(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    types: impl IntoIterator<Item = TypeId>,
) -> Vec<Option<SetKind>> {
    let mut marks = Marks {
        in_type: vec![false; params.len()],
        in_set: vec![None; params.len()],
    };
    for ty in types {
        mark_params(program, decls, params, ty, &mut marks);
    }
    marks
        .in_type
        .iter()
        .zip(&marks.in_set)
        .map(|(&t, &s)| if t { None } else { s })
        .collect()
}

struct Marks {
    in_type: Vec<bool>,
    in_set: Vec<Option<SetKind>>,
}

impl Marks {
    fn set(&mut self, i: usize, kind: SetKind) {
        self.in_set[i].get_or_insert(kind);
    }
}

fn mark_params(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    ty: TypeId,
    marks: &mut Marks,
) {
    let position = |res: &Res| match res {
        Res::Def(def) => params.iter().position(|p| p == def),
        _ => None,
    };
    match &program.types[ty].kind {
        TypeKind::Path { res, args } => {
            if let Some(i) = position(res) {
                marks.in_type[i] = true;
            }
            let sets = match res {
                Res::Def(def) => decls.get(*def),
                _ => None,
            };
            for (j, &a) in args.iter().enumerate() {
                // 集合の型引数の位置に書いた型引数は集合．
                if let Some(kind) = sets.and_then(|s| s.get(j).copied().flatten())
                    && let TypeKind::Path { res, args } = &program.types[a].kind
                    && args.is_empty()
                    && let Some(i) = position(res)
                {
                    marks.set(i, kind);
                    continue;
                }
                mark_params(program, decls, params, a, marks);
            }
        }
        TypeKind::Tuple(elems) => {
            for &e in elems {
                mark_params(program, decls, params, e, marks);
            }
        }
        TypeKind::Fn {
            params: fn_params,
            ret,
            fails,
            uses,
        } => {
            for &p in fn_params {
                mark_params(program, decls, params, p, marks);
            }
            if let Some(ret) = ret {
                mark_params(program, decls, params, *ret, marks);
            }
            for (res, _) in fails.iter().flat_map(|f| &f.items) {
                if let Some(i) = position(res) {
                    marks.set(i, SetKind::Errors);
                }
            }
            for (res, _) in uses.iter().flat_map(|u| &u.items) {
                if let Some(i) = position(res) {
                    marks.set(i, SetKind::Effects);
                }
            }
        }
        TypeKind::ErrorSet(set) => {
            for (res, _) in &set.items {
                if let Some(i) = position(res) {
                    marks.set(i, SetKind::Errors);
                }
            }
        }
        TypeKind::EffectSet(set) => {
            for (res, _) in &set.items {
                if let Some(i) = position(res) {
                    marks.set(i, SetKind::Effects);
                }
            }
        }
        TypeKind::SelfType | TypeKind::Unit | TypeKind::Missing => {}
    }
}

/// type / enum の宣言の型引数の集合の種類を決める．フィールドの型から決め，名前付き型の
/// 集合の型引数の位置に渡したもの（`type Server[E](h: Handler[E, {}])`）も集合なので，
/// 変わらなくなるまで繰り返す．
fn decl_sets(program: &Program) -> DeclSets {
    let decls: Vec<(DefId, Vec<DefId>, Vec<TypeId>)> = program
        .defs
        .iter()
        .filter(|(_, d)| matches!(d.kind, DefKind::Type | DefKind::Enum))
        .map(|(def, _)| {
            (
                def,
                type_param_defs(program, def),
                field_types(program, def),
            )
        })
        .collect();
    let mut sets = DeclSets::default();
    loop {
        let mut changed = false;
        for (def, params, types) in &decls {
            let kinds = classify_sets(program, &sets, params, types.iter().copied());
            if sets.get(*def) != Some(&kinds) {
                sets.insert(*def, kinds);
                changed = true;
            }
        }
        if !changed {
            return sets;
        }
    }
}

/// type / enum の構成子のフィールドの型の書き方．
fn field_types(program: &Program, def: DefId) -> Vec<TypeId> {
    let fields = |f: &Fields| -> Vec<TypeId> {
        match f {
            Fields::None => Vec::new(),
            Fields::Positional(tys) => tys.clone(),
            Fields::Named(fs) => fs.iter().map(|f| f.ty).collect(),
        }
    };
    let ctor_fields = |ctor: DefId| match program.item(ctor) {
        Some(Item::Ctor(c)) => fields(&c.fields),
        _ => Vec::new(),
    };
    match program.item(def) {
        Some(Item::Type(t)) => t.ctor.map(ctor_fields).unwrap_or_default(),
        Some(Item::Enum(e)) => e.variants.iter().flat_map(|&v| ctor_fields(v)).collect(),
        _ => Vec::new(),
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
        TypeKind::SelfType
        | TypeKind::Unit
        | TypeKind::Missing
        | TypeKind::ErrorSet(_)
        | TypeKind::EffectSet(_) => false,
    }
}

/// 型の書き方の中に，`use` 節を省略した関数型があるか．戻り値の型にあれば，
/// その R は本体から推論する（9.2）．
pub(crate) fn has_omitted_uses(program: &Program, ty: TypeId) -> bool {
    match &program.types[ty].kind {
        TypeKind::Path { args, .. } => args.iter().any(|&a| has_omitted_uses(program, a)),
        TypeKind::Tuple(elems) => elems.iter().any(|&e| has_omitted_uses(program, e)),
        TypeKind::Fn { uses: None, .. } => true,
        TypeKind::Fn { params, ret, .. } => {
            params.iter().any(|&p| has_omitted_uses(program, p))
                || ret.is_some_and(|r| has_omitted_uses(program, r))
        }
        TypeKind::SelfType
        | TypeKind::Unit
        | TypeKind::Missing
        | TypeKind::ErrorSet(_)
        | TypeKind::EffectSet(_) => false,
    }
}
