//! 関数と const の本体の推論．式とパターンに型を付ける．
//!
//! 互いに参照し合う定義のまとまり（呼び出しグラフの強連結成分）ごとに [`BodyInfer`] を作り，
//! 式を下から順に推論する．型が合わなければ式の位置を付けた診断を積んで続ける．
//! 宣言の型のない定義（戻り値の型を書いていない fn，型を書いていない const）は，まとまりの
//! 中では単相の型で参照し，まとまりを推論し終えてから汎化する（[`crate::check`]）．

use emela_resolve::hir::{
    Arg, Arm, BinaryOp, Block, DefId, DefModule, ExprId, ExprKind, FnItem, Item, Label, ListRest,
    Literal, LocalId, Param, PatArg, PatId, PatKind, Program, Res, Span, Stmt, StrPart, TypeId,
    UnaryOp,
};
use la_arena::{ArenaMap, Idx};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::FnTy;
use crate::coverage::Shapes;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::exhaustiveness::{self, Witness, check_match};
use crate::infer::{InferCtx, substitute_bound};
use crate::lower::TyTable;
use crate::signature::{Signature, Signatures, param_ty};
use crate::ty::{Prim, Ty, TyParam, TyVar};

/// 定義のまとまり1つの推論の結果．型は単一化の束縛を辿った形．
pub(crate) struct BodyTypes {
    pub(crate) exprs: ArenaMap<ExprId, Ty>,
    pub(crate) pats: ArenaMap<PatId, Ty>,
    pub(crate) locals: ArenaMap<LocalId, Ty>,
    /// 局所変数がどの定義の本体のものか．
    pub(crate) local_owner: ArenaMap<LocalId, DefId>,
    pub(crate) fields: ArenaMap<ExprId, usize>,
    pub(crate) calls: ArenaMap<ExprId, Vec<Option<ExprId>>>,
    pub(crate) type_args: ArenaMap<ExprId, Vec<Ty>>,
    pub(crate) pat_fields: ArenaMap<PatId, Vec<Option<PatId>>>,
    pub(crate) diags: Vec<Diagnostic>,
    /// 宣言の型のなかった定義の，汎化した宣言の型．
    pub(crate) inferred: Vec<(DefId, Signature)>,
}

/// 定義のまとまり1つを推論する間の状態．
pub(crate) struct BodyInfer<'a> {
    program: &'a Program,
    ty_table: &'a TyTable,
    sigs: &'a Signatures,
    /// 推論している定義．
    current: Option<DefId>,
    /// 推論している定義のモジュール．opaque な型のフィールドを引けるかの判定に使う．
    module: DefModule,
    /// 推論している fn の型引数．
    type_params: Vec<DefId>,
    ctx: InferCtx,
    /// まとまりの中の，宣言の型のない定義の単相の型と，書いた型引数の数．
    /// 推論し終えてから汎化する．
    pending: Vec<(DefId, Ty, usize)>,
    /// 書いた型引数のある `pending` の定義を，まとまりの中の他の定義から参照したもの．
    /// 推論し終えてから，その定義の最終的な型と照合する（[`Self::finish`]）．
    deferred: Vec<DeferredRef>,
    locals: ArenaMap<LocalId, Ty>,
    local_owner: ArenaMap<LocalId, DefId>,
    /// フィールドの参照の式 → フィールドの添字．
    fields: ArenaMap<ExprId, usize>,
    /// 呼び出し・パイプ・部分更新の式 → 位置の順に並べ直した引数．
    calls: ArenaMap<ExprId, Vec<Option<ExprId>>>,
    /// 汎用の定義の参照の式 → 具体化した型引数．
    type_args: ArenaMap<ExprId, Vec<Ty>>,
    /// 構成子のパターン → フィールドの位置の順に並べ直したパターン．
    pat_fields: ArenaMap<PatId, Vec<Option<PatId>>>,
    exprs: ArenaMap<ExprId, Ty>,
    pats: ArenaMap<PatId, Ty>,
    diags: Vec<Diagnostic>,
    /// 数の型の制約．本体を推論し終えたときに確かめる（[`Self::check_numbers`]）．
    numbers: Vec<NumConstraint>,
    /// 網羅性を確かめる match と束縛．型が決まってから確かめる（[`Self::check_coverage`]）．
    coverage: Vec<CoverageSite>,
}

impl<'a> BodyInfer<'a> {
    /// 空の状態で始める．
    pub(crate) fn new(program: &'a Program, ty_table: &'a TyTable, sigs: &'a Signatures) -> Self {
        BodyInfer {
            program,
            ty_table,
            sigs,
            current: None,
            module: DefModule::Prelude,
            type_params: Vec::new(),
            ctx: InferCtx::new(),
            pending: Vec::new(),
            deferred: Vec::new(),
            locals: ArenaMap::new(),
            local_owner: ArenaMap::new(),
            fields: ArenaMap::new(),
            calls: ArenaMap::new(),
            type_args: ArenaMap::new(),
            pat_fields: ArenaMap::new(),
            exprs: ArenaMap::new(),
            pats: ArenaMap::new(),
            diags: Vec::new(),
            numbers: Vec::new(),
            coverage: Vec::new(),
        }
    }

    /// alpha.1 ではまだ扱えない式．「まだ扱えない」の診断を積み，中の式だけ推論する．
    fn unsupported(&mut self, span: Span, what: &'static str, inner: &[ExprId]) -> Ty {
        self.diags.push(Diagnostic {
            span: Some(span),
            kind: DiagnosticKind::Unsupported { what },
        });
        for &expr in inner {
            self.infer_expr(expr);
        }
        Ty::Error
    }

    /// これから推論する定義に切り替える．モジュールと型引数をその定義のものにする．
    fn enter(&mut self, def: DefId) {
        self.current = Some(def);
        self.module = self.program.defs[def].module.clone();
        self.type_params = match self.program.item(def) {
            Some(Item::Fn(item)) => item.sig.type_params.iter().map(|tp| tp.def).collect(),
            _ => Vec::new(),
        };
    }

