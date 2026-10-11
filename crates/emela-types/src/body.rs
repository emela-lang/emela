//! 関数の本体の推論．式とパターンに型を付ける．
//!
//! 関数の本体1つごとに [`BodyInfer`] を作り，式を下から順に推論する．型が合わなければ
//! 式の位置を付けた診断を積んで続ける．

use emela_resolve::hir::{
    Block, DefId, ExprId, ExprKind, FnItem, Item, Literal, LocalId, PatId, PatKind, Program, Res,
    Span, Stmt, TypeId,
};
use la_arena::{ArenaMap, Idx};

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::InferCtx;
use crate::lower::TyTable;
use crate::signature::{Signature, Signatures};
use crate::ty::Ty;

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
        }
    }

    /// 推論を終え，型を辿った形にして返す．
    pub(crate) fn finish(mut self, fn_ty: Ty) -> BodyTypes {
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
        let ty = self.infer_expr_kind(expr);
        self.exprs.insert(expr, ty.clone());
        ty
    }

    /// 式の種類ごとの推論．
    fn infer_expr_kind(&mut self, expr: ExprId) -> Ty {
        let program = self.program;
        let e = &program.exprs[expr];
        match &e.kind {
            ExprKind::Missing => Ty::Error,
            ExprKind::Lit(Literal::Int(_)) => Ty::INT,
            ExprKind::Lit(Literal::Float(_)) => Ty::FLOAT,
            ExprKind::Lit(Literal::String(_)) => Ty::STRING,
            ExprKind::Unit => Ty::UNIT,
            ExprKind::Tuple(elems) => Ty::tuple(elems.iter().map(|&e| self.infer_expr(e))),
            ExprKind::Path(Res::Local(local)) => {
                self.locals.get(*local).cloned().unwrap_or(Ty::Error)
            }
            ExprKind::Block(block) => self.infer_block(block),
            _ => {
                self.diags.push(Diagnostic {
                    span: Some(e.span),
                    kind: DiagnosticKind::Unsupported { what: "expression" },
                });
                Ty::Error
            }
        }
    }

    /// 式を推論し，期待する型の位置に置く．
    fn check_expr(&mut self, expr: ExprId, expected: &Ty) {
        let program = self.program;
        let e = &program.exprs[expr];
        let actual = self.infer_expr(expr);
        self.coerce_at(e.span, expected, &actual)
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
            PatKind::Wild => {}
            _ => {
                self.diags.push(Diagnostic {
                    span: Some(p.span),
                    kind: DiagnosticKind::Unsupported { what: "pattern" },
                });
            }
        }
    }

    /// ブロックの型．最後が束縛か空なら `()`．
    fn infer_block(&mut self, block: &Block) -> Ty {
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
        match block.tail {
            Some(expr) => self.infer_expr(expr),
            None => Ty::UNIT,
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
