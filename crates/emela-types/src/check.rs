//! Type checking of a whole program: collects declared types and infers definition bodies in
//! call order.
//!
//! Definitions (fns with bodies, consts and handlers) form a graph with edges to the definitions
//! their bodies refer to. Strongly connected components (groups that refer to each other) are
//! inferred callees first (Tarjan). Definitions without a declared type are generalized and added
//! to the declared types once their group is inferred. Inside a group they are monomorphic, so
//! recursion between definitions without declared types is not polymorphic.

use emela_resolve::hir::{
    Block, DefId, ExprId, ExprKind, Item, LocalId, PatId, PatKind, Program, Res, Span, Stmt,
    StrPart,
};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;

use crate::body::{BodyInfer, HandlerFacts};
use crate::diagnostic::{Diagnostic, EffectContext};
use crate::layer::{self, LayerInfo};
use crate::lower::TyTable;
use crate::row::{Row, RowVar};
use crate::signature::Signatures;
use crate::traits::Traits;
use crate::ty::{Ty, TyVar};

/// Tables keyed by HIR ids.
///
/// Lowering walks the HIR and looks up what it needs in these tables (see NOTES.md). Types have
/// unification bindings followed. Type variables that were never determined (an unused `None`,
/// say) remain type variables; lowering treats them as `()`.
pub struct ProgramTypes {
    pub ty_table: TyTable,
    /// Declared types of fns, constructors and consts. Definitions without a declared type get
    /// their inferred, generalized type.
    pub sigs: Signatures,
    pub exprs: ArenaMap<ExprId, Ty>,
    pub pats: ArenaMap<PatId, Ty>,
    pub locals: ArenaMap<LocalId, Ty>,
    /// The definition whose body a local variable belongs to.
    pub local_owner: ArenaMap<LocalId, DefId>,
    /// Field access expressions (`ExprKind::Field`) → the field index (constructor declaration
    /// order).
    pub fields: ArenaMap<ExprId, usize>,
    /// Call (`Call`), pipe (`Binary { op: Pipe }`) and update (`Update`) expressions → the
    /// arguments reordered by parameter (field) position. Pipes include the left operand first.
    /// Updates have `Some` only for the fields written.
    pub calls: ArenaMap<ExprId, Vec<Option<ExprId>>>,
    /// References to generic definitions (`Path`) → the instantiated type arguments, in the
    /// declaration's type parameter order.
    pub type_args: ArenaMap<ExprId, Vec<Ty>>,
    /// References to generic definitions (`Path`) → the instantiated sets, in the declaration's
    /// type parameter order (empty at positions that are not set parameters). Unlike `type_args`,
    /// these hold the sets instantiating set type parameters. Lowering uses them to decide, per
    /// instantiation, whether a function polymorphic in R may suspend.
    pub type_rows: ArenaMap<ExprId, Vec<Row>>,
    /// In the bodies of definitions without a declared type: generalized set variable → index of
    /// the type parameter in the declared type (`sigs`). The bodies' expression types and sets
    /// (`exprs`, `effects`) still contain the generalized set variables as `RowElem::Var`.
    pub generalized_rows: ArenaMap<DefId, Vec<(RowVar, u32)>>,
    /// For a definition without a declared type: generalized type variables in its body → the
    /// index of the type parameter of the declared type. Body expression types keep generalized
    /// variables as `Ty::Var`; lowering uses this to treat the `A` of `use State[A]` in the body
    /// as a type parameter (the evidence table key, spec 8.1).
    pub generalized_tys: ArenaMap<DefId, Vec<(TyVar, u32)>>,
    /// Constructor patterns → the patterns reordered by field position; omitted fields are
    /// `None`.
    pub pat_fields: ArenaMap<PatId, Vec<Option<PatId>>>,
    /// The error set E of each expression (spec 9.4), with set variables replaced by their
    /// solutions. In the bodies of generalized definitions, the generalized set variables remain
    /// as `RowElem::Var`.
    pub errors: ArenaMap<ExprId, Row>,
    /// The effect set R of each expression (spec 9.4), with set variables replaced by their
    /// solutions as in `errors`.
    pub effects: ArenaMap<ExprId, Row>,
    /// References to capability operations (the `Field` expression of `log.info`) → the effect
    /// operation's definition.
    pub ops: ArenaMap<ExprId, DefId>,
    /// A handler's dependencies (the union of the R of init, release and the operation bodies;
    /// "deps(H)" in spec 9.4). Only for inferred handlers.
    pub handler_uses: ArenaMap<DefId, Row>,
    /// The E of a handler's init, which goes into the E of `with` (spec 9.4). Only for inferred
    /// handlers.
    pub handler_fails: ArenaMap<DefId, Row>,
    /// `with` expressions → handlers in initialization order (spec 14.2). Layers are expanded, and
    /// for handlers of the same effect only the last one is kept (spec 8.3, 8.5). Release runs in
    /// reverse order.
    pub with_handlers: ArenaMap<ExprId, crate::layer::WithInfo>,
    /// Layer → its expansion (handlers in initialization order and the dependencies the layer
    /// leaves unmet).
    pub layers: ArenaMap<DefId, LayerInfo>,
    /// Supertraits and impls (spec 10.1–10.2).
    pub traits: Traits,
    pub diags: Vec<Diagnostic>,
}