    /// 宣言の型のない定義に，まとまりの中で使う単相の型を付ける．fn は書いた引数の型と，
    /// 戻り値の型の型変数．const は型変数．
    pub(crate) fn declare(&mut self, def: DefId) {
        self.enter(def);
        let program = self.program;
        let ty = match program.item(def) {
            Some(Item::Fn(item)) => {
                let params: Vec<Ty> = item
                    .sig
                    .params
                    .iter()
                    .map(|p| {
                        param_ty(
                            program,
                            self.ty_table,
                            &self.type_params,
                            p,
                            &mut self.diags,
                        )
                    })
                    .collect();
                let ret = self.ctx.new_var();
                Ty::func(params, ret)
            }
            _ => self.ctx.new_var(),
        };
        self.pending.push((def, ty, self.type_params.len()));
    }

    /// 定義の本体を推論する．宣言の型（なければ [`Self::declare`] で付けた型）の位置に置く．
    pub(crate) fn infer_def(&mut self, def: DefId) {
        self.enter(def);
        let declared = match self.sigs.get(def) {
            Some(sig) => sig.scheme.ty.clone(),
            None => match self.pending_ty(def) {
                Some((ty, _)) => ty,
                None => return,
            },
        };
        let program = self.program;
        match program.item(def) {
            Some(Item::Fn(item)) => self.infer_fn(item, &declared),
            Some(Item::Const(item)) => self.check_expr(item.value, &declared),
            _ => {}
        }
    }

    /// fn の本体を推論する．`declared` は fn の型．
    fn infer_fn(&mut self, item: &FnItem, declared: &Ty) {
        let Ty::Fn(f) = declared else {
            return;
        };
        for (p, ty) in item.sig.params.iter().zip(&f.params) {
            self.set_local(p.local, ty.clone());
        }
        if let Some(body) = item.body {
            self.check_expr(body, &f.ret);
        }
    }

    /// まとまりの中の，宣言の型のない定義の単相の型と，書いた型引数の数．
    fn pending_ty(&self, def: DefId) -> Option<(Ty, usize)> {
        self.pending
            .iter()
            .find(|(d, _, _)| *d == def)
            .map(|(_, ty, n)| (ty.clone(), *n))
    }

    /// match の網羅性と冗長な腕，束縛のパターンが必ず成功するかを確かめる（6.3，6.5）．
    /// 対象の型が `Ty::Error` なら，別の誤りが出ているので確かめない．
    fn check_coverage(&mut self) {
        let shapes = Shapes {
            program: self.program,
            ty_table: self.ty_table,
            sigs: self.sigs,
        };
        for site in std::mem::take(&mut self.coverage) {
            match site {
                CoverageSite::Match {
                    scrutinee,
                    ty,
                    arms,
                } => {
                    let ty = self.ctx.resolve(&ty);
                    if ty == Ty::Error {
                        continue;
                    }
                    let check_arms: Vec<exhaustiveness::Arm> = arms
                        .iter()
                        .map(|arm| exhaustiveness::Arm {
                            pat: shapes.pat(arm.pat, &ty),
                            guarded: arm.guard.is_some(),
                        })
                        .collect();
                    let result = check_match(&shapes, &ty, &check_arms);
                    if !result.missing.is_empty() {
                        self.diags.push(Diagnostic {
                            span: Some(self.program.exprs[scrutinee].span),
                            kind: DiagnosticKind::NonExhaustiveMatch {
                                missing: witnesses(&result.missing),
                            },
                        });
                    }
                    for i in result.redundant {
                        self.diags.push(Diagnostic {
                            span: Some(self.program.pats[arms[i].pat].span),
                            kind: DiagnosticKind::UnreachableArm,
                        });
                    }
                }
                CoverageSite::Binding { pat, ty } => {
                    let ty = self.ctx.resolve(&ty);
                    if ty == Ty::Error {
                        continue;
                    }
                    let arm = exhaustiveness::Arm {
                        pat: shapes.pat(pat, &ty),
                        guarded: false,
                    };
                    let result = check_match(&shapes, &ty, &[arm]);
                    if !result.missing.is_empty() {
                        self.diags.push(Diagnostic {
                            span: Some(self.program.pats[pat].span),
                            kind: DiagnosticKind::RefutableBinding {
                                missing: witnesses(&result.missing),
                            },
                        });
                    }
                }
            }
        }
    }

    /// 型の中のまだ決まっていない型変数を，新しい型変数に置き換える．同じ型変数は同じものにする．
    fn freshen(&mut self, ty: &Ty, fresh: &mut FxHashMap<TyVar, Ty>) -> Ty {
        match self.ctx.shallow_resolve(ty) {
            Ty::Var(v) => fresh.entry(v).or_insert_with(|| self.ctx.new_var()).clone(),
            ty => ty.map_children(|child| self.freshen(child, fresh)),
        }
    }

    /// 局所変数に型を付ける．どの定義の本体のものかも覚える．
    fn set_local(&mut self, local: LocalId, ty: Ty) {
        self.locals.insert(local, ty);
        if let Some(def) = self.current {
            self.local_owner.insert(local, def);
        }
    }

    /// 推論を終える．数の型を確かめ，宣言の型のなかった定義を汎化して，型を辿った形にして返す．
    pub(crate) fn finish(mut self) -> BodyTypes {
        for r in std::mem::take(&mut self.deferred) {
            if let Some((ty, _)) = self.pending_ty(r.def) {
                let ty = self.ctx.resolve(&ty);
                let actual = substitute_bound(&ty, &r.args);
                self.coerce_at(r.span, &r.instance, &actual);
            }
        }
        self.check_numbers();
        self.check_coverage();

        let program = self.program;
        let mut inferred = Vec::new();
        for (def, ty, _) in std::mem::take(&mut self.pending) {
            let (existing, labels) = match program.item(def) {
                Some(Item::Fn(item)) => (
                    item.sig
                        .type_params
                        .iter()
                        .map(|tp| TyParam::new(program.defs[tp.def].name.clone()))
                        .collect(),
                    item.sig
                        .params
                        .iter()
                        .map(|p| Some(program.locals[p.local].name.clone()))
                        .collect(),
                ),
                _ => (Vec::new(), Vec::new()),
            };
            // トップレベルなので環境の型変数はない．
            let scheme = self.ctx.generalize_with(&ty, existing, &[]);
            inferred.push((def, Signature { scheme, labels }));
        }

        let exprs = resolve_all(&mut self.ctx, &self.exprs);
        let pats = resolve_all(&mut self.ctx, &self.pats);
        let locals = resolve_all(&mut self.ctx, &self.locals);
        let mut type_args = ArenaMap::default();
        for (expr, args) in self.type_args.iter() {
            let args: Vec<Ty> = args.iter().map(|ty| self.ctx.resolve(ty)).collect();
            type_args.insert(expr, args);
        }
        BodyTypes {
            exprs,
            pats,
            locals,
            local_owner: self.local_owner,
            fields: self.fields,
            calls: self.calls,
            type_args,
            pat_fields: self.pat_fields,
            diags: self.diags,
            inferred,
        }
    }

