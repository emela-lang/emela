//! Type checking of a whole program: collects declared types and infers definition bodies in
//! call order.
//!
//! Definitions (fns with bodies, consts and handlers) form a graph with edges to the definitions
//! their bodies refer to. Strongly connected components (groups that refer to each other) are
//! inferred callees first (Tarjan). Definitions without a declared type are generalized and added
//! to the declared types once their group is inferred. Inside a group they are monomorphic, so
//! recursion between definitions without declared types is not polymorphic.

use emela_resolve::hir::{
    Block, DefId, DefKind, DefModule, ExprId, ExprKind, Item, PatId, PatKind, Program, Res, Span,
    Stmt, StrPart, TypeKind,
};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::body::{BodyInfer, HandlerFacts};
use crate::check::program_types::ProgramTypes;
use crate::check::scc::strongly_connected;
use crate::decl::layer;
use crate::decl::lower::TyTable;
use crate::decl::signature::Signatures;
use crate::diagnostic::EffectContext;

mod const_cycle;
pub(crate) mod entry_main;
pub(crate) mod export;
pub(crate) mod program_types;
mod scc;
mod unused_type_params;

/// Names the definitions whose R must be empty (`main` in spec 8.3, test functions in spec
/// 13.3). The driver decides which is the entry's `main` and which are test functions.
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// The entry module. Its exports are checked against the names `main.mjs` already exports
    /// (`main`, `$start`, `[target.js.module]`). `None` when no `main.mjs` is produced, as in a
    /// library-only build, so that there is no launcher name to collide with.
    pub entry: Option<emela_resolve::ModuleId>,
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
    // Every `@export` fn, entry adapters included, is given Async (`[ext.export]`), and an entry
    // adapter also the host effects that the function type of its first parameter uses
    // (`[ext.entry]`).
    let names = |hosts: Vec<DefId>| -> Vec<SmolStr> {
        hosts
            .into_iter()
            .map(|d| program.defs[d].name.clone())
            .collect()
    };
    for (def, item) in program.items.iter() {
        if let Item::Fn(f) = item
            && !f.exports.is_empty()
            && f.body.is_some()
        {
            let name = program.defs[def].name.clone();
            let hosts = names(crate::check::export::adapter_hosts(program, def));
            closed.insert(def, EffectContext::Export { name, hosts });
        }
    }
    // So is a fn made by `@entry`, given what its adapter declares; an error in it becomes E5105
    // (`crate::check::export::check`).
    for e in &program.entry_exports {
        let name = program.defs[e.entry].name.clone();
        let hosts = names(crate::check::export::adapter_hosts(program, e.adapter));
        closed.insert(e.def, EffectContext::Export { name, hosts });
    }
    let ty_table = TyTable::new(program);
    let mut diags = Vec::new();
    let mut sigs = Signatures::collect(program, &ty_table, &mut diags);
    let traits = crate::decl::traits::collect(program, &ty_table, &mut sigs, &mut diags);
    crate::check::const_cycle::check(program, &mut diags);

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
        members: ArenaMap::default(),
        methods: ArenaMap::default(),
        handler_uses: ArenaMap::default(),
        handler_fails: ArenaMap::default(),
        with_handlers: ArenaMap::default(),
        layers: ArenaMap::default(),
        with_args: ArenaMap::default(),
        narrowings: ArenaMap::default(),
        self_caps: Default::default(),
        member_open_withs: ArenaMap::default(),
        bound_item_redirects: Vec::new(),
        layer_args: ArenaMap::default(),
        with_ty_args: ArenaMap::default(),
        layer_ty_args: ArenaMap::default(),
        traits,
        diags,
    };

    let builtins = crate::decl::builtin::Builtins::new(program);
    for group in groups(program, &sigs) {
        let mut infer = BodyInfer::new(
            program,
            &types.ty_table,
            &sigs,
            HandlerFacts {
                uses: &types.handler_uses,
                fails: &types.handler_fails,
                layer_ty_args: &types.layer_ty_args,
            },
            &closed,
            &builtins,
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
        types
            .members
            .extend(body.members.iter().map(|(id, &m)| (id, m)));
        types
            .member_open_withs
            .extend(body.member_open_withs.iter().map(|(id, &w)| (id, w)));
        types.bound_item_redirects.extend(body.bound_item_redirects);
        types
            .methods
            .extend(body.methods.iter().map(|(id, &def)| (id, def)));
        types.handler_uses.extend(body.handler_uses);
        types.handler_fails.extend(body.handler_fails);
        types
            .with_handlers
            .extend(body.with_handlers.iter().map(|(id, hs)| (id, hs.clone())));
        types
            .with_args
            .extend(body.with_args.iter().map(|(id, args)| (id, args.clone())));
        types
            .narrowings
            .extend(body.narrowings.iter().map(|(id, tags)| (id, tags.clone())));
        types.self_caps.extend(body.self_caps.iter().copied());
        types.layer_args.extend(body.layer_args);
        types.with_ty_args.extend(
            body.with_ty_args
                .iter()
                .map(|(id, args)| (id, args.clone())),
        );
        types.layer_ty_args.extend(body.layer_ty_args);
        types.diags.extend(body.diags);
    }

    crate::check::export::check(
        program,
        &types.ty_table,
        &sigs,
        options.entry,
        options.main,
        &mut types.diags,
    );
    crate::decl::traits::unused_derives(program, &types.traits, &mut types.diags);
    crate::check::entry_main::check(
        program,
        &types.ty_table,
        &sigs,
        options.main,
        &mut types.diags,
    );
    crate::check::unused_type_params::check(program, &sigs, &mut types.diags);
    types.layers = layer::check_layers(
        program,
        &sigs,
        &types.ty_table,
        &types.handler_uses,
        &types.layer_ty_args,
        &mut types.diags,
    );
    types.sigs = sigs;
    types
}