/// Names the definitions whose R must be empty (`main` in spec 8.3, test functions in spec
/// 13.3). The driver decides which is the entry's `main` and which are test functions.
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// The entry module's `main`.
    pub main: Option<DefId>,
    pub tests: Vec<DefId>,
}

pub fn check_program(program: &Program) -> ProgramTypes {
    check_program_with(program, &CheckOptions::default())
}

pub fn check_program_with(program: &Program, options: &CheckOptions) -> ProgramTypes {
    let mut closed: FxHashMap<DefId, EffectContext> = FxHashMap::default();
    if let Some(main) = options.main {
        closed.insert(main, EffectContext::Main);
    }
    for &test in &options.tests {
        let name = program.defs[test].name.clone();
        closed.insert(test, EffectContext::Test { name });
    }
    let ty_table = TyTable::new(program);
    let mut diags = Vec::new();
    let mut sigs = Signatures::collect(program, &ty_table, &mut diags);
    let traits = crate::traits::collect(program, &ty_table, &mut sigs, &mut diags);
    crate::const_cycle::check(program, &mut diags);

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
        type_rows: ArenaMap::default(),
        generalized_rows: ArenaMap::default(),
        generalized_tys: ArenaMap::default(),
        pat_fields: ArenaMap::default(),
        errors: ArenaMap::default(),
        effects: ArenaMap::default(),
        ops: ArenaMap::default(),
        handler_uses: ArenaMap::default(),
        handler_fails: ArenaMap::default(),
        with_handlers: ArenaMap::default(),
        layers: ArenaMap::default(),
        traits,
        diags,
    };

    let fibers = crate::fiber::Fibers::new(program);
    for group in groups(program, &sigs) {
        let mut infer = BodyInfer::new(
            program,
            &types.ty_table,
            &sigs,
            HandlerFacts {
                uses: &types.handler_uses,
                fails: &types.handler_fails,
            },
            &closed,
            &fibers,
            &types.traits,
        );
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
            .type_rows
            .extend(body.type_rows.iter().map(|(id, rows)| (id, rows.clone())));
        types.generalized_rows.extend(body.generalized_rows);
        types.generalized_tys.extend(body.generalized_tys);
        types
            .pat_fields
            .extend(body.pat_fields.iter().map(|(id, pats)| (id, pats.clone())));
        types
            .errors
            .extend(body.errors.iter().map(|(id, row)| (id, row.clone())));
        types
            .effects
            .extend(body.effects.iter().map(|(id, row)| (id, row.clone())));
        types.ops.extend(body.ops.iter().map(|(id, &op)| (id, op)));
        types.handler_uses.extend(body.handler_uses);
        types.handler_fails.extend(body.handler_fails);
        types
            .with_handlers
            .extend(body.with_handlers.iter().map(|(id, hs)| (id, hs.clone())));
        types.diags.extend(body.diags);
    }

    crate::traits::unused_derives(program, &types.traits, &mut types.diags);
    types.layers = layer::check_layers(
        program,
        &sigs,
        &types.ty_table.cons,
        &types.handler_uses,
        &mut types.diags,
    );
    types.sigs = sigs;
    types
}

