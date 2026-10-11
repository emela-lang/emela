//! 関数の本体の推論．式とパターンに型を付ける．
//!
//! 関数の本体1つごとに [`BodyInfer`] を作り，式を下から順に推論する．型が合わなければ
//! 式の位置を付けた診断を積んで続ける．

use emela_resolve::hir::{
    Arg, Arm, BinaryOp, Block, DefId, ExprId, ExprKind, FnItem, Item, Label, ListRest, Literal,
    LocalId, PatArg, PatId, PatKind, Program, Res, Span, Stmt, TypeId, UnaryOp,
};
use la_arena::{ArenaMap, Idx};
use smol_str::SmolStr;

use crate::FnTy;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::InferCtx;
use crate::lower::TyTable;
use crate::signature::{Signature, Signatures};
use crate::ty::{Prim, Ty};

/// fn の本体1つの推論の結果．型は単一化の束縛を辿った形．
///
/// 形は仮．lowering に渡す形はステップ6で決める．
#[derive(Debug)]
pub struct BodyTypes {
    /// fn の型．
    pub fn_ty: Ty,
    pub exprs: ArenaMap<ExprId, Ty>,
    pub pats: ArenaMap<PatId, Ty>,
    pub locals: ArenaMap<LocalId, Ty>,
    pub diags: Vec<Diagnostic>,
}

/// トップレベルの fn `def` の本体を推論する．
pub fn infer_fn_body(
    program: &Program,
    ty_table: &TyTable,
    sigs: &Signatures,
    def: DefId,
) -> Option<BodyTypes> {
    let Some(Item::Fn(item)) = program.item(def) else {
        return None;
    };
    let type_params = item.sig.type_params.iter().map(|tp| tp.def).collect();
    let mut infer = BodyInfer::new(program, ty_table, sigs, type_params);
    let fn_ty = infer.infer_fn(item, sigs.get(def));
    Some(infer.finish(fn_ty))
}

/// 関数の本体1つを推論する間の状態．
pub(crate) struct BodyInfer<'a> {
    program: &'a Program,
    ty_table: &'a TyTable,
    sigs: &'a Signatures,
    type_params: Vec<DefId>,
    ctx: InferCtx,
    locals: ArenaMap<LocalId, Ty>,
    exprs: ArenaMap<ExprId, Ty>,
    pats: ArenaMap<PatId, Ty>,
    diags: Vec<Diagnostic>,
    /// 数の型の制約．本体を推論し終えたときに確かめる（[`Self::check_numbers`]）．
    numbers: Vec<NumConstraint>,
}

impl<'a> BodyInfer<'a> {
    /// 推論している fn の型引数 `type_params` を受け取り，空の状態で始める．
    pub(crate) fn new(
        program: &'a Program,
        ty_table: &'a TyTable,
        sigs: &'a Signatures,
        type_params: Vec<DefId>,
    ) -> Self {
        BodyInfer {
            program,
            ty_table,
            sigs,
            type_params,
            ctx: InferCtx::new(),
            locals: ArenaMap::new(),
            exprs: ArenaMap::new(),
            pats: ArenaMap::new(),
            diags: Vec::new(),
            numbers: Vec::new(),
        }
    }

    /// 推論を終え，型を辿った形にして返す．
    pub(crate) fn finish(mut self, fn_ty: Ty) -> BodyTypes {
        self.check_numbers();
        let fn_ty = self.ctx.resolve(&fn_ty);
        let exprs = resolve_all(&mut self.ctx, &self.exprs);
        let pats = resolve_all(&mut self.ctx, &self.pats);
        let locals = resolve_all(&mut self.ctx, &self.locals);
        BodyTypes {
            fn_ty,
            exprs,
            pats,
            locals,
            diags: self.diags,
        }
    }

