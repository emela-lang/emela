//! 補完（LSP の `textDocument/completion`）．
//!
//! 書きかけのソースは構文が崩れているので，文脈（import の中か，`.` の後ろか）はカーソルの前の
//! テキストで決め，候補は名前解決と型推論の結果から引く．

use emela_resolve::hir::{
    DefId, DefKind, Expr, ExprId, ExprKind, Fields, Item, LocalId, PatId, PatKind, Program, Res,
    Stmt,
};
use emela_resolve::{ModuleId, ModuleName};
use emela_syntax::ast::AstNode;
use emela_types::{Ty, TyParam};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// 補完の候補．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    /// 型やモジュール名など，候補の横に出す説明．
    pub detail: Option<String>,
    /// 選んだときに入れる文字列．`None` なら `label`．
    pub insert_text: Option<String>,
}

/// 候補の種類．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    /// 局所変数（引数，束縛，パターンの変数）．
    Local,
    Fn,
    Const,
    /// 構成子（type の構成子，enum のバリアント）．
    Ctor,
    Type,
    Enum,
    Error,
    Effect,
    Handler,
    Layer,
    Trait,
    /// effect の操作と trait の関数．
    Method,
    Field,
    Module,
    Keyword,
    /// 名前付き引数のラベル（`h: `）．
    Label,
}

/// 予約語（3.1）のうち，書き始めに補える語．`as` は構文を持たないので出さない．
const KEYWORDS: &[&str] = &[
    "fn", "type", "enum", "error", "const", "trait", "impl", "effect", "handler", "layer",
    "import", "pub", "opaque", "suspend", "if", "else", "match", "fail", "escape", "use", "with",
    "assert", "self", "Self",
];

/// `file` の `offset` の位置の補完の候補．書きかけの名前で絞り込むのはエディタに任せる．
pub(crate) fn complete(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Vec<Completion> {
    let Some(module) = module_of(analysis, file) else {
        return Vec::new();
    };
    let text = analysis.sources[file].text();
    let Some(before) = text.get(..usize::from(offset)) else {
        return Vec::new();
    };
    let cx = Context {
        program,
        analysis,
        module,
        text,
    };
    // 書きかけの名前の前．
    let start = before.trim_end_matches(is_name_char).len();
    let line = &before[before[..start].rfind('\n').map_or(0, |i| i + 1)..start];
    let mut items = if let Some(rest) = line.trim_start().strip_prefix("import ") {
        cx.import(rest)
    } else if before[..start].ends_with('.') {
        cx.after_dot(&before[..start - 1])
    } else {
        match before[..start]
            .trim_end()
            .rsplit(|c: char| !is_name_char(c))
            .next()
        {
            Some("use") => cx.in_scope(offset, |kind| kind == DefKind::Effect),
            Some("with") => cx.in_scope(offset, |kind| {
                matches!(kind, DefKind::Handler | DefKind::Layer)
            }),
            _ => {
                // 呼び出しの引数の書き始めなら，まだ書いていないラベルを先に出す．
                let mut items = labels(program, analysis, file, offset, start);
                items.extend(cx.in_scope(offset, |_| true));
                items.extend(KEYWORDS.iter().map(|k| Completion {
                    label: (*k).to_owned(),
                    kind: CompletionKind::Keyword,
                    detail: None,
                    insert_text: None,
                }));
                items
            }
        }
    };
    // 同じ名前は先に見つけたもの（内側のもの）を残す．
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.label.clone()));
    items
}