/// Orders the definitions to infer (fns with bodies, consts, handlers, layers, trait default
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
            // The arguments of a layer's items are inferred, as the value of a const. Every
            // layer is, since an item without arguments may need them (E4104).
            Some(Item::Layer(_)) => true,
            // An impl function without a signature (its impl or trait function has an error) is
            // not inferred.
            _ => program
                .impl_fn(def)
                .is_some_and(|(_, f)| f.body.is_some() && sigs.get(def).is_some()),
        })
        .collect();
    let is_node: FxHashMap<DefId, ()> = nodes.iter().map(|&def| (def, ())).collect();

    // A method call `x.f(...)` calls a fn found from the type of x (`[type.rules]`), which is
    // known only during inference. A fn without a declared type has to be inferred before its
    // callers, so a method call gets an edge to every such fn named f that can be called as a
    // method (its first parameter is written as a named type of its own module). This can put
    // definitions that do not call each other into one group, inferred together and not
    // generalized between them: the cost of not knowing the callee before inference.
    let mut methods: FxHashMap<&SmolStr, Vec<DefId>> = FxHashMap::default();
    for &def in &nodes {
        if sigs.get(def).is_none() && may_be_method(program, def) {
            methods
                .entry(&program.defs[def].name)
                .or_default()
                .push(def);
        }
    }

    let mut edges: FxHashMap<DefId, Vec<DefId>> = FxHashMap::default();
    for &def in &nodes {
        let body = body_refs_and_methods(program, def);
        // A layer written in `with` becomes edges to its handlers (spec 8.5).
        let mut refs: Vec<DefId> = body
            .defs
            .into_iter()
            .flat_map(|(r, _)| match program.item(r) {
                Some(Item::Layer(_)) => layer::flatten(program, r)
                    .into_iter()
                    .map(|f| f.handler)
                    .collect(),
                _ => vec![r],
            })
            .collect();
        refs.extend(
            body.methods
                .iter()
                .flat_map(|name| methods.get(name).into_iter().flatten()),
        );
        refs.retain(|r| is_node.contains_key(r));
        refs.dedup();
        edges.insert(def, refs);
    }

    strongly_connected(&nodes, &edges)
}