/// Orders the definitions to infer (fns with bodies, consts, handlers, trait default
/// implementations and impl functions with signatures) into groups that refer to each other,
/// callees first.
fn groups(program: &Program, sigs: &Signatures) -> Vec<Vec<DefId>> {
    let nodes: Vec<DefId> = program
        .defs
        .iter()
        .map(|(def, _)| def)
        .filter(|&def| match program.item(def) {
            Some(Item::Fn(item)) => item.body.is_some(),
            Some(Item::TraitFn(item)) => item.body.is_some() && sigs.get(def).is_some(),
            Some(Item::Const(_) | Item::Handler(_)) => true,
            // An impl function without a signature (its impl or trait function has an error) is
            // not inferred.
            _ => program
                .impl_fn(def)
                .is_some_and(|(_, f)| f.body.is_some() && sigs.get(def).is_some()),
        })
        .collect();
    let is_node: FxHashMap<DefId, ()> = nodes.iter().map(|&def| (def, ())).collect();

    let mut edges: FxHashMap<DefId, Vec<DefId>> = FxHashMap::default();
    for &def in &nodes {
        // A layer written in `with` becomes edges to its handlers (spec 8.5).
        let mut refs: Vec<DefId> = body_refs(program, def)
            .into_iter()
            .flat_map(|(r, _)| match program.item(r) {
                Some(Item::Layer(_)) => layer::flatten(program, r)
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect(),
                _ => vec![r],
            })
            .collect();
        refs.retain(|r| is_node.contains_key(r));
        refs.dedup();
        edges.insert(def, refs);
    }

    strongly_connected(&nodes, &edges)
}

/// The strongly connected components of the graph, each sorted in definition order. A component
/// comes after every component reachable from it.
pub(crate) fn strongly_connected(
    nodes: &[DefId],
    edges: &FxHashMap<DefId, Vec<DefId>>,
) -> Vec<Vec<DefId>> {
    let mut tarjan = Tarjan {
        edges,
        index: FxHashMap::default(),
        low: FxHashMap::default(),
        on_stack: FxHashMap::default(),
        stack: Vec::new(),
        next: 0,
        groups: Vec::new(),
    };
    for &def in nodes {
        if !tarjan.index.contains_key(&def) {
            tarjan.visit(def);
        }
    }
    tarjan.groups
}

/// Tarjan's strongly connected components. A component comes after every component reachable
/// from it (callees first).
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
                let w = self.stack.pop().expect("v is on the stack");
                self.on_stack.insert(w, false);
                group.push(w);
                if w == v {
                    break;
                }
            }
            // Within a group, infer in definition order so diagnostics come in a stable order.
            group.sort_by_key(|def| def.into_raw());
            self.groups.push(group);
        }
    }
}

/// The definitions referred to in a definition's body (fns with bodies, consts, handlers), with
/// their spans, in source order. Empty for definitions without a body. Includes consts in
/// patterns and handlers written in `with`. For handlers, looks at init, release and the
/// operation bodies.
pub fn body_refs(program: &Program, def: DefId) -> Vec<(DefId, Span)> {
    let mut out = Vec::new();
    match program.item(def) {
        Some(Item::Fn(item) | Item::TraitFn(item)) => {
            if let Some(body) = item.body {
                collect_expr_refs(program, body, &mut out);
            }
        }
        Some(Item::Const(item)) => collect_expr_refs(program, item.value, &mut out),
        Some(Item::Handler(item)) => {
            let bodies = item
                .init
                .iter()
                .chain(item.release.iter().map(|(_, body)| body))
                .chain(item.ops.iter().filter_map(|op| op.body.as_ref()));
            for &body in bodies {
                collect_expr_refs(program, body, &mut out);
            }
        }
        _ => {
            if let Some((_, f)) = program.impl_fn(def)
                && let Some(body) = f.body
            {
                collect_expr_refs(program, body, &mut out);
            }
        }
    }
    out
}

fn collect_expr_refs(program: &Program, expr: ExprId, out: &mut Vec<(DefId, Span)>) {
    let go = |e: ExprId, out: &mut Vec<(DefId, Span)>| collect_expr_refs(program, e, out);
    let e = &program.exprs[expr];
    match &e.kind {
        ExprKind::Path(Res::Def(def)) => out.push((*def, e.span)),
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
        ExprKind::With { handlers, body } => {
            for item in handlers {
                for (res, span) in std::iter::once((item.res, item.span)).chain(item.by) {
                    if let Res::Def(h) = res {
                        out.push((h, span));
                    }
                }
            }
            go(*body, out);
        }
        ExprKind::Lambda { body, .. } => go(*body, out),
    }
}

fn collect_block_refs(program: &Program, block: &Block, out: &mut Vec<(DefId, Span)>) {
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

/// Collects the consts referred to in a pattern.
fn collect_pat_refs(program: &Program, pat: PatId, out: &mut Vec<(DefId, Span)>) {
    let p = &program.pats[pat];
    match &p.kind {
        PatKind::Const(Res::Def(def)) => out.push((*def, p.span)),
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