/// 呼び出しの引数の書き始め（`f(` の後ろかカンマの後ろで，書きかけの名前だけ）なら，呼ぶ先の
/// ラベルのうちまだ書いていないもの．選ぶと `label: ` を入れる．説明はその引数の型．
fn labels(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
    start: usize,
) -> Vec<Completion> {
    let text = analysis.sources[file].text();
    let Some((callee, open)) = crate::signature::call_at(program, analysis, file, offset) else {
        return Vec::new();
    };
    let Some(args) = text.get(usize::from(open) + 1..start) else {
        return Vec::new();
    };
    let parts = crate::signature::split_args(args);
    let (current, written) = parts.split_last().expect("1つはある");
    if !current.trim().is_empty() {
        return Vec::new();
    }
    let written: Vec<&str> = written
        .iter()
        .filter_map(|a| crate::signature::arg_label(a))
        .collect();
    let Some((_, Ty::Fn(f), params, labels)) = crate::signature::callee_type(program, callee)
    else {
        return Vec::new();
    };
    let cons = &program.types.ty_table.cons;
    labels
        .iter()
        .zip(&f.params)
        .filter_map(|(label, ty)| {
            let label = label.as_deref()?;
            (!written.contains(&label)).then(|| Completion {
                label: format!("{label}:"),
                kind: CompletionKind::Label,
                detail: Some(ty.display_with(cons, &params).to_string()),
                insert_text: Some(format!("{label}: ")),
            })
        })
        .collect()
}

struct Context<'a> {
    program: &'a TypedProgram,
    analysis: &'a Analysis,
    module: ModuleId,
    text: &'a str,
}

