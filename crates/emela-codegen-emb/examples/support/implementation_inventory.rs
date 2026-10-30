//! A bounded syntax audit. Evidence is never promoted to a backend support claim.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use quote::ToTokens;
use serde::{Deserialize, Serialize};
use syn::{
    Expr, Item, Pat,
    parse::Parse,
    spanned::Spanned,
    visit::{self, Visit},
};

use super::definitions::{
    self, Inventory, Location, SourceFile, cell, code, relative_path, sha256,
};

const COMMAND: &str = "crates/emela-codegen-emb/tools/flow-implementation-inventory.sh";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub targets: Vec<Target>,
    pub routes: Vec<Route>,
    pub exclusions: Vec<Exclusion>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub owner: String,
    #[serde(default)]
    pub representation_owner: Option<String>,
    pub contexts: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Selector {
    Match,
    If,
    LetElse,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Emit,
    Lower,
    Reject,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub id: String,
    pub path: String,
    pub symbol: String,
    pub owner: String,
    #[serde(default)]
    pub representation_owner: Option<String>,
    pub selector: Selector,
    pub expression: String,
    /// Disambiguates e.g. `let expression = match op` from an arity/type-checking table.
    #[serde(default)]
    pub binding: Option<String>,
    #[serde(default)]
    pub top_level: bool,
    pub contexts: Vec<String>,
    pub role: Role,
    pub note: String,
    #[serde(default)]
    pub only_variants: Vec<String>,
    #[serde(default)]
    pub required_calls: Vec<String>,
}