    /// `actual` を `expected` の位置に置く．合わなければ `span` の位置で診断を積む．
    fn coerce_at(&mut self, span: Span, expected: &Ty, actual: &Ty) {
        if let Err(e) = self.ctx.coerce(expected, actual) {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::Mismatch(e),
            });
        }
    }

    /// 式の型を推論し，記録して返す．
    fn infer_expr(&mut self, expr: ExprId) -> Ty {
        let ty = self.infer_expr_kind(expr, None);
        self.exprs.insert(expr, ty.clone());
        ty
    }

    /// 式の種類ごとの推論．
    ///
    /// `expected` は期待する型．ブロック，if，match だけが使い，最後の式や各分岐まで渡す
    /// （[`Self::check_expr`]）．それ以外の式は無視する．
    fn infer_expr_kind(&mut self, expr: ExprId, expected: Option<&Ty>) -> Ty {
        let program = self.program;
        let e = &program.exprs[expr];
        match &e.kind {
            ExprKind::Missing => Ty::Error,
            ExprKind::Lit(lit) => self.lit_ty(lit, e.span),
            ExprKind::Unit => Ty::UNIT,
            ExprKind::Tuple(elems) => Ty::tuple(elems.iter().map(|&e| self.infer_expr(e))),
            ExprKind::Path(Res::Local(local)) => {
                self.locals.get(*local).cloned().unwrap_or(Ty::Error)
            }
            ExprKind::Path(Res::Def(def)) => {
                let (ty, args) = self.instantiate_def(*def, e.span);
                if !args.is_empty() {
                    self.type_args.insert(expr, args);
                }
                ty
            }
            ExprKind::Path(Res::Err) => Ty::Error,
            ExprKind::Block(block) => self.infer_block(block, e.span, expected),
            ExprKind::Call { callee, args } => self.infer_call(expr, *callee, args, e.span),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.infer_if(*cond, *then_branch, *else_branch, e.span, expected),
            ExprKind::Match { scrutinee, arms } => self.infer_match(*scrutinee, arms, expected),
            ExprKind::Binary { op, lhs, rhs } => self.infer_binary(expr, *op, *lhs, *rhs, e.span),
            ExprKind::Unary { op, operand } => self.infer_unary(*op, *operand, e.span),
            ExprKind::Lambda { params, body } => self.infer_lambda(params, *body, e.span, expected),
            ExprKind::List { elems, rest } => self.infer_list(elems, *rest, e.span, expected),
            ExprKind::Field { base, name } => self.infer_field(expr, *base, name),
            ExprKind::Interpolated(parts) => {
                for part in parts {
                    if let StrPart::Interp(expr) = part {
                        // Show を満たすかは Trait の段で見る．
                        self.infer_expr(*expr);
                    }
                }
                Ty::STRING
            }
            ExprKind::Assert(cond) => {
                self.check_expr(*cond, &Ty::BOOL);
                Ty::UNIT
            }
            ExprKind::Update { ctor, fields, base } => {
                self.infer_update(expr, *ctor, fields, *base)
            }
            // エラーとエフェクト（7章，8章）は alpha.2．中の式は推論して型を記録しておく．
            ExprKind::Fail(inner) => self.unsupported(e.span, "`fail`", &[*inner]),
            ExprKind::Escape { expr: inner, .. } => self.unsupported(e.span, "`escape`", &[*inner]),
            ExprKind::Use { .. } => self.unsupported(e.span, "`use`", &[]),
            ExprKind::With { body, .. } => self.unsupported(e.span, "`with`", &[*body]),
        }
    }

    /// ブロックの型．最後が束縛か空なら `()`．
    /// 期待する型があれば最後の式に渡し，ブロックの型はその型にする．
    fn infer_block(&mut self, block: &Block, span: Span, expected: Option<&Ty>) -> Ty {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { pat, ty, value } => {
                    let bound_ty = match ty {
                        Some(ty) => {
                            let expected = self.lower_type(*ty);
                            self.check_expr(*value, &expected);
                            expected
                        }
                        None => self.infer_expr(*value),
                    };
                    self.bind_pat(*pat, &bound_ty);
                    self.coverage.push(CoverageSite::Binding {
                        pat: *pat,
                        ty: bound_ty,
                    });
                }
                Stmt::Expr(expr) => {
                    self.infer_expr(*expr);
                }
            }
        }
        match (block.tail, expected) {
            (Some(expr), Some(expected)) => {
                self.check_expr(expr, expected);
                expected.clone()
            }
            (Some(expr), None) => self.infer_expr(expr),
            (None, expected) => self.unit_at(span, expected),
        }
    }

    /// 値が `()` の式の型．期待する型があれば `()` をその位置に置く．
    fn unit_at(&mut self, span: Span, expected: Option<&Ty>) -> Ty {
        if let Some(expected) = expected {
            self.coerce_at(span, expected, &Ty::UNIT);
        }
        Ty::UNIT
    }

    /// 定義の参照の型．宣言の型の型引数を新しい型変数に置き換える．
    ///
    /// 同じまとまりの中の，宣言の型のない定義は単相の型をそのまま使う（具体化しない）．
    fn infer_def_ref(&mut self, def: DefId, span: Span) -> Ty {
        self.instantiate_def(def, span).0
    }

    /// [`Self::infer_def_ref`] と同じく型を付け，型引数に置いた型も返す．
    /// 書いた型引数を持たない参照（単相の参照を含む）は空．
    fn instantiate_def(&mut self, def: DefId, span: Span) -> (Ty, Vec<Ty>) {
        let sigs = self.sigs;
        if let Some(sig) = sigs.get(def) {
            return self.ctx.instantiate_with_args(&sig.scheme);
        }
        if let Some((ty, n_type_params)) = self.pending_ty(def) {
            if self.current == Some(def) || n_type_params == 0 {
                return (ty, Vec::new());
            }
            // 書いた型引数（`Ty::Bound(i)`）はその定義の中でだけ意味を持つので，
            // 他の定義から参照するときは新しい型変数に置き換える．まだ決まっていない部分
            // （戻り値の型など）は，後でその定義の型引数に決まることがあるので，ここでは
            // 新しい型変数にしておき，まとまりを推論し終えてから照合する．
            let ty = self.ctx.resolve(&ty);
            let args: Vec<Ty> = (0..n_type_params).map(|_| self.ctx.new_var()).collect();
            let mut fresh = FxHashMap::default();
            let instance = self.freshen(&substitute_bound(&ty, &args), &mut fresh);
            self.deferred.push(DeferredRef {
                def,
                args: args.clone(),
                instance: instance.clone(),
                span,
            });
            return (instance, args);
        }
        // 型を付けない種類の定義（trait の関数，effect の操作など．alpha.2 以降）．
        self.diags.push(Diagnostic {
            span: Some(span),
            kind: DiagnosticKind::Unsupported {
                what: "reference to this kind of definition",
            },
        });
        (Ty::Error, Vec::new())
    }

    /// 呼び出しの型．引数を仮引数の位置に並べ直してから，型を照合する（6.2）．
    /// `expr` は並べ直した引数を記録する式（呼び出しの式か，パイプの式）．
    fn infer_call(&mut self, expr: ExprId, callee: ExprId, args: &[Arg], span: Span) -> Ty {
        let program = self.program;
        let sigs = self.sigs;

        let (callee_name, labels) = match &program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => (
                Some(&program.defs[*def].name),
                sigs.get(*def).map(|sig| sig.labels.as_slice()),
            ),
            _ => (None, None),
        };
        let callee_ty = self.infer_expr(callee);

        let f: FnTy = match self.ctx.shallow_resolve(&callee_ty) {
            Ty::Fn(f) => *f,
            Ty::Error => {
                for arg in args {
                    self.infer_expr(arg.value);
                }
                return Ty::Error;
            }
            _ => {
                let params: Vec<Ty> = (0..args.len()).map(|_| self.ctx.new_var()).collect();
                let ret = self.ctx.new_var();
                let expected = Ty::func(params.clone(), ret.clone());
                let callee_span = program.exprs[callee].span;
                self.coerce_at(callee_span, &expected, &callee_ty);
                FnTy { params, ret }
            }
        };

        let assigned = self.assign_args(
            callee_name,
            labels,
            f.params.len(),
            args.iter().map(|arg| (arg.label.as_ref(), arg.value)),
        );
        if !assigned.reported && (assigned.too_many || assigned.has_missing()) {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::WrongArgCount {
                    callee: callee_name.cloned(),
                    expected: f.params.len(),
                    found: args.len(),
                },
            });
        }

        for (slot, param_ty) in assigned.slots.iter().zip(&f.params) {
            if let Some(arg) = slot {
                self.check_expr(*arg, param_ty);
            }
        }

        for extra in assigned.extra {
            self.infer_expr(extra);
        }
        self.calls.insert(expr, assigned.slots);

        f.ret
    }

    /// if の型．else がなければ本体は `()` か `Never` で，全体は `()`（6.4）．
    /// 期待する型があり else もあれば，両方の分岐に期待する型を渡す．
    fn infer_if(
        &mut self,
        cond: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        self.check_expr(cond, &Ty::BOOL);

        match (else_branch, expected) {
            (Some(else_branch), Some(expected)) => {
                self.check_expr(then_branch, expected);
                self.check_expr(else_branch, expected);
                expected.clone()
            }
            (Some(else_branch), None) => {
                let then_ty = self.infer_expr(then_branch);
                let else_ty = self.infer_expr(else_branch);
                let span = self.program.exprs[else_branch].span;
                self.join_at(span, &then_ty, &else_ty)
            }
            (None, expected) => {
                self.check_expr(then_branch, &Ty::UNIT);
                self.unit_at(span, expected)
            }
        }
    }

    /// 二項演算子の型（6.6）．パイプは呼び出しに読み替える（6.7）．
    fn infer_binary(
        &mut self,
        expr: ExprId,
        op: BinaryOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Ty {
        match op {
            // 算術: 両辺は同じ数の型．
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                let ty = self.infer_expr(lhs);
                self.check_expr(rhs, &ty);
                self.require_numeric(&ty, span);
                ty
            }
            // 比較: 両辺は同じ型．Eq と Ord の検査は Trait の段で行う．
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => {
                let ty = self.infer_expr(lhs);
                self.check_expr(rhs, &ty);
                Ty::BOOL
            }
            // 論理: 両辺は Bool．
            BinaryOp::And | BinaryOp::Or => {
                self.check_expr(lhs, &Ty::BOOL);
                self.check_expr(rhs, &Ty::BOOL);
                Ty::BOOL
            }
            BinaryOp::Pipe => self.infer_pipe(expr, lhs, rhs, span),
        }
    }

    /// `x |> f(a)` は `f(x, a)`，`x |> f` は `f(x)` として推論する（6.7）．
    /// 並べ直した引数（左辺を含む）はパイプの式 `expr` に記録する．呼ばれる側は，右辺が
    /// 呼び出しならその呼ばれる側，そうでなければ右辺そのもの．
    fn infer_pipe(&mut self, expr: ExprId, lhs: ExprId, rhs: ExprId, span: Span) -> Ty {
        let program = self.program;
        let piped = Arg {
            label: None,
            value: lhs,
            punned: false,
        };
        let ty = match &program.exprs[rhs].kind {
            ExprKind::Call { callee, args } => {
                let args: Vec<Arg> = std::iter::once(piped).chain(args.iter().cloned()).collect();
                self.infer_call(expr, *callee, &args, span)
            }
            _ => self.infer_call(expr, rhs, &[piped], span),
        };
        // 右辺の呼び出しの式にも，読み替えた呼び出しの型を記録しておく．
        if matches!(program.exprs[rhs].kind, ExprKind::Call { .. }) {
            self.exprs.insert(rhs, ty.clone());
        }
        ty
    }

    /// ラムダの型．期待する型が同じ数の引数の関数型なら，その引数の型をラムダの引数に先に渡す．
    /// 本体の型を外へ出さない（9.4．fails と use は alpha.2）．
    fn infer_lambda(
        &mut self,
        params: &[Param],
        body: ExprId,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        let expected_fn = match expected.map(|ty| self.ctx.shallow_resolve(ty)) {
            Some(Ty::Fn(f)) if f.params.len() == params.len() => Some(*f),
            _ => None,
        };

        let mut param_tys = Vec::new();
        for (i, param) in params.iter().enumerate() {
            let from_context = expected_fn.as_ref().map(|f| f.params[i].clone());
            let ty = match (param.ty, from_context) {
                (Some(annotation), from_context) => {
                    let ty = self.lower_type(annotation);
                    if let Some(from_context) = from_context {
                        let span = self.program.locals[param.local].span;
                        self.coerce_at(span, &from_context, &ty);
                    }
                    ty
                }
                (None, Some(from_context)) => from_context,
                (None, None) => self.ctx.new_var(),
            };
            self.set_local(param.local, ty.clone());
            param_tys.push(ty);
        }

        let ret_ty = match &expected_fn {
            Some(f) => {
                self.check_expr(body, &f.ret);
                f.ret.clone()
            }
            None => self.infer_expr(body),
        };

        let ty = Ty::func(param_tys, ret_ty);
        // 期待する型が関数型でなかった（か引数の数が違った）ときは，ここで照合して誤りを出す．
        if let (Some(expected), None) = (expected, &expected_fn) {
            self.coerce_at(span, expected, &ty);
        }
        ty
    }

    /// リストの型．要素は全部同じ型で，`..rest` はその型のリスト（2.4，17.5）．
    /// 期待する型がリストなら，その要素の型を各要素に先に渡す．
    fn infer_list(
        &mut self,
        elems: &[ExprId],
        rest: Option<ExprId>,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        let from_context = match expected.map(|ty| self.ctx.shallow_resolve(ty)) {
            Some(Ty::List(elem)) => Some(*elem),
            _ => None,
        };
        let elem_ty = match &from_context {
            Some(elem) => elem.clone(),
            None => self.ctx.new_var(),
        };
        for &elem in elems {
            self.check_expr(elem, &elem_ty);
        }
        let ty = Ty::list(elem_ty);
        if let Some(rest) = rest {
            self.check_expr(rest, &ty);
        }
        if let (Some(expected), None) = (expected, &from_context) {
            self.coerce_at(span, expected, &ty);
        }
        ty
    }

    /// フィールドの参照 `x.name` の型．対象の型（type か error）の宣言からフィールドの型を引く．
    /// 対象の型はこの時点でわかっている必要がある（名前からの逆算はしない）．
    fn infer_field(&mut self, expr: ExprId, base: ExprId, name: &Label) -> Ty {
        let base_ty = self.infer_expr(base);
        let base_ty = self.ctx.shallow_resolve(&base_ty);
        let (id, args) = match &base_ty {
            Ty::Named(id, args) => (*id, args),
            Ty::Error | Ty::Never => return Ty::Error,
            Ty::Var(_) => {
                return self.field_error(
                    name,
                    DiagnosticKind::FieldOnUnknownType {
                        field: name.name.clone(),
                    },
                );
            }
            _ => {
                let ty = self.ctx.resolve(&base_ty);
                return self.field_error(
                    name,
                    DiagnosticKind::NoFields {
                        ty,
                        field: name.name.clone(),
                    },
                );
            }
        };

        let program = self.program;
        let type_def = self.ty_table.def(id);
        let data = &program.defs[type_def];
        if data.is_opaque && data.module != self.module {
            let ty = self.ctx.resolve(&base_ty);
            return self.field_error(
                name,
                DiagnosticKind::OpaqueField {
                    ty,
                    field: name.name.clone(),
                },
            );
        }

        match self.field_ty(type_def, args, &name.name) {
            FieldTy::Found(index, ty) => {
                self.fields.insert(expr, index);
                ty
            }
            FieldTy::NoSuchField => {
                let ty = self.ctx.resolve(&base_ty);
                self.field_error(
                    name,
                    DiagnosticKind::NoField {
                        ty,
                        field: name.name.clone(),
                    },
                )
            }
            FieldTy::NotARecord => {
                let ty = self.ctx.resolve(&base_ty);
                self.field_error(
                    name,
                    DiagnosticKind::NoFields {
                        ty,
                        field: name.name.clone(),
                    },
                )
            }
        }
    }

    /// 型の定義 `type_def` を型引数 `args` で適用した型の，フィールド `field` の型．
    /// フィールドは構成子の宣言の型（ラベルと引数の型）から引く．
    fn field_ty(&mut self, type_def: DefId, args: &[Ty], field: &SmolStr) -> FieldTy {
        let program = self.program;
        let ctor = match program.item(type_def) {
            Some(Item::Type(item)) => match item.ctor {
                Some(ctor) => ctor,
                None => return FieldTy::NotARecord,
            },
            // error は自分自身が構成子．
            Some(Item::Error(_)) => type_def,
            _ => return FieldTy::NotARecord,
        };
        let Some(sig) = self.sigs.get(ctor) else {
            return FieldTy::NotARecord;
        };
        let Ty::Fn(f) = &sig.scheme.ty else {
            // フィールドのない type と error．
            return FieldTy::NoSuchField;
        };
        match sig.labels.iter().position(|l| l.as_ref() == Some(field)) {
            Some(i) => FieldTy::Found(i, substitute_bound(&f.params[i], args)),
            None => FieldTy::NoSuchField,
        }
    }

    /// フィールドの誤りの診断をフィールド名の位置に積み，`Ty::Error` を返す．
    fn field_error(&mut self, name: &Label, kind: DiagnosticKind) -> Ty {
        self.diags.push(Diagnostic {
            span: Some(name.span),
            kind,
        });
        Ty::Error
    }

    /// 部分更新 `C(f: e, ..base)` の型（5.3）．
    ///
    /// 構成子の型を2回具体化する．1つ目が作る値の型，2つ目が `base` の型で，型引数は違ってよい．
    /// 書いたフィールドは1つ目の型で検査し，書いていないフィールドは `base` から引き継ぐので，
    /// 2つの型のそのフィールドの型を単一化する．
    ///
    /// 並べ直した引数は，書いたフィールドだけが `Some` で，書いていないフィールドは `None`
    /// （`base` から引き継ぐ）．
    fn infer_update(&mut self, expr: ExprId, ctor: ExprId, fields: &[Arg], base: ExprId) -> Ty {
        let program = self.program;
        let sigs = self.sigs;
        let ctor_ty = self.infer_expr(ctor);
        let ExprKind::Path(Res::Def(ctor_def)) = program.exprs[ctor].kind else {
            // 構成子でないものは名前解決が誤りを出している．
            for field in fields {
                self.infer_expr(field.value);
            }
            self.infer_expr(base);
            return Ty::Error;
        };
        let Ty::Fn(made) = self.ctx.shallow_resolve(&ctor_ty) else {
            // フィールドのない構成子（か宣言の型のないもの）．
            self.check_expr(base, &ctor_ty);
            for field in fields {
                self.infer_expr(field.value);
            }
            return ctor_ty;
        };
        let Some(sig) = sigs.get(ctor_def) else {
            return Ty::Error;
        };
        let base_ty = self.ctx.instantiate(&sig.scheme);
        let Ty::Fn(from) = base_ty else {
            return Ty::Error;
        };
        self.check_expr(base, &from.ret);

        let mut written = vec![false; made.params.len()];
        let mut slots: Vec<Option<ExprId>> = vec![None; made.params.len()];
        for field in fields {
            let Some(label) = &field.label else {
                // 位置で書いたフィールドは名前解決が誤りを出している．
                self.infer_expr(field.value);
                continue;
            };
            let slot = sig
                .labels
                .iter()
                .position(|l| l.as_ref() == Some(&label.name));
            match slot {
                Some(i) if written[i] => {
                    self.diags.push(Diagnostic {
                        span: Some(label.span),
                        kind: DiagnosticKind::DuplicateArg {
                            label: label.name.clone(),
                        },
                    });
                    self.infer_expr(field.value);
                }
                Some(i) => {
                    written[i] = true;
                    slots[i] = Some(field.value);
                    self.check_expr(field.value, &made.params[i]);
                }
                None => {
                    let ty = self.ctx.resolve(&made.ret);
                    self.diags.push(Diagnostic {
                        span: Some(label.span),
                        kind: DiagnosticKind::NoField {
                            ty,
                            field: label.name.clone(),
                        },
                    });
                    self.infer_expr(field.value);
                }
            }
        }

        // 書いていないフィールドは base から引き継ぐ．
        let base_span = program.exprs[base].span;
        for (i, was_written) in written.iter().enumerate() {
            if !was_written {
                self.coerce_at(base_span, &made.params[i], &from.params[i]);
            }
        }
        self.calls.insert(expr, slots);

        made.ret
    }

    /// 単項演算子の型．`!` は Bool，`-` は数の型（6.6）．
    fn infer_unary(&mut self, op: UnaryOp, operand: ExprId, span: Span) -> Ty {
        match op {
            UnaryOp::Not => {
                self.check_expr(operand, &Ty::BOOL);
                Ty::BOOL
            }
            UnaryOp::Neg => {
                let ty = self.infer_expr(operand);
                self.require_numeric(&ty, span);
                ty
            }
        }
    }

    /// match の型．各腕のパターンに対象の型を渡し，腕の本体の型を合わせる．
    /// 全体の型は `Never` から始めて腕ごとに `join` する．腕のない match は `Never`．
    /// 期待する型があれば，各腕の本体に渡す．
    fn infer_match(&mut self, scrutinee: ExprId, arms: &[Arm], expected: Option<&Ty>) -> Ty {
        let scrutinee_ty = self.infer_expr(scrutinee);
        self.coverage.push(CoverageSite::Match {
            scrutinee,
            ty: scrutinee_ty.clone(),
            arms: arms.to_vec(),
        });

        let mut result = Ty::Never;

        for arm in arms {
            self.bind_pat(arm.pat, &scrutinee_ty);

            if let Some(guard) = arm.guard {
                self.check_expr(guard, &Ty::BOOL);
            }

            match expected {
                Some(expected) => {
                    self.check_expr(arm.body, expected);
                    result = expected.clone();
                }
                None => {
                    let body_ty = self.infer_expr(arm.body);
                    let span = self.program.exprs[arm.body].span;
                    result = self.join_at(span, &result, &body_ty);
                }
            }
        }

        result
    }

    /// 式を推論し，期待する型の位置に置く．
    ///
    /// ブロック，if，match は期待する型を最後の式や各分岐まで渡すので，型が合わないときは
    /// 一番内側の食い違った式を指す．ラムダとリストは期待する型を引数や要素に先に渡す．
    /// それ以外の式は推論してから照合する．
    fn check_expr(&mut self, expr: ExprId, expected: &Ty) {
        let program = self.program;
        let e = &program.exprs[expr];
        match e.kind {
            ExprKind::Block(_)
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::List { .. } => {
                let ty = self.infer_expr_kind(expr, Some(expected));
                self.exprs.insert(expr, ty);
            }
            _ => {
                let actual = self.infer_expr(expr);
                self.coerce_at(e.span, expected, &actual);
            }
        }
    }

    /// 本体の中の型注釈を Ty にする．
    fn lower_type(&mut self, ty: TypeId) -> Ty {
        self.ty_table
            .lower(self.program, &self.type_params, ty, &mut self.diags)
    }

    /// パターンに型を付け，束縛する変数の型を覚える．
    fn bind_pat(&mut self, pat: PatId, ty: &Ty) {
        self.pats.insert(pat, ty.clone());

        let program = self.program;
        let p = &program.pats[pat];
        match &p.kind {
            PatKind::Bind(local) => {
                self.set_local(*local, ty.clone());
            }
            PatKind::Unit => self.coerce_at(p.span, ty, &Ty::UNIT),
            PatKind::Lit(lit) => {
                let lit_ty = self.lit_ty(lit, p.span);
                self.coerce_at(p.span, ty, &lit_ty);
            }
            PatKind::Const(Res::Def(def)) => {
                let const_ty = self.infer_def_ref(*def, p.span);
                self.coerce_at(p.span, ty, &const_ty);
            }
            PatKind::Const(_) => {}
            PatKind::Tuple(elems) => {
                let elem_tys: Vec<Ty> = elems.iter().map(|_| self.ctx.new_var()).collect();
                self.coerce_at(p.span, ty, &Ty::tuple(elem_tys.clone()));
                for (elem, elem_ty) in elems.iter().zip(&elem_tys) {
                    self.bind_pat(*elem, elem_ty);
                }
            }
            PatKind::List { elems, rest } => {
                let elem_ty = self.ctx.new_var();
                self.coerce_at(p.span, ty, &Ty::list(elem_ty.clone()));

                for elem in elems {
                    self.bind_pat(*elem, &elem_ty);
                }

                if let Some(ListRest {
                    binding: Some(local),
                    ..
                }) = rest
                {
                    self.set_local(*local, Ty::list(elem_ty));
                }
            }
            PatKind::Ctor {
                res: Res::Def(def),
                args,
                rest,
            } => self.bind_ctor_pat(pat, *def, args.as_deref(), *rest, ty),
            PatKind::Ctor { .. } => {}
            PatKind::Missing => {}
            PatKind::Wild => {}
        }
    }

    /// 構成子のパターン．構成子の型を具体化し，作る型を `ty` と照合して，フィールドに型を渡す．
    /// フィールドは呼び出しと同じく並べ直す．`..` なしでフィールドを省けば E0308（6.5）．
    fn bind_ctor_pat(
        &mut self,
        pat: PatId,
        def: DefId,
        args: Option<&[PatArg]>,
        rest: bool,
        ty: &Ty,
    ) {
        let span = self.program.pats[pat].span;
        let sigs = self.sigs;
        let program = self.program;

        let ctor_ty = self.infer_def_ref(def, span);
        let labels = sigs.get(def).map(|sig| sig.labels.as_slice());

        let (fields_tys, made_ty) = match self.ctx.shallow_resolve(&ctor_ty) {
            Ty::Fn(f) => (f.params, f.ret),
            other => (Vec::new(), other),
        };

        let ctor_name = &program.defs[def].name;
        let pat_args = args.unwrap_or(&[]);
        let assigned = self.assign_args(
            Some(ctor_name),
            labels,
            fields_tys.len(),
            pat_args.iter().map(|arg| (arg.label.as_ref(), arg.pat)),
        );

        if !assigned.reported {
            if assigned.too_many {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::PatternFieldCount {
                        ctor: ctor_name.clone(),
                        expected: assigned.slots.len(),
                        found: pat_args.len(),
                    },
                });
            } else if assigned.has_missing() && !rest {
                // 埋まっていない名前付きフィールドを全部集める．位置だけのフィールドは名前がない．
                let missing_fields: Vec<SmolStr> = assigned
                    .slots
                    .iter()
                    .zip(labels.unwrap_or(&[]))
                    .filter(|(slot, _)| slot.is_none())
                    .filter_map(|(_, label)| label.clone())
                    .collect();

                if !missing_fields.is_empty() {
                    self.diags.push(Diagnostic {
                        span: Some(span),
                        kind: DiagnosticKind::MissingPatternFields {
                            ctor: ctor_name.clone(),
                            fields: missing_fields,
                        },
                    })
                } else {
                    self.diags.push(Diagnostic {
                        span: Some(span),
                        kind: DiagnosticKind::PatternFieldCount {
                            ctor: ctor_name.clone(),
                            expected: assigned.slots.len(),
                            found: pat_args.len(),
                        },
                    })
                }
            }
        }

        for (slot, field_ty) in assigned.slots.iter().zip(&fields_tys) {
            if let Some(pat) = slot {
                self.bind_pat(*pat, field_ty);
            }
        }

        for extra in assigned.extra {
            self.bind_pat(extra, &Ty::Error);
        }
        self.pat_fields.insert(pat, assigned.slots);

        self.coerce_at(span, ty, &made_ty);
    }

    /// 引数を仮引数の位置に並べ直す．位置引数は前から，名前付き引数はラベルで置く．
    /// 呼び出しの引数（`ExprId`）と構成子のパターンの引数（`PatId`）の両方に使う．
    ///
    /// `labels` は呼ばれる側の仮引数のラベル．名前で指していない値（局所変数，ラムダ）なら `None`．
    /// ラベルの誤りはここで診断を積む．数の誤りは，呼び出しとパターンで規則が違うので呼ぶ側で出す．
    fn assign_args<'l, T: Copy>(
        &mut self,
        callee_name: Option<&SmolStr>,
        labels: Option<&[Option<SmolStr>]>,
        n_params: usize,
        args: impl IntoIterator<Item = (Option<&'l Label>, T)>,
    ) -> Assigned<T> {
        let mut assigned = Assigned {
            slots: vec![None; n_params],
            extra: Vec::new(),
            reported: false,
            too_many: false,
        };
        let mut next_positional = 0;

        for (label, value) in args {
            match label {
                None => {
                    if next_positional < n_params {
                        assigned.slots[next_positional] = Some(value);
                    } else {
                        assigned.extra.push(value);
                        assigned.too_many = true;
                    }
                    next_positional += 1;
                }
                Some(label) => {
                    match label_slot(callee_name, labels, &assigned.slots, &label.name) {
                        Ok(i) => assigned.slots[i] = Some(value),
                        Err(kind) => {
                            self.diags.push(Diagnostic {
                                span: Some(label.span),
                                kind,
                            });
                            assigned.reported = true;
                            assigned.extra.push(value);
                        }
                    }
                }
            }
        }

        assigned
    }

    /// 2つの分岐の共通の型．合わなければ `span` の位置で診断を積み，`a` を全体の型にする．
    fn join_at(&mut self, span: Span, a: &Ty, b: &Ty) -> Ty {
        match self.ctx.join(a, b) {
            Ok(ty) => ty,
            Err(e) => {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::Mismatch(e),
                });
                a.clone()
            }
        }
    }

    /// リテラルの型．整数は Int か Int64 の型変数で，どちらかは文脈で決まる（2.4）．
    fn lit_ty(&mut self, lit: &Literal, span: Span) -> Ty {
        match lit {
            Literal::Int(text) => {
                let ty = self.ctx.new_int_var();
                self.numbers.push(NumConstraint {
                    ty: ty.clone(),
                    span,
                    kind: NumKind::Integer,
                    literal: Some(text.clone()),
                });
                ty
            }
            Literal::Float(_) => Ty::FLOAT,
            Literal::String(_) => Ty::STRING,
        }
    }

    /// `ty` が Int，Int64，Float のどれかであることを求める（算術演算子，単項マイナス）．
    /// 確かめるのは本体を推論し終えたとき．
    fn require_numeric(&mut self, ty: &Ty, span: Span) {
        self.numbers.push(NumConstraint {
            ty: ty.clone(),
            span,
            kind: NumKind::Numeric,
            literal: None,
        });
    }

    /// 数の型の制約を確かめる．まだ決まっていない型変数は Int にする（2.4）．
    /// 整数リテラルは，決まった型の範囲に収まるかも確かめる．
    fn check_numbers(&mut self) {
        for c in std::mem::take(&mut self.numbers) {
            let ty = match self.ctx.shallow_resolve(&c.ty) {
                Ty::Var(_) => {
                    // 何にも決まらなかった数は Int．単一化は必ず成功する．
                    let _ = self.ctx.unify(&Ty::INT, &c.ty);
                    Ty::INT
                }
                ty => ty,
            };
            let ok = match (&ty, c.kind) {
                (Ty::Error | Ty::Never, _) => continue,
                (Ty::Prim(Prim::Int | Prim::Int64), _) => true,
                (Ty::Prim(Prim::Float), NumKind::Numeric) => true,
                _ => false,
            };
            if !ok {
                self.diags.push(Diagnostic {
                    span: Some(c.span),
                    kind: DiagnosticKind::NotNumeric {
                        found: self.ctx.resolve(&ty),
                        integer: c.kind == NumKind::Integer,
                    },
                });
                continue;
            }
            if let Some(literal) = c.literal
                && !int_literal_fits(&literal, &ty)
            {
                self.diags.push(Diagnostic {
                    span: Some(c.span),
                    kind: DiagnosticKind::IntLiteralOutOfRange { literal, ty },
                });
            }
        }
    }
}

