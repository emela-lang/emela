//! プログラム全体の型検査．宣言の型を集め，定義の本体を呼び出しの順に推論する．
//!
//! 定義（本体のある fn と const）を，本体の中で参照する定義への辺で結んだグラフにし，
//! 強連結成分（互いに参照し合うまとまり）ごとに，参照される側から順に推論する（Tarjan）．
//! 宣言の型のない定義は，まとまりを推論し終えたところで汎化して宣言の型の表に足す．
//! まとまりの中では単相なので，宣言の型のない定義どうしの再帰は多相にならない．

use emela_resolve::hir::{
    Block, DefId, ExprId, ExprKind, Item, LocalId, PatId, PatKind, Program, Res, Stmt, StrPart,
};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;

use crate::body::BodyInfer;
use crate::diagnostic::Diagnostic;
use crate::lower::TyTable;
use crate::signature::Signatures;
use crate::ty::Ty;

/// プログラム全体の型検査の結果．HIR の番号をキーにした表を並べたもの．
///
/// lowering は HIR を辿りながら，必要な情報をこの表から引く（NOTES.md「結果の形」）．
/// 型は単一化の束縛を辿った形．決まらずに残った型変数（使われない `None` など）は
/// 型変数のまま残る．lowering は `()` として扱う．
pub struct ProgramTypes {
    pub ty_table: TyTable,
    /// fn，構成子，const の宣言の型．宣言の型のなかった定義は推論して汎化した型．
    pub sigs: Signatures,
    pub exprs: ArenaMap<ExprId, Ty>,
    pub pats: ArenaMap<PatId, Ty>,
    pub locals: ArenaMap<LocalId, Ty>,
    /// 局所変数がどの定義の本体のものか．
    pub local_owner: ArenaMap<LocalId, DefId>,
    /// フィールドの参照の式（`ExprKind::Field`）→ フィールドの添字（構成子の宣言の順）．
    pub fields: ArenaMap<ExprId, usize>,
    /// 呼び出し（`Call`），パイプ（`Binary { op: Pipe }`），部分更新（`Update`）の式 →
    /// 仮引数（フィールド）の位置の順に並べ直した引数．
    /// パイプは左辺を先頭に含む．部分更新は書いたフィールドだけが `Some`．
    pub calls: ArenaMap<ExprId, Vec<Option<ExprId>>>,
    /// 汎用の定義の参照の式（`Path`）→ 具体化した型引数（宣言の型引数の順）．
    pub type_args: ArenaMap<ExprId, Vec<Ty>>,
    /// 構成子のパターン → フィールドの位置の順に並べ直したパターン．省いたフィールドは `None`．
    pub pat_fields: ArenaMap<PatId, Vec<Option<PatId>>>,
    pub diags: Vec<Diagnostic>,
}

/// プログラム全体を型検査する．
pub fn check_program(program: &Program) -> ProgramTypes {
    let ty_table = TyTable::new(program);
    let mut diags = Vec::new();
    let mut sigs = Signatures::collect(program, &ty_table, &mut diags);

    let mut types = ProgramTypes {
        ty_table,
        sigs: Signatures::new(),
        exprs: ArenaMap::default(),
        pats: ArenaMap::default(),
        locals: ArenaMap::default(),
        local_owner: ArenaMap::default(),
        fields: ArenaMap::default(),
        calls: ArenaMap::default(),
        type_args: ArenaMap::default(),
        pat_fields: ArenaMap::default(),
        diags,
    };

    for group in groups(program) {
        let mut infer = BodyInfer::new(program, &types.ty_table, &sigs);
        for &def in &group {
            if sigs.get(def).is_none() {
                infer.declare(def);
            }
        }
        for &def in &group {
            infer.infer_def(def);
        }
        let body = infer.finish();
        for (def, sig) in body.inferred {
            sigs.insert(def, sig);
        }
        types
            .exprs
            .extend(body.exprs.iter().map(|(id, ty)| (id, ty.clone())));
        types
            .pats
            .extend(body.pats.iter().map(|(id, ty)| (id, ty.clone())));
        types
            .locals
            .extend(body.locals.iter().map(|(id, ty)| (id, ty.clone())));
        types
            .local_owner
            .extend(body.local_owner.iter().map(|(id, &def)| (id, def)));
        types
            .fields
            .extend(body.fields.iter().map(|(id, &i)| (id, i)));
        types
            .calls
            .extend(body.calls.iter().map(|(id, args)| (id, args.clone())));
        types
            .type_args
            .extend(body.type_args.iter().map(|(id, args)| (id, args.clone())));
        types
            .pat_fields
            .extend(body.pat_fields.iter().map(|(id, pats)| (id, pats.clone())));
        types.diags.extend(body.diags);
    }

    types.sigs = sigs;
    types
}

/// 推論する定義（本体のある fn と const）を，参照される側が先になる順に，
/// 互いに参照し合うまとまりに分けて並べる．
fn groups(program: &Program) -> Vec<Vec<DefId>> {
    let nodes: Vec<DefId> = program
        .defs
        .iter()
        .map(|(def, _)| def)
        .filter(|&def| match program.item(def) {
            Some(Item::Fn(item)) => item.body.is_some(),
            Some(Item::Const(_)) => true,
            _ => false,
        })
        .collect();
    let is_node: FxHashMap<DefId, ()> = nodes.iter().map(|&def| (def, ())).collect();

    let mut edges: FxHashMap<DefId, Vec<DefId>> = FxHashMap::default();
    for &def in &nodes {
        let mut refs = Vec::new();
        match program.item(def) {
            Some(Item::Fn(item)) => {
                if let Some(body) = item.body {
                    collect_expr_refs(program, body, &mut refs);
                }
            }
            Some(Item::Const(item)) => collect_expr_refs(program, item.value, &mut refs),
            _ => {}
        }
        refs.retain(|r| is_node.contains_key(r));
        refs.dedup();
        edges.insert(def, refs);
    }

    let mut tarjan = Tarjan {
        edges: &edges,
        index: FxHashMap::default(),
        low: FxHashMap::default(),
        on_stack: FxHashMap::default(),
        stack: Vec::new(),
        next: 0,
        groups: Vec::new(),
    };
    for &def in &nodes {
        if !tarjan.index.contains_key(&def) {
            tarjan.visit(def);
        }
    }
    tarjan.groups
}