impl Route {
    fn pattern_owner(&self) -> String {
        self.representation_owner.as_ref().map_or_else(
            || self.owner.clone(),
            |owner| format!("{}=>{owner}", self.owner),
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub id: String,
    pub route: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct Evidence {
    pub route: String,
    pub role: String,
    pub contexts: Vec<String>,
    pub source: Location,
    pub symbol: String,
    pub pattern: String,
    pub condition: Option<String>,
    pub calls: Vec<String>,
    pub body_sha256: String,
    pub error_only: bool,
}

#[derive(Debug, Serialize)]
pub struct ImplementationEntry {
    pub id: String,
    pub kind: &'static str,
    pub parent: String,
    /// The current declaration, not a hash approved by a reviewer.
    pub observed_definition_sha256: Option<String>,
    pub review_status: &'static str,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Serialize)]
pub struct Fallback {
    pub route: String,
    pub pattern: String,
    pub source: Location,
    pub body_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct ImplementationInventory {
    pub schema_version: u32,
    pub inventory_kind: &'static str,
    pub backend: &'static str,
    pub config: SourceFile,
    pub sources: Vec<SourceFile>,
    pub routes: Vec<Route>,
    pub items: Vec<ImplementationEntry>,
    pub fallbacks: Vec<Fallback>,
}

#[derive(Debug, Serialize)]
pub struct ContextResult {
    pub context: String,
    pub status: &'static str,
    pub routes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CoverageEntry {
    pub id: String,
    pub definition_sha256: String,
    pub flow_input: &'static str,
    pub exclusion_reason: Option<String>,
    pub review_status: &'static str,
    pub contexts: Vec<ContextResult>,
}

#[derive(Debug, Serialize)]
pub struct RepresentationGap {
    pub owner: String,
    pub representation_owner: String,
    pub missing_variants: Vec<String>,
    pub extra_variants: Vec<String>,
    pub missing_adapter: bool,
}

#[derive(Debug, Serialize)]
pub struct RouteError {
    pub route: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Coverage {
    pub schema_version: u32,
    pub inventory_kind: &'static str,
    pub audit_level: &'static str,
    pub targets: Vec<Target>,
    pub summary: BTreeMap<String, usize>,
    pub items: Vec<CoverageEntry>,
    pub orphan_evidence: Vec<String>,
    pub unconfigured_enums: Vec<String>,
    pub out_of_scope: Vec<String>,
    pub representation_gaps: Vec<RepresentationGap>,
    pub route_errors: Vec<RouteError>,
}

fn tokens(value: impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

fn expr_key(value: &Expr) -> String {
    match value {
        Expr::Paren(e) => expr_key(&e.expr),
        _ => tokens(value),
    }
}

fn location(path: &str, span: proc_macro2::Span) -> Location {
    Location {
        path: path.into(),
        line: span.start().line,
        end_line: span.end().line,
    }
}

fn variant_id(path: &syn::Path, owner: &str) -> Option<String> {
    let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    let (owner, representation) = owner.split_once("=>").unwrap_or((owner, owner));
    let short = representation.rsplit("::").next()?;
    if segments.len() == 2 && segments[0] == short
        || segments.len() > 2 && segments[..segments.len() - 1].join("::") == representation
    {
        Some(format!("{owner}::{}", segments.last()?))
    } else {
        None
    }
}

struct Patterns<'a> {
    owner: &'a str,
    ids: BTreeSet<String>,
}
impl<'ast> Visit<'ast> for Patterns<'_> {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if let Some(id) = variant_id(path, self.owner) {
            self.ids.insert(id);
        }
        visit::visit_path(self, path);
    }
}
fn pattern_ids(pattern: &Pat, owner: &str) -> BTreeSet<String> {
    let mut visitor = Patterns {
        owner,
        ids: BTreeSet::new(),
    };
    visitor.visit_pat(pattern);
    visitor.ids
}

#[derive(Default)]
struct Calls(BTreeSet<String>);
impl<'ast> Visit<'ast> for Calls {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        self.0.insert(tokens(&call.func));
        visit::visit_expr_call(self, call);
    }
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.0.insert(format!(".{}", call.method));
        visit::visit_expr_method_call(self, call);
    }
}

fn error_only(expr: &Expr) -> bool {
    match expr {
        Expr::Return(e) => e.expr.as_deref().is_some_and(error_only),
        Expr::Block(e) => e.block.stmts.last().is_some_and(|s| match s {
            syn::Stmt::Expr(e, _) => error_only(e),
            _ => false,
        }),
        Expr::Call(e) => matches!(&*e.func, Expr::Path(p) if p.path.is_ident("Err")),
        _ => false,
    }
}

struct MatchMacro {
    expression: Expr,
    pattern: Pat,
    guard: Option<Expr>,
}
impl Parse for MatchMacro {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let expression = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let pattern = Pat::parse_multi_with_leading_vert(input)?;
        let guard = if input.peek(syn::Token![if]) {
            input.parse::<syn::Token![if]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        if !input.is_empty() {
            input.parse::<syn::Token![,]>()?;
        }
        Ok(Self {
            expression,
            pattern,
            guard,
        })
    }
}

fn condition_ids(expr: &Expr, expression: &str, owner: &str) -> BTreeSet<String> {
    match expr {
        Expr::Let(e) if expr_key(&e.expr) == expression => pattern_ids(&e.pat, owner),
        Expr::Paren(e) => condition_ids(&e.expr, expression, owner),
        Expr::Binary(e) if matches!(e.op, syn::BinOp::And(_)) => {
            condition_ids(&e.left, expression, owner)
                .union(&condition_ids(&e.right, expression, owner))
                .cloned()
                .collect()
        }
        Expr::Binary(e) if matches!(e.op, syn::BinOp::Eq(_)) => {
            let other = if expr_key(&e.left) == expression {
                &e.right
            } else if expr_key(&e.right) == expression {
                &e.left
            } else {
                return BTreeSet::new();
            };
            if let Expr::Path(p) = &**other {
                variant_id(&p.path, owner).into_iter().collect()
            } else {
                BTreeSet::new()
            }
        }
        Expr::Macro(e) if e.mac.path.is_ident("matches") => {
            syn::parse2::<MatchMacro>(e.mac.tokens.clone())
                .ok()
                .filter(|m| expr_key(&m.expression) == expression)
                .map_or_else(BTreeSet::new, |m| {
                    // Guard is preserved by the whole if condition in Evidence.
                    let _ = m.guard;
                    pattern_ids(&m.pattern, owner)
                })
        }
        _ => BTreeSet::new(),
    }
}

fn catch_all(pattern: &Pat) -> bool {
    match pattern {
        Pat::Wild(_) => true,
        Pat::Ident(p) => p.subpat.is_none(),
        Pat::Paren(p) => catch_all(&p.pat),
        _ => false,
    }
}

#[derive(Default)]
pub struct Extracted {
    pub items: BTreeMap<String, Vec<Evidence>>,
    pub fallbacks: Vec<Fallback>,
    pub sites: usize,
}

struct Extractor<'a> {
    route: &'a Route,
    expression: String,
    binding: Option<String>,
    match_depth: usize,
    whole_function: &'a syn::Block,
    result: Extracted,
}

impl Extractor<'_> {
    fn add(
        &mut self,
        ids: BTreeSet<String>,
        pattern: String,
        condition: Option<String>,
        body: &Expr,
        span: proc_macro2::Span,
    ) {
        let mut calls = Calls::default();
        calls.visit_expr(body);
        if !self
            .route
            .required_calls
            .iter()
            .all(|required| calls.0.contains(required))
        {
            return;
        }
        for id in ids {
            let variant = id.rsplit("::").next().unwrap();
            if !self.route.only_variants.is_empty()
                && !self.route.only_variants.iter().any(|v| v == variant)
            {
                continue;
            }
            let evidence = Evidence {
                route: self.route.id.clone(),
                role: match self.route.role {
                    Role::Emit => "emit_candidate",
                    Role::Lower => "lower_candidate",
                    Role::Reject => "reject_candidate",
                }
                .into(),
                contexts: self.route.contexts.clone(),
                source: location(&self.route.path, span),
                symbol: self.route.symbol.clone(),
                pattern: pattern.clone(),
                condition: condition.clone(),
                calls: calls.0.iter().cloned().collect(),
                body_sha256: sha256(tokens(body).as_bytes()),
                error_only: error_only(body),
            };
            self.result.items.entry(id).or_default().push(evidence);
        }
    }
}