/// A top-level fn whose first parameter is written as a named type (not a type parameter)
/// defined by the fn's own module, so that a method call can find it. A method call looks only
/// in the module that defines the type (`[type.rules]`): `fn get(url: String)` outside the core
/// module String is never a method.
fn may_be_method(program: &Program, def: DefId) -> bool {
    let Some(Item::Fn(item)) = program.item(def) else {
        return false;
    };
    let Some(ty) = item.sig.params.first().and_then(|p| p.ty) else {
        return false;
    };
    let TypeKind::Path {
        res: Res::Def(d), ..
    } = program.types[ty].kind
    else {
        return false;
    };
    program.defs[d].kind != DefKind::TypeParam
        && matches!(program.defs[def].module, DefModule::Source(m)
            if crate::decl::methods::defining_module(program, d) == Some(m))
}

/// The definitions referred to in a definition's body (fns with bodies, consts, handlers), with
/// their spans, in source order. Empty for definitions without a body. Includes consts in
/// patterns and handlers written in `with`. For handlers, looks at init, release and the
/// operation bodies; for layers, at the arguments of their items.
pub fn body_refs(program: &Program, def: DefId) -> Vec<(DefId, Span)> {
    body_refs_and_methods(program, def).defs
}

/// What a body refers to: the definitions of [`body_refs`], and the names of the method calls
/// (`x.f(...)`), whose callees are known only after type inference.
#[derive(Default)]
struct Refs {
    defs: Vec<(DefId, Span)>,
    methods: Vec<SmolStr>,
}

fn body_refs_and_methods(program: &Program, def: DefId) -> Refs {
    let mut out = Refs::default();
    match program.item(def) {
        Some(Item::Fn(item) | Item::TraitFn(item)) => {
            if let Some(body) = item.body {
                collect_expr_refs(program, body, &mut out);
            }
        }
        Some(Item::Const(item)) => collect_expr_refs(program, item.value, &mut out),
        Some(Item::Layer(item)) => {
            for arg in item
                .members
                .iter()
                .flat_map(|m| m.args.iter().flat_map(|a| &a.args))
            {
                collect_expr_refs(program, arg.value, &mut out);
            }
        }
        Some(Item::Handler(item)) => {
            let bodies = item
                .init
                .iter()
                .chain(item.release.iter().map(|(_, body)| body))
                .chain(item.ops.iter().filter_map(|op| op.body.as_ref()));
            for &body in bodies {
                collect_expr_refs(program, body, &mut out);
            }
            // The requirements of inner handlers are the outer handler's (`[effect.handler]`).
            if let Some(span) = program.defs[def].span {
                out.defs
                    .extend(item.inner.iter().map(|&inner| (inner, span)));
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

fn collect_expr_refs(program: &Program, expr: ExprId, out: &mut Refs) {
    let go = |e: ExprId, out: &mut Refs| collect_expr_refs(program, e, out);
    let e = &program.exprs[expr];
    match &e.kind {
        ExprKind::Path(Res::Def(def)) => out.defs.push((*def, e.span)),
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
            if let ExprKind::Field { name, .. } = &program.exprs[*callee].kind {
                out.methods.push(name.name.clone());
            }
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
                        out.defs.push((h, span));
                    }
                }
                for arg in layer::item_args(item) {
                    go(arg, out);
                }
            }
            go(*body, out);
        }
        ExprKind::Lambda { body, .. } => go(*body, out),
    }
}

fn collect_block_refs(program: &Program, block: &Block, out: &mut Refs) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { pat, value, .. } => {
                collect_pat_refs(program, *pat, out);
                collect_expr_refs(program, *value, out);
            }
            Stmt::LocalFn(f) => collect_expr_refs(program, f.body, out),
            Stmt::Expr(e) => collect_expr_refs(program, *e, out),
        }
    }
    if let Some(tail) = block.tail {
        collect_expr_refs(program, tail, out);
    }
}

/// Collects the consts referred to in a pattern.
fn collect_pat_refs(program: &Program, pat: PatId, out: &mut Refs) {
    let p = &program.pats[pat];
    match &p.kind {
        PatKind::Const(Res::Def(def)) => out.defs.push((*def, p.span)),
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