/// 表の型をすべて，単一化の束縛を辿った形にする．
fn resolve_all<T>(ctx: &mut InferCtx, map: &ArenaMap<Idx<T>, Ty>) -> ArenaMap<Idx<T>, Ty> {
    let mut out = ArenaMap::default();
    for (id, ty) in map.iter() {
        out.insert(id, ctx.resolve(ty));
    }
    out
}

/// 引数を仮引数の位置に並べ直した結果（[`BodyInfer::assign_args`]）．
struct Assigned<T> {
    /// 位置ごとの引数．埋まらなければ `None`．
    slots: Vec<Option<T>>,
    /// どこにも置けなかった引数．照合はせず，型を記録するためだけに推論する．
    extra: Vec<T>,
    /// ラベルの誤りの診断を出したか．出していれば数の誤りは重ねない．
    reported: bool,
    /// 位置引数が仮引数より多かったか．
    too_many: bool,
}

impl<T> Assigned<T> {
    /// 埋まっていない位置があるか．
    fn has_missing(&self) -> bool {
        self.slots.iter().any(Option::is_none)
    }
}

/// 名前付き引数を置く位置．置けなければ，その理由の診断の中身．
fn label_slot<T>(
    callee_name: Option<&SmolStr>,
    labels: Option<&[Option<SmolStr>]>,
    slots: &[Option<T>],
    label: &SmolStr,
) -> Result<usize, DiagnosticKind> {
    let labels = labels.ok_or_else(|| DiagnosticKind::LabelOnUnnamedCallee {
        label: label.clone(),
    })?;

    let i = labels
        .iter()
        .position(|l| l.as_ref() == Some(label))
        .ok_or_else(|| DiagnosticKind::UnknownLabel {
            callee: callee_name.cloned().unwrap_or_default(),
            label: label.clone(),
        })?;

    if slots[i].is_some() {
        return Err(DiagnosticKind::DuplicateArg {
            label: label.clone(),
        });
    }

    Ok(i)
}