impl<'ast> Visit<'ast> for Extractor<'_> {
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if matches!(self.route.selector, Selector::LetElse)
            && let Some(init) = &local.init
            && init.diverge.is_some()
            && expr_key(&init.expr) == self.expression
        {
            let ids = pattern_ids(&local.pat, &self.route.pattern_owner());
            if !ids.is_empty() {
                self.result.sites += 1;
                let body = Expr::Block(syn::ExprBlock {
                    attrs: vec![],
                    label: None,
                    block: self.whole_function.clone(),
                });
                self.add(ids, tokens(&local.pat), Some("let-else; subsequent function body is evidence, not a proven transformation".into()), &body, local.span());
            }
        }
        let previous = self.binding.replace(tokens(&local.pat));
        visit::visit_local(self, local);
        self.binding = previous;
    }

    fn visit_expr_match(&mut self, value: &'ast syn::ExprMatch) {
        if matches!(self.route.selector, Selector::Match)
            && expr_key(&value.expr) == self.expression
            && (!self.route.top_level || self.match_depth == 0)
            && self
                .route
                .binding
                .as_ref()
                .is_none_or(|b| self.binding.as_deref() == Some(b.as_str()))
        {
            self.result.sites += 1;
            for arm in &value.arms {
                let ids = pattern_ids(&arm.pat, &self.route.pattern_owner());
                if ids.is_empty() && catch_all(&arm.pat) {
                    self.result.fallbacks.push(Fallback {
                        route: self.route.id.clone(),
                        pattern: tokens(&arm.pat),
                        source: location(&self.route.path, arm.span()),
                        body_sha256: sha256(tokens(&arm.body).as_bytes()),
                    });
                }
                self.add(
                    ids,
                    tokens(&arm.pat),
                    arm.guard.as_ref().map(|(_, e)| tokens(e)),
                    &arm.body,
                    arm.span(),
                );
            }
        }
        self.match_depth += 1;
        visit::visit_expr_match(self, value);
        self.match_depth -= 1;
    }

    fn visit_expr_if(&mut self, value: &'ast syn::ExprIf) {
        if matches!(self.route.selector, Selector::If) {
            let ids = condition_ids(&value.cond, &self.expression, &self.route.pattern_owner());
            if !ids.is_empty() {
                self.result.sites += 1;
                let body = Expr::Block(syn::ExprBlock {
                    attrs: vec![],
                    label: None,
                    block: value.then_branch.clone(),
                });
                self.add(
                    ids,
                    tokens(&value.cond),
                    Some(tokens(&value.cond)),
                    &body,
                    value.span(),
                );
            }
        }
        visit::visit_expr_if(self, value);
    }
}