    /// fn の本体を推論し，fn の型を返す．
    pub(crate) fn infer_fn(&mut self, item: &FnItem, sig: Option<&Signature>) -> Ty {
        let (params_tys, declared_ret): (Vec<Ty>, Option<Ty>) = match sig {
            Some(sig) => {
                let Ty::Fn(f) = &sig.scheme.ty else {
                    return Ty::Error;
                };
                (f.params.clone(), Some(f.ret.clone()))
            }
            None => {
                let tys = item
                    .sig
                    .params
                    .iter()
                    .map(|p| p.ty.map(|ty| self.lower_type(ty)).unwrap_or(Ty::Error))
                    .collect();
                (tys, None)
            }
        };

        for (p, ty) in item.sig.params.iter().zip(&params_tys) {
            self.locals.insert(p.local, ty.clone());
        }

        let ret_ty = match (item.body, declared_ret) {
            (Some(body), Some(ret)) => {
                self.check_expr(body, &ret);
                ret
            }
            (Some(body), None) => self.infer_expr(body),
            (None, Some(ret)) => ret,
            (None, None) => Ty::Error,
        };

        Ty::func(params_tys, ret_ty)
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
            ExprKind::Path(Res::Def(def)) => self.infer_def_ref(*def, e.span),
            ExprKind::Path(Res::Err) => Ty::Error,
            ExprKind::Block(block) => self.infer_block(block, e.span, expected),
            ExprKind::Call { callee, args } => self.infer_call(*callee, args, e.span),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.infer_if(*cond, *then_branch, *else_branch, e.span, expected),
            ExprKind::Match { scrutinee, arms } => self.infer_match(*scrutinee, arms, expected),
            ExprKind::Binary { op, lhs, rhs } => self.infer_binary(*op, *lhs, *rhs, e.span),
            ExprKind::Unary { op, operand } => self.infer_unary(*op, *operand, e.span),
            _ => {
                self.diags.push(Diagnostic {
                    span: Some(e.span),
                    kind: DiagnosticKind::Unsupported { what: "expression" },
                });
                Ty::Error
            }
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
    fn infer_def_ref(&mut self, def: DefId, span: Span) -> Ty {
        let sigs = self.sigs;
        if let Some(sig) = sigs.get(def) {
            self.ctx.instantiate(&sig.scheme)
        } else {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::Unsupported { what: "def" },
            });
            Ty::Error
        }
    }

    /// 呼び出しの型．引数を仮引数の位置に並べ直してから，型を照合する（6.2）．
    fn infer_call(&mut self, callee: ExprId, args: &[Arg], span: Span) -> Ty {
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

        for expr in assigned.extra {
            self.infer_expr(expr);
        }

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
    fn infer_binary(&mut self, op: BinaryOp, lhs: ExprId, rhs: ExprId, span: Span) -> Ty {
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
            BinaryOp::Pipe => self.infer_pipe(lhs, rhs, span),
        }
    }

    /// `x |> f(a)` は `f(x, a)`，`x |> f` は `f(x)` として推論する（6.7）．
    fn infer_pipe(&mut self, lhs: ExprId, rhs: ExprId, span: Span) -> Ty {
        let program = self.program;
        let piped = Arg {
            label: None,
            value: lhs,
            punned: false,
        };
        let ty = match &program.exprs[rhs].kind {
            ExprKind::Call { callee, args } => {
                let args: Vec<Arg> = std::iter::once(piped).chain(args.iter().cloned()).collect();
                self.infer_call(*callee, &args, span)
            }
            _ => self.infer_call(rhs, &[piped], span),
        };
        // 右辺の呼び出しの式にも，読み替えた呼び出しの型を記録しておく．
        if matches!(program.exprs[rhs].kind, ExprKind::Call { .. }) {
            self.exprs.insert(rhs, ty.clone());
        }
        ty
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
    /// 一番内側の食い違った式を指す．それ以外の式は推論してから照合する．
    fn check_expr(&mut self, expr: ExprId, expected: &Ty) {
        let program = self.program;
        let e = &program.exprs[expr];
        match e.kind {
            ExprKind::Block(_) | ExprKind::If { .. } | ExprKind::Match { .. } => {
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
                self.locals.insert(*local, ty.clone());
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
                    self.locals.insert(*local, Ty::list(elem_ty));
                }
            }
            PatKind::Ctor {
                res: Res::Def(def),
                args,
                rest,
            } => self.bind_ctor_pat(p.span, *def, args.as_deref(), *rest, ty),
            PatKind::Ctor { .. } => {}
            PatKind::Missing => {}
            PatKind::Wild => {}
        }
    }

    /// 構成子のパターン．構成子の型を具体化し，作る型を `ty` と照合して，フィールドに型を渡す．
    /// フィールドは呼び出しと同じく並べ直す．`..` なしでフィールドを省けば E0308（6.5）．
    fn bind_ctor_pat(
        &mut self,
        span: Span,
        def: DefId,
        args: Option<&[PatArg]>,
        rest: bool,
        ty: &Ty,
    ) {
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

        for pat in assigned.extra {
            self.bind_pat(pat, &Ty::Error);
        }

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