/// 数の型の制約．
struct NumConstraint {
    ty: Ty,
    /// 診断の位置．リテラルか演算子の式．
    span: Span,
    kind: NumKind,
    /// 整数リテラルなら，その数字の列（`_` を除いたもの．`0x`，`0b` の接頭辞は残る）．
    literal: Option<SmolStr>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NumKind {
    /// Int か Int64．整数リテラル．
    Integer,
    /// Int，Int64，Float．算術演算子と単項マイナス．
    Numeric,
}

/// 整数リテラルが型の範囲に収まるか．負の数は単項マイナスの式なので，ここでは正の数だけを見る．
/// 2の補数として読み替えない（`0xFFFF_FFFF` は Int に収まらない，2.4）．
fn int_literal_fits(literal: &str, ty: &Ty) -> bool {
    let max = match ty {
        Ty::Prim(Prim::Int) => i32::MAX as u128,
        Ty::Prim(Prim::Int64) => i64::MAX as u128,
        _ => return true,
    };
    let (digits, radix) = if let Some(hex) = literal.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(bin) = literal.strip_prefix("0b") {
        (bin, 2)
    } else {
        (literal, 10)
    };
    u128::from_str_radix(digits, radix).is_ok_and(|n| n <= max)
}

/// フィールドの型を引いた結果．
enum FieldTy {
    /// フィールドの添字と型．
    Found(usize, Ty),
    /// 型はフィールドを持つが，その名前のフィールドがない．
    NoSuchField,
    /// フィールドを持たない型（enum，フィールドのない `@external` の type）．
    NotARecord,
}

/// まとまりの中の，書いた型引数のある宣言の型のない定義への参照．
struct DeferredRef {
    def: DefId,
    /// 型引数に置いた型変数．
    args: Vec<Ty>,
    /// 参照の式に付けた型．
    instance: Ty,
    span: Span,
}

/// 網羅性を確かめる場所．
enum CoverageSite {
    Match {
        scrutinee: ExprId,
        ty: Ty,
        arms: Vec<Arm>,
    },
    /// 束縛 `pat = value`．パターンは必ず成功しなければならない（6.3）．
    Binding { pat: PatId, ty: Ty },
}

/// 足りない値の例を文字列にする．
fn witnesses(missing: &[Witness]) -> Vec<SmolStr> {
    missing
        .iter()
        .map(|w| SmolStr::new(w.to_string()))
        .collect()
}