fn function_blocks<'a>(items: &'a [Item], symbol: &str, out: &mut Vec<&'a syn::Block>) {
    for item in items {
        match item {
            Item::Fn(f) if f.sig.ident == symbol => out.push(&f.block),
            Item::Impl(i) => {
                if let syn::Type::Path(ty) = &*i.self_ty {
                    let owner = ty.path.segments.last().unwrap().ident.to_string();
                    for item in &i.items {
                        if let syn::ImplItem::Fn(f) = item
                            && format!("{owner}::{}", f.sig.ident) == symbol
                        {
                            out.push(&f.block);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn extract(route: &Route, source: &str) -> Result<Extracted, String> {
    let parsed = syn::parse_file(source).map_err(|e| format!("{}: {e}", route.path))?;
    let mut functions = vec![];
    function_blocks(&parsed.items, &route.symbol, &mut functions);
    if functions.len() != 1 {
        return Err(format!(
            "route {}: expected one function {}, found {}",
            route.id,
            route.symbol,
            functions.len()
        ));
    }
    let expression =
        expr_key(&syn::parse_str::<Expr>(&route.expression).map_err(|e| e.to_string())?);
    let mut extractor = Extractor {
        route,
        expression,
        binding: None,
        match_depth: 0,
        whole_function: functions[0],
        result: Extracted::default(),
    };
    extractor.visit_block(functions[0]);
    if extractor.result.sites == 0
        || matches!(route.selector, Selector::Match) && extractor.result.sites != 1
    {
        return Err(format!(
            "route {}: selector matched {} sites; refine expression/binding",
            route.id, extractor.result.sites
        ));
    }
    if extractor.result.items.is_empty() {
        return Err(format!(
            "route {} has no explicit variant evidence",
            route.id
        ));
    }
    Ok(extractor.result)
}

pub fn collect(
    root: &Path,
    config_path: &Path,
    definitions: &Inventory,
) -> Result<(ImplementationInventory, Coverage), String> {
    let config_source = fs::read_to_string(root.join(config_path)).map_err(|e| e.to_string())?;
    let config: Config = serde_json::from_str(&config_source).map_err(|e| e.to_string())?;
    if config.schema_version != 1 {
        return Err("unsupported configuration schema".into());
    }
    let owners: BTreeSet<_> = config.targets.iter().map(|t| t.owner.as_str()).collect();
    if config.targets.is_empty()
        || config.routes.is_empty()
        || owners.len() != config.targets.len()
        || config.targets.iter().any(|t| {
            t.contexts.is_empty()
                || t.contexts.iter().collect::<BTreeSet<_>>().len() != t.contexts.len()
        })
    {
        return Err("empty/duplicate targets, routes or contexts".into());
    }
    let primary_files: BTreeSet<_> = definitions
        .scope
        .iter()
        .filter(|scope| scope.include.is_none())
        .map(|scope| scope.path.as_str())
        .collect();
    let unconfigured_enums = definitions
        .items
        .iter()
        .filter(|i| {
            i.kind == "enum"
                && primary_files.contains(i.source.path.as_str())
                && !owners.contains(i.id.as_str())
        })
        .map(|i| i.id.clone())
        .collect();
    for owner in &owners {
        if !definitions
            .items
            .iter()
            .any(|i| i.id == *owner && i.kind == "enum")
        {
            return Err(format!("unknown target enum {owner}"));
        }
    }
    let mut items = BTreeMap::<String, Vec<Evidence>>::new();
    let mut fallbacks = Vec::new();
    let mut sources = BTreeMap::<String, String>::new();
    let mut representation_gaps = Vec::new();
    let mut route_errors = Vec::new();
    if config
        .targets
        .iter()
        .any(|target| target.representation_owner.is_some())
    {
        let normalized_path = "crates/emela-codegen-emb/src/contract/ir.rs";
        let normalized_source =
            fs::read_to_string(root.join(normalized_path)).map_err(|e| e.to_string())?;
        let normalized = syn::parse_file(&normalized_source).map_err(|e| e.to_string())?;
        sources.insert(normalized_path.into(), sha256(normalized_source.as_bytes()));
        for target in &config.targets {
            if let Some(owner) = &target.representation_owner {
                let name = owner.rsplit("::").next().unwrap();
                let variants = normalized
                    .items
                    .iter()
                    .find_map(|item| match item {
                        Item::Enum(item) if item.ident == name => Some(
                            item.variants
                                .iter()
                                .map(|variant| variant.ident.to_string())
                                .collect::<BTreeSet<_>>(),
                        ),
                        _ => None,
                    })
                    .unwrap_or_default();
                let upstream: BTreeSet<_> = definitions
                    .items
                    .iter()
                    .filter(|item| {
                        item.kind == "variant"
                            && item.parent.as_deref() == Some(target.owner.as_str())
                    })
                    .map(|item| item.id.rsplit("::").next().unwrap().to_owned())
                    .collect();
                let missing_adapter = !config.routes.iter().any(|route| {
                    route.owner == target.owner
                        && route.path.ends_with("lower/adapter.rs")
                        && route.representation_owner.is_none()
                });
                if variants != upstream || missing_adapter {
                    representation_gaps.push(RepresentationGap {
                        owner: target.owner.clone(),
                        representation_owner: owner.clone(),
                        missing_variants: upstream.difference(&variants).cloned().collect(),
                        extra_variants: variants.difference(&upstream).cloned().collect(),
                        missing_adapter,
                    });
                }
            }
        }
    }
    let mut route_ids = BTreeSet::new();
    for route in &config.routes {
        if !route_ids.insert(route.id.clone())
            || !owners.contains(route.owner.as_str())
            || route.contexts.is_empty()
        {
            return Err(format!("invalid/duplicate route {}", route.id));
        }
        if route.representation_owner.is_some()
            && !config.targets.iter().any(|target| {
                target.owner == route.owner
                    && target.representation_owner == route.representation_owner
            })
        {
            return Err(format!("undeclared representation mapping {}", route.id));
        }
        let extracted = (|| {
            let source = fs::read_to_string(root.join(&route.path))
                .map_err(|e| format!("{}: {e}", route.path))?;
            sources.insert(route.path.clone(), sha256(source.as_bytes()));
            extract(route, &source)
        })();
        let extracted = match extracted {
            Ok(extracted) => extracted,
            Err(message) => {
                route_errors.push(RouteError {
                    route: route.id.clone(),
                    path: route.path.clone(),
                    message,
                });
                continue;
            }
        };
        for (id, evidence) in extracted.items {
            items.entry(id).or_default().extend(evidence);
        }
        fallbacks.extend(extracted.fallbacks);
    }
    let mut exclusions = BTreeMap::new();
    for exclusion in &config.exclusions {
        if !route_ids.contains(&exclusion.route)
            || exclusions
                .insert(exclusion.id.as_str(), exclusion)
                .is_some()
            || !definitions.items.iter().any(|i| {
                i.id == exclusion.id
                    && i.kind == "variant"
                    && i.parent.as_deref().is_some_and(|p| owners.contains(p))
            })
        {
            return Err(format!("invalid exclusion {}", exclusion.id));
        }
    }
    let mut implementation_items = Vec::new();
    for (id, evidence) in items {
        let definition = definitions.items.iter().find(|i| i.id == id);
        implementation_items.push(ImplementationEntry {
            parent: id.rsplit_once("::").unwrap().0.into(),
            id,
            kind: "variant",
            observed_definition_sha256: definition.map(|i| i.definition_sha256.clone()),
            review_status: "unreviewed",
            evidence,
        });
    }
    let implementation = ImplementationInventory {
        schema_version: 1,
        inventory_kind: "implementation_evidence",
        backend: "emela-codegen-emb",
        config: SourceFile {
            path: config_path.to_string_lossy().replace('\\', "/"),
            sha256: sha256(config_source.as_bytes()),
        },
        sources: sources
            .into_iter()
            .map(|(path, sha256)| SourceFile { path, sha256 })
            .collect(),
        routes: config.routes,
        items: implementation_items,
        fallbacks,
    };
    let mut coverage = Coverage {
        schema_version: 2,
        inventory_kind: "coverage_comparison",
        audit_level: "explicit_variant_syntax_only",
        targets: config.targets,
        summary: BTreeMap::new(),
        items: Vec::new(),
        orphan_evidence: implementation
            .items
            .iter()
            .filter(|i| i.observed_definition_sha256.is_none())
            .map(|i| i.id.clone())
            .collect(),
        unconfigured_enums,
        out_of_scope: Vec::new(),
        representation_gaps,
        route_errors,
    };
    for definition in &definitions.items {
        let target = coverage.targets.iter().find(|t| {
            definition.kind == "variant" && definition.parent.as_deref() == Some(&t.owner)
        });
        let Some(target) = target else {
            coverage.out_of_scope.push(definition.id.clone());
            continue;
        };
        let found = implementation.items.iter().find(|i| i.id == definition.id);
        let exclusion = exclusions.get(definition.id.as_str());
        let required = if exclusion.is_some() {
            vec!["input".into()]
        } else {
            target.contexts.clone()
        };
        let mut contexts = Vec::new();
        for context in required {
            let evidence: Vec<_> = found
                .into_iter()
                .flat_map(|i| &i.evidence)
                .filter(|e| {
                    if coverage.representation_gaps.iter().any(|gap| {
                        gap.owner == target.owner
                            && gap.missing_variants.iter().any(|name| {
                                definition.id.rsplit("::").next() == Some(name.as_str())
                            })
                    }) {
                        return false;
                    }
                    if let Some(exclusion) = exclusion {
                        e.route == exclusion.route && e.role == "reject_candidate"
                    } else {
                        e.contexts.contains(&context)
                            && e.role != "reject_candidate"
                            && !e.error_only
                    }
                })
                .collect();
            let status = if evidence.is_empty() {
                "missing_evidence"
            } else if exclusion.is_some() {
                "rejection_evidence"
            } else {
                "candidate_found"
            };
            *coverage.summary.entry(status.into()).or_default() += 1;
            contexts.push(ContextResult {
                context,
                status,
                routes: evidence
                    .iter()
                    .map(|e| e.route.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            });
        }
        coverage.items.push(CoverageEntry {
            id: definition.id.clone(),
            definition_sha256: definition.definition_sha256.clone(),
            flow_input: if exclusion.is_some() {
                "forbidden_by_config"
            } else {
                "unreviewed"
            },
            exclusion_reason: exclusion.map(|e| e.reason.clone()),
            review_status: "unreviewed",
            contexts,
        });
    }
    Ok((implementation, coverage))
}

pub fn require_evidence(coverage: &Coverage) -> Result<(), String> {
    let missing: Vec<_> = coverage
        .items
        .iter()
        .flat_map(|i| {
            i.contexts
                .iter()
                .filter(|c| c.status == "missing_evidence")
                .map(|c| format!("{} [{}]", i.id, c.context))
        })
        .chain(
            coverage
                .orphan_evidence
                .iter()
                .map(|id| format!("orphan: {id}")),
        )
        .chain(
            coverage
                .unconfigured_enums
                .iter()
                .map(|id| format!("unconfigured enum: {id}")),
        )
        .chain(coverage.representation_gaps.iter().map(|gap| {
            format!(
                "representation {}: missing={:?}, extra={:?}, missing_adapter={}",
                gap.owner, gap.missing_variants, gap.extra_variants, gap.missing_adapter
            )
        }))
        .chain(
            coverage
                .route_errors
                .iter()
                .map(|error| format!("route {} ({}): {}", error.route, error.path, error.message)),
        )
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "explicit evidence audit has {} gaps:\n{}",
            missing.len(),
            missing.join("\n")
        ))
    }
}

pub fn reports(
    implementation: &ImplementationInventory,
    coverage: &Coverage,
    root: &Path,
    out_dir: &Path,
) -> Result<Vec<(&'static str, String)>, String> {
    let mut md = format!(
        "# codegen-embの実装箇所の自動抽出\n\n<!-- Generated by {COMMAND}; schema_version=1. Do not edit. -->\n\n設定で指定した関数・分岐から得た構文上の候補。役割と実行経路は設定の宣言であり、実際の到達性・処理の完全性は未検証。全レコードはunreviewed。\n\n[JSON](flow-implementation-inventory.json) / [定義との比較](flow-coverage.md)\n\n"
    );
    md.push_str("## 抽出設定と定義元\n\n| source | SHA-256 |\n| --- | --- |\n");
    for source in std::iter::once(&implementation.config).chain(&implementation.sources) {
        let link = relative_path(out_dir, &root.join(&source.path));
        md.push_str(&format!(
            "| [{}]({link}) | {} |\n",
            source.path,
            code(&source.sha256)
        ));
    }
    md.push_str("\n## 実装候補\n\n| ID | 経路 | 役割（設定） | 関数・位置 | パターン・条件 | 呼出し先 | 直接のerror戻り値 |\n| --- | --- | --- | --- | --- | --- | --- |\n");
    for item in &implementation.items {
        for e in &item.evidence {
            let link = relative_path(out_dir, &root.join(&e.source.path));
            md.push_str(&format!(
                "| {} | {} | {} | [{} L{}]({link}#L{}) | {} {} | {} | {} |\n",
                code(&item.id),
                cell(&e.contexts.join(", ")),
                e.role,
                cell(&e.symbol),
                e.source.line,
                e.source.line,
                code(&e.pattern),
                e.condition.as_deref().map(code).unwrap_or_default(),
                cell(&e.calls.join(", ")),
                e.error_only
            ));
        }
    }
    md.push_str("\n## 自動で対応に数えない既定分岐\n\n`_`や識別子だけの分岐は、個別variantの対応として展開しない。残ったvariantがこの分岐へ到達するという制御フローの証明も行わない。\n\n| route | パターン | 位置 |\n| --- | --- | --- |\n");
    for e in &implementation.fallbacks {
        let link = relative_path(out_dir, &root.join(&e.source.path));
        md.push_str(&format!(
            "| {} | {} | [L{}]({link}#L{}) |\n",
            code(&e.route),
            code(&e.pattern),
            e.source.line,
            e.source.line
        ));
    }
    let mut compared = format!(
        "# Flow IR定義と実装候補の比較\n\n<!-- Generated by {COMMAND}; schema_version=1. Do not edit. -->\n\n**監査単位は指定したenumのvariantに対する明示的な構文上の根拠。実装対応の完了判定ではない。** candidate_foundは指定された経路の候補が見つかった状態。missing_evidenceは設定・抽出方式の不足も含み、未実装と同義ではない。全件unreviewed。\n\n[実装候補](flow-implementation-inventory.md) / [JSON](flow-coverage.json) / [定義一覧](flow-ir-inventory.md)\n\n"
    );
    compared.push_str(&format!("対象variant {}件。対象外 {}レコード（field・型宣言・指定外のCore型など）。orphan evidence {}件。監査設定のないLIR enum {}件。\n\n", coverage.items.len(), coverage.out_of_scope.len(), coverage.orphan_evidence.len(), coverage.unconfigured_enums.len()));
    compared.push_str("| 状態 | variant × 必須経路の件数 |\n| --- | --- |\n");
    for (status, count) in &coverage.summary {
        compared.push_str(&format!("| {status} | {count} |\n"));
    }
    compared.push_str("\n## 上流IRへの未追従\n\n");
    if coverage.representation_gaps.is_empty() && coverage.route_errors.is_empty() {
        compared.push_str("なし。\n");
    }
    for gap in &coverage.representation_gaps {
        compared.push_str(&format!(
            "- {} → {}: 不足 {}、定義外 {}、adapter登録欠落 {}\n",
            code(&gap.owner),
            code(&gap.representation_owner),
            code(&gap.missing_variants.join(", ")),
            code(&gap.extra_variants.join(", ")),
            gap.missing_adapter
        ));
    }
    for error in &coverage.route_errors {
        compared.push_str(&format!(
            "- route {}: {}\n",
            code(&error.route),
            cell(&error.message)
        ));
    }
    compared.push_str("\n## 根拠が見つからない項目\n\n");
    for id in &coverage.unconfigured_enums {
        compared.push_str(&format!("- 監査設定のないenum: {}\n", code(id)));
    }
    let missing: Vec<_> = coverage
        .items
        .iter()
        .flat_map(|i| {
            i.contexts
                .iter()
                .filter(|c| c.status == "missing_evidence")
                .map(|c| format!("- {} / {}\n", code(&i.id), c.context))
        })
        .chain(coverage.representation_gaps.iter().map(|gap| {
            format!(
                "representation {}: missing={:?}, extra={:?}, missing_adapter={}",
                gap.owner, gap.missing_variants, gap.extra_variants, gap.missing_adapter
            )
        }))
        .chain(
            coverage
                .route_errors
                .iter()
                .map(|error| format!("route {} ({}): {}", error.route, error.path, error.message)),
        )
        .collect();
    if missing.is_empty() {
        compared.push_str(
            "なし。ただし構文上の候補の有無だけの集計であり、対応や動作の保証ではない。\n",
        );
    } else {
        for line in missing {
            compared.push_str(&line);
        }
    }
    compared.push_str("\n## 比較表\n\n| ID | Flow入力（設定） | 必須経路 | 抽出結果 | route | 注記 |\n| --- | --- | --- | --- | --- | --- |\n");
    for item in &coverage.items {
        for context in &item.contexts {
            compared.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                code(&item.id),
                item.flow_input,
                context.context,
                context.status,
                cell(&context.routes.join(", ")),
                cell(
                    item.exclusion_reason
                        .as_deref()
                        .unwrap_or("未review。型・フィールド条件・到達性は別監査。")
                )
            ));
        }
    }
    compared.push_str("\n## 設定した監査範囲\n\n| enum | 必須経路 |\n| --- | --- |\n");
    for target in &coverage.targets {
        compared.push_str(&format!(
            "| {} | {} |\n",
            code(&target.owner),
            cell(&target.contexts.join(", "))
        ));
    }
    compared.push_str("\n入力禁止は設定に明示したIDと拒否routeの構文上の根拠を照合する。Flow許可の自動判定、変換先までの経路追跡、cfg評価、macro展開、関数型の解決、試験実行、レビュー済みhashの自動更新は行わない。\n");
    Ok(vec![
        (
            "flow-implementation-inventory.json",
            serde_json::to_string_pretty(implementation).map_err(|e| e.to_string())? + "\n",
        ),
        ("flow-implementation-inventory.md", md),
        (
            "flow-coverage.json",
            serde_json::to_string_pretty(coverage).map_err(|e| e.to_string())? + "\n",
        ),
        ("flow-coverage.md", compared),
    ])
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let root = definitions::repository_root();
    let mut out_dir = root.join("crates/emela-codegen-emb/docs/generated");
    let mut config_path =
        root.join("crates/emela-codegen-emb/tools/flow-implementation-routes.json");
    let (mut check, mut require) = (false, false);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--require-evidence" => require = true,
            "--out-dir" => out_dir = args.next().ok_or("--out-dir requires a path")?.into(),
            "--config" => config_path = args.next().ok_or("--config requires a path")?.into(),
            "--help" | "-h" => {
                println!(
                    "Usage: {COMMAND} [--check] [--require-evidence] [--out-dir PATH] [--config PATH]\n--check checks generated contents without writing.\n--require-evidence fails on missing/orphan syntactic evidence; this does not verify backend support.\nRelative paths are resolved from the current working directory."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let out_dir = definitions::absolute(&out_dir)?;
    let config_path = definitions::absolute(&config_path)?;
    let config_path = config_path
        .strip_prefix(root)
        .map_err(|_| "configuration must be inside this repository")?;
    let definitions = definitions::collect(root, definitions::default_scope())?;
    let (implementation, coverage) = collect(root, config_path, &definitions)?;
    let mut reports = reports(&implementation, &coverage, root, &out_dir)?;
    // The comparison and definitions always describe the same source snapshot.
    reports.extend(definitions::reports(&definitions, root, &out_dir)?);
    definitions::save(&reports, &out_dir, check)?;
    println!(
        "{} {} variant records; {} missing context records; {} orphan IDs",
        if check { "Checked" } else { "Generated" },
        coverage.items.len(),
        coverage
            .summary
            .get("missing_evidence")
            .copied()
            .unwrap_or(0),
        coverage.orphan_evidence.len()
    );
    if require {
        require_evidence(&coverage)?;
    }
    Ok(())
}