/// Tarjan の強連結成分の分解．成分は，そこから辿れる成分の後に並ぶ（参照される側が先）．
struct Tarjan<'g> {
    edges: &'g FxHashMap<DefId, Vec<DefId>>,
    index: FxHashMap<DefId, usize>,
    low: FxHashMap<DefId, usize>,
    on_stack: FxHashMap<DefId, bool>,
    stack: Vec<DefId>,
    next: usize,
    groups: Vec<Vec<DefId>>,
}

impl Tarjan<'_> {
    fn visit(&mut self, v: DefId) {
        self.index.insert(v, self.next);
        self.low.insert(v, self.next);
        self.next += 1;
        self.stack.push(v);
        self.on_stack.insert(v, true);

        let edges = self.edges;
        for &w in edges.get(&v).map(Vec::as_slice).unwrap_or(&[]) {
            if !self.index.contains_key(&w) {
                self.visit(w);
                let low = self.low[&v].min(self.low[&w]);
                self.low.insert(v, low);
            } else if self.on_stack.get(&w).copied().unwrap_or(false) {
                let low = self.low[&v].min(self.index[&w]);
                self.low.insert(v, low);
            }
        }

        if self.low[&v] == self.index[&v] {
            let mut group = Vec::new();
            loop {
                let w = self.stack.pop().expect("v はスタックにある");
                self.on_stack.insert(w, false);
                group.push(w);
                if w == v {
                    break;
                }
            }
            // まとまりの中は定義の順に推論する（診断の順を安定させるため）．
            group.sort_by_key(|def| def.into_raw());
            self.groups.push(group);
        }
    }
}

/// 式の中で参照している定義を集める．
fn collect_expr_refs(program: &Program, expr: ExprId, out: &mut Vec<DefId>) {
    let go = |e: ExprId, out: &mut Vec<DefId>| collect_expr_refs(program, e, out);
    match &program.exprs[expr].kind {
        ExprKind::Path(Res::Def(def)) => out.push(*def),
        ExprKind::Missing
        | ExprKind::Lit(_)
        | ExprKind::Path(_)
        | ExprKind::Unit
        | ExprKind::Use { .. } => {}
        ExprKind::Interpolated(parts) => {
            for part in parts {
                if let StrPart::Interp(e) = part {
                    go(*e, out);
                }
            }
        }
        ExprKind::Field { base, .. } => go(*base, out),
        ExprKind::Call { callee, args } => {
            go(*callee, out);
            for arg in args {
                go(arg.value, out);
            }
        }
        ExprKind::Unary { operand, .. } => go(*operand, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            go(*lhs, out);
            go(*rhs, out);
        }
        ExprKind::Tuple(elems) => elems.iter().for_each(|&e| go(e, out)),
        ExprKind::List { elems, rest } => {
            elems.iter().for_each(|&e| go(e, out));
            if let Some(rest) = rest {
                go(*rest, out);
            }
        }
        ExprKind::Block(block) => collect_block_refs(program, block, out),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            go(*cond, out);
            go(*then_branch, out);
            if let Some(e) = else_branch {
                go(*e, out);
            }
        }
        ExprKind::Match { scrutinee, arms }
        | ExprKind::Escape {
            expr: scrutinee,
            arms,
        } => {
            go(*scrutinee, out);
            for arm in arms {
                collect_pat_refs(program, arm.pat, out);
                if let Some(guard) = arm.guard {
                    go(guard, out);
                }
                go(arm.body, out);
            }
        }
        ExprKind::Fail(e) | ExprKind::Assert(e) => go(*e, out),
        ExprKind::Update { ctor, fields, base } => {
            go(*ctor, out);
            for field in fields {
                go(field.value, out);
            }
            go(*base, out);
        }
        ExprKind::With { body, .. } => go(*body, out),
        ExprKind::Lambda { body, .. } => go(*body, out),
    }
}

fn collect_block_refs(program: &Program, block: &Block, out: &mut Vec<DefId>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { pat, value, .. } => {
                collect_pat_refs(program, *pat, out);
                collect_expr_refs(program, *value, out);
            }
            Stmt::Expr(e) => collect_expr_refs(program, *e, out),
        }
    }
    if let Some(tail) = block.tail {
        collect_expr_refs(program, tail, out);
    }
}

/// パターンの中で参照している const を集める．
fn collect_pat_refs(program: &Program, pat: PatId, out: &mut Vec<DefId>) {
    match &program.pats[pat].kind {
        PatKind::Const(Res::Def(def)) => out.push(*def),
        PatKind::Ctor { args, .. } => {
            for arg in args.iter().flatten() {
                collect_pat_refs(program, arg.pat, out);
            }
        }
        PatKind::Tuple(pats) => pats.iter().for_each(|&p| collect_pat_refs(program, p, out)),
        PatKind::List { elems, .. } => elems
            .iter()
            .for_each(|&p| collect_pat_refs(program, p, out)),
        _ => {}
    }
}