impl Context<'_> {
    fn hir(&self) -> &Program {
        &self.program.hir
    }

    /// `import` の後ろ．`{` の中ならモジュールの pub な名前，その前ならモジュール名の次の区切り．
    fn import(&self, rest: &str) -> Vec<Completion> {
        if let Some((path, _)) = rest.split_once('{') {
            let name = ModuleName::parse(path.trim().trim_end_matches('.'));
            let Some(target) = self.analysis.modules.lookup(&name) else {
                return Vec::new();
            };
            return self.members(target);
        }
        // `import Http.` なら `Http.` で始まるモジュール名の次の区切り．
        let typed: Vec<&str> = rest.trim().split('.').collect();
        let typed = &typed[..typed.len() - 1];
        let mut items: Vec<Completion> = self
            .analysis
            .modules
            .iter()
            .filter(|&(id, _)| id != self.module)
            .filter_map(|(_, data)| {
                let segments = data.name.segments();
                if segments.len() <= typed.len() || segments.iter().zip(typed).any(|(s, t)| s != t)
                {
                    return None;
                }
                Some(Completion {
                    label: segments[typed.len()].to_string(),
                    kind: CompletionKind::Module,
                    detail: Some(data.name.to_string()),
                    insert_text: None,
                })
            })
            .collect();
        items.sort_by(|a, b| a.label.cmp(&b.label));
        items
    }

    /// `.` の後ろ．左が大文字名の並びならモジュールか Trait の中身，そうでなければ式の型から
    /// 能力の値の操作か，type のフィールド．
    fn after_dot(&self, left: &str) -> Vec<Completion> {
        let path = &left[left.trim_end_matches(|c| is_name_char(c) || c == '.').len()..];
        let segments: Vec<&str> = path.split('.').collect();
        if segments
            .iter()
            .all(|s| s.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
        {
            return match self.dot_left(&segments) {
                Some(Left::Module(m)) => self.members(m),
                Some(Left::Trait(t)) => self.trait_fns(t),
                None => Vec::new(),
            };
        }
        self.expr_members(TextSize::of(left))
    }

    /// `.` の左の名前（4.6）．import した名前か core のモジュール，なければ Trait．
    /// 2つ目からはモジュールの pub な Trait．
    fn dot_left(&self, segments: &[&str]) -> Option<Left> {
        let (first, rest) = segments.split_first()?;
        let mut left = if let Some(m) = self.imported_module(first) {
            Left::Module(m)
        } else if let Some(m) = self
            .analysis
            .modules
            .lookup(&ModuleName::new([*first]))
            .filter(|&m| self.analysis.modules.is_core(m))
        {
            Left::Module(m)
        } else {
            Left::Trait(self.trait_named(self.module, first, false)?)
        };
        for segment in rest {
            let Left::Module(m) = left else { return None };
            left = Left::Trait(self.trait_named(m, segment, true)?);
        }
        Some(left)
    }

    /// このファイルの `import A.B` が入れたモジュール `B`．
    fn imported_module(&self, alias: &str) -> Option<ModuleId> {
        imports(self.text)
            .into_iter()
            .find(|(path, _)| path.last() == alias)
            .and_then(|(path, _)| self.analysis.modules.lookup(&path))
    }

    /// モジュールの Trait か Prelude の Trait．
    fn trait_named(&self, module: ModuleId, name: &str, public: bool) -> Option<DefId> {
        let hir = self.hir();
        let own = hir.modules.get(module)?.items.iter().copied();
        let prelude = hir.prelude.defs.iter().copied().filter(|_| !public);
        own.chain(prelude).find(|&def| {
            let data = &hir.defs[def];
            data.kind == DefKind::Trait && data.name == name && (!public || data.is_pub)
        })
    }

    /// モジュールの pub な定義（ほかのモジュールから引ける名前）．enum のバリアントも入れる．
    fn members(&self, module: ModuleId) -> Vec<Completion> {
        let hir = self.hir();
        let Some(items) = hir.modules.get(module) else {
            return Vec::new();
        };
        let mut defs = Vec::new();
        for &def in &items.items {
            defs.push(def);
            if let Some(Item::Enum(e)) = hir.item(def) {
                defs.extend(e.variants.iter().copied());
            }
        }
        defs.into_iter()
            .filter(|&def| hir.defs[def].is_pub)
            .filter_map(|def| self.def_item(def))
            .collect()
    }

    fn trait_fns(&self, def: DefId) -> Vec<Completion> {
        match self.hir().item(def) {
            Some(Item::Trait(t)) => t.fns.iter().filter_map(|&f| self.def_item(f)).collect(),
            _ => Vec::new(),
        }
    }

    /// `.` の直前で終わる式の型から，能力の値の操作か type と error のフィールド．
    fn expr_members(&self, end: TextSize) -> Vec<Completion> {
        let hir = self.hir();
        let types = &self.program.types;
        // 同じ位置で終わる式のうち一番外側（`a.b.` なら `a.b`）．
        let Some((expr, _)) = hir
            .exprs
            .iter()
            .filter(|(_, e)| e.span.module == self.module && e.span.range.end() == end)
            .filter(|(_, e)| !matches!(e.kind, ExprKind::Missing))
            .max_by_key(|(_, e)| e.span.range.len())
        else {
            return Vec::new();
        };
        let ty = self.expr_ty(expr);
        match ty.as_ref() {
            Some(Ty::Cap(effect)) => match hir.item(effect.def) {
                Some(Item::Effect(e)) => e.ops.iter().filter_map(|&op| self.def_item(op)).collect(),
                _ => Vec::new(),
            },
            Some(Ty::Named(con, _)) => {
                let def = types.ty_table.def(*con);
                let fields = match hir.item(def) {
                    Some(Item::Type(t)) => match &t.fields {
                        Fields::Named(fields) => fields,
                        _ => return Vec::new(),
                    },
                    Some(Item::Error(e)) => match &e.fields {
                        Fields::Named(fields) => fields,
                        _ => return Vec::new(),
                    },
                    // ハンドラの値（`self.`，8.4）．
                    Some(Item::Handler(h)) => &h.fields,
                    _ => return Vec::new(),
                };
                fields
                    .iter()
                    .map(|f| Completion {
                        label: f.name.to_string(),
                        kind: CompletionKind::Field,
                        detail: hir.types[f.ty].span.and_then(|s| self.source_text(s)),
                        insert_text: None,
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// 式の型．型推論の表になければ（`log.` や `a.b.` の名前のない `.` は名前解決が `Missing` にし，
    /// 左の式は型推論に回らない），左からたどって求める．局所変数と定義はその宣言の型，
    /// `x.f` はフィールドの型か能力の値の操作の型，呼び出しは呼ぶ先の関数型の戻り値．
    fn expr_ty(&self, expr: ExprId) -> Option<Ty> {
        let hir = self.hir();
        let types = &self.program.types;
        if let Some(ty) = types.exprs.get(expr) {
            return Some(ty.clone());
        }
        match &hir.exprs[expr].kind {
            ExprKind::Path(Res::Local(local)) => types.locals.get(*local).cloned(),
            ExprKind::Path(Res::Def(def)) => types.sigs.get(*def).map(|sig| sig.scheme.ty.clone()),
            ExprKind::Field { base, name } => match self.expr_ty(*base)? {
                Ty::Cap(effect) => {
                    let Some(Item::Effect(e)) = hir.item(effect.def) else {
                        return None;
                    };
                    let op = e.ops.iter().find(|&&op| hir.defs[op].name == name.name)?;
                    // エフェクトの型引数は能力の値の型引数で置き換える（8.1）．
                    types
                        .sigs
                        .get(*op)
                        .map(|sig| emela_types::substitute_bound(&sig.scheme.ty, &effect.args))
                }
                Ty::Named(con, args) => self.field_ty(types.ty_table.def(con), &args, &name.name),
                _ => None,
            },
            ExprKind::Call { callee, .. } => match self.expr_ty(*callee)? {
                Ty::Fn(f) => Some(f.ret),
                _ => None,
            },
            _ => None,
        }
    }

    /// type か error の名前付きのフィールドの型．構成子のシグネチャ（引数がフィールドの型）から引き，
    /// 型引数を `args` で置き換える．
    fn field_ty(&self, def: DefId, args: &[Ty], name: &str) -> Option<Ty> {
        let hir = self.hir();
        let ctor = match hir.item(def)? {
            Item::Type(t) => t.ctor?,
            Item::Error(_) => def,
            Item::Handler(h) => h.ctor?,
            _ => return None,
        };
        let sig = self.program.types.sigs.get(ctor)?;
        let index = sig.labels.iter().position(|l| l.as_deref() == Some(name))?;
        let Ty::Fn(f) = &sig.scheme.ty else {
            return None;
        };
        Some(emela_types::substitute_bound(f.params.get(index)?, args))
    }

    /// 位置から見える名前．局所変数（内側が先），このモジュールの定義，import した名前と
    /// モジュール，core のモジュール，Prelude の順．`want` で定義の種類を絞る（局所変数と
    /// モジュールは種類を絞らないときだけ出す）．
    fn in_scope(&self, offset: TextSize, want: impl Fn(DefKind) -> bool) -> Vec<Completion> {
        let hir = self.hir();
        let all = want(DefKind::Fn) && want(DefKind::Type);
        let mut items = Vec::new();
        if all {
            let params = self.enclosing_params(offset);
            for local in visible_locals(hir, self.module, offset) {
                let data = &hir.locals[local];
                let detail = self.program.types.locals.get(local).map(|ty| {
                    ty.display_with(&self.program.types.ty_table.cons, &params)
                        .to_string()
                });
                items.push(Completion {
                    label: data.name.to_string(),
                    kind: CompletionKind::Local,
                    detail,
                    insert_text: None,
                });
            }
        }
        let mut defs: Vec<DefId> = Vec::new();
        if let Some(own) = hir.modules.get(self.module) {
            for &def in &own.items {
                defs.push(def);
                if let Some(Item::Enum(e)) = hir.item(def) {
                    defs.extend(e.variants.iter().copied());
                }
            }
        }
        let imports = imports(self.text);
        for (path, names) in &imports {
            let Some(target) = self.analysis.modules.lookup(path) else {
                continue;
            };
            let Some(module) = hir.modules.get(target) else {
                continue;
            };
            let mut found: Vec<DefId> = module.items.clone();
            for &def in &module.items {
                if let Some(Item::Enum(e)) = hir.item(def) {
                    found.extend(e.variants.iter().copied());
                }
            }
            defs.extend(found.into_iter().filter(|&def| {
                let data = &hir.defs[def];
                data.is_pub && names.iter().any(|n| *n == data.name)
            }));
        }
        defs.extend(hir.prelude.defs.iter().copied());
        items.extend(
            defs.into_iter()
                .filter(|&def| want(hir.defs[def].kind))
                .filter_map(|def| self.def_item(def)),
        );
        if all {
            // import したモジュールと，import なしで `.` の左に書ける core のモジュール．
            let modules = imports.iter().map(|(path, _)| path.clone()).chain(
                self.analysis
                    .modules
                    .iter()
                    .filter(|&(id, data)| {
                        self.analysis.modules.is_core(id)
                            && data.name.segments().len() == 1
                            && data.name.last() != emela_resolve::PRELUDE
                    })
                    .map(|(_, data)| data.name.clone()),
            );
            for name in modules {
                items.push(Completion {
                    label: name.last().to_string(),
                    kind: CompletionKind::Module,
                    detail: Some(name.to_string()),
                    insert_text: None,
                });
            }
        }
        items
    }

    /// 定義の候補．種類のない定義（impl，型引数）は出さない．型の名前と同じ名前の構成子は
    /// 型として出す（名前が同じなので後で1つにまとまる）．
    fn def_item(&self, def: DefId) -> Option<Completion> {
        let data = &self.hir().defs[def];
        let kind = match data.kind {
            DefKind::Fn => CompletionKind::Fn,
            DefKind::Const => CompletionKind::Const,
            DefKind::Ctor => CompletionKind::Ctor,
            DefKind::Type | DefKind::BuiltinType => CompletionKind::Type,
            DefKind::Enum => CompletionKind::Enum,
            DefKind::Error => CompletionKind::Error,
            DefKind::Effect => CompletionKind::Effect,
            DefKind::Handler => CompletionKind::Handler,
            DefKind::Layer => CompletionKind::Layer,
            DefKind::Trait => CompletionKind::Trait,
            DefKind::Op | DefKind::TraitFn => CompletionKind::Method,
            DefKind::Impl | DefKind::TypeParam => return None,
        };
        let types = &self.program.types;
        let detail = types
            .sigs
            .get(def)
            .map(|sig| sig.scheme.display(&types.ty_table.cons).to_string());
        Some(Completion {
            label: data.name.to_string(),
            kind,
            detail,
            insert_text: None,
        })
    }

    /// 位置を本体に含む定義の型引数（局所変数の型の `Bound(i)` の名前）．
    fn enclosing_params(&self, offset: TextSize) -> Vec<TyParam> {
        crate::hover::enclosing_def(self.program, self.module, offset)
            .and_then(|def| self.program.types.sigs.get(def))
            .map(|sig| sig.scheme.params.clone())
            .unwrap_or_default()
    }

    fn source_text(&self, span: emela_resolve::hir::Span) -> Option<String> {
        let file = *self.analysis.files.get(span.module)?;
        let text = self.analysis.sources[file].text();
        text.get(std::ops::Range::<usize>::from(span.range))
            .map(str::to_owned)
    }
}

enum Left {
    Module(ModuleId),
    Trait(DefId),
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn module_of(analysis: &Analysis, file: FileId) -> Option<ModuleId> {
    analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)
}

/// ファイルの import．モジュール名と，波括弧で列挙した名前．
fn imports(text: &str) -> Vec<(ModuleName, Vec<String>)> {
    emela_syntax::parse(text)
        .tree()
        .imports()
        .filter_map(|import| {
            let path = import.path()?;
            let name = ModuleName::new(path.segments().map(|t| t.text().to_owned()));
            if name.segments().is_empty() || path.syntax().text_range().is_empty() {
                return None;
            }
            Some((name, import.names().map(|t| t.text().to_owned()).collect()))
        })
        .collect()
}

/// 位置から見える局所変数．内側（後で束縛したもの）が先．同じ名前は内側だけ．
fn visible_locals(
    hir: &Program,
    module: emela_resolve::ModuleId,
    offset: TextSize,
) -> Vec<LocalId> {
    let mut scopes: Vec<(LocalId, TextRange)> = Vec::new();
    let mut add = |locals: Vec<LocalId>, range: TextRange| {
        scopes.extend(locals.into_iter().map(|l| (l, range)));
    };
    let span_of = |expr: emela_resolve::hir::ExprId| hir.exprs[expr].span.range;
    for (_, item) in hir.items.iter() {
        match item {
            Item::Fn(f) | Item::TraitFn(f) => {
                if let Some(body) = f.body {
                    add(
                        f.sig.params.iter().map(|p| p.local).collect(),
                        span_of(body),
                    );
                }
            }
            Item::Handler(h) => {
                for op in &h.ops {
                    if let Some(body) = op.body {
                        let mut locals = vec![op.self_local];
                        locals.extend(op.params.iter().map(|p| p.local));
                        add(locals, span_of(body));
                    }
                }
                if let Some((local, body)) = h.release {
                    add(vec![local], span_of(body));
                }
            }
            Item::Impl(i) => {
                for f in &i.fns {
                    if let Some(body) = f.body {
                        add(f.params.iter().map(|p| p.local).collect(), span_of(body));
                    }
                }
            }
            _ => {}
        }
    }
    for (_, Expr { kind, span }) in hir.exprs.iter() {
        if span.module != module || !span.range.contains_inclusive(offset) {
            continue;
        }
        match kind {
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    if let Stmt::Let { pat, value, .. } = stmt {
                        // 右辺の後ろからブロックの終わりまで（6.3）．
                        let start = span_of(*value).end();
                        if start <= span.range.end() {
                            add(
                                pat_locals(hir, *pat),
                                TextRange::new(start, span.range.end()),
                            );
                        }
                    }
                }
            }
            ExprKind::Lambda { params, body } => {
                add(params.iter().map(|p| p.local).collect(), span_of(*body));
            }
            ExprKind::Match { arms, .. } | ExprKind::Escape { arms, .. } => {
                for arm in arms {
                    let range = hir.pats[arm.pat].span.range.cover(span_of(arm.body));
                    add(pat_locals(hir, arm.pat), range);
                }
            }
            _ => {}
        }
    }
    let mut visible: Vec<LocalId> = scopes
        .into_iter()
        .filter(|&(local, range)| {
            hir.locals[local].span.module == module && range.contains_inclusive(offset)
        })
        .map(|(local, _)| local)
        .collect();
    // 内側（後で束縛したもの）を先に．
    visible.sort_by_key(|&l| std::cmp::Reverse(hir.locals[l].span.range.start()));
    let mut seen = std::collections::HashSet::new();
    visible.retain(|&l| seen.insert(hir.locals[l].name.clone()));
    visible
}

/// パターンが束縛する局所変数．
fn pat_locals(hir: &Program, pat: PatId) -> Vec<LocalId> {
    let mut locals = Vec::new();
    let mut stack = vec![pat];
    while let Some(pat) = stack.pop() {
        match &hir.pats[pat].kind {
            PatKind::Bind(local) => locals.push(*local),
            PatKind::Ctor {
                args: Some(args), ..
            } => stack.extend(args.iter().map(|a| a.pat)),
            PatKind::Tuple(pats) => stack.extend(pats.iter().copied()),
            PatKind::List { elems, rest } => {
                stack.extend(elems.iter().copied());
                if let Some(binding) = rest.and_then(|r| r.binding) {
                    locals.push(binding);
                }
            }
            _ => {}
        }
    }
    locals
}
