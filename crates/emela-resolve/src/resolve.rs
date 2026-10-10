//! 名前解決の入口．宣言を集め，import を解き，宣言の中身を HIR へ写す．
//!
//! 段は3つ．
//!
//! 1. 宣言の収集: モジュールごとに，名前空間（型，構成子，値）を分けて定義を登録する
//! 2. import: 全モジュールの宣言が揃ってから，列挙した名前とモジュールをスコープに入れる
//! 3. 写す: 宣言の中身（型，式，パターン）を HIR にする（[`crate::lower`]）
//!
//! 名前空間は位置で分ける．`.` の左ではモジュールと Trait だけを，型の位置では型だけを，
//! 式とパターンの型名では構成子だけを引く．モジュール `Email` とその中の型 `Email`（4.4）は
//! 位置で区別がつくので衝突しない．

use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{SyntaxNode, SyntaxToken};
use la_arena::ArenaMap;
use rowan::TextRange;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::builtins::{BuiltinKind, BuiltinModules, build_prelude};
use crate::diagnostic::{Diagnostic, DiagnosticKind, Expected};
use crate::hir::{DefData, DefId, DefKind, DefModule, Program, Span};
use crate::source::{ModuleId, ModuleMap};
use crate::suggest::best_match;
use crate::{Import, ModuleName};

/// モジュールの名前空間．モジュール名の名前空間は [`ModuleScope::modules`] に別に持つ．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Ns {
    /// 型名: type，enum，error，effect，handler，layer，trait．
    Type = 0,
    /// 式とパターンの型名: 構成子，バリアント，error，フィールドのある handler．
    Ctor = 1,
    /// 小文字名と大文字名: fn と const．
    Value = 2,
}

const NAMESPACES: [Ns; 3] = [Ns::Type, Ns::Ctor, Ns::Value];

/// `.` の左に書けるモジュール．
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModRef {
    Source(ModuleId),
    Builtin(SmolStr),
}

#[derive(Debug, Clone)]
pub(crate) struct Entry<T> {
    pub(crate) target: T,
    /// 定義の名前か import の名前の位置．
    pub(crate) range: Option<TextRange>,
    /// import で入った名前．他のモジュールへは見せない．
    pub(crate) imported: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ModuleScope {
    names: [FxHashMap<SmolStr, Entry<DefId>>; 3],
    pub(crate) modules: FxHashMap<SmolStr, Entry<ModRef>>,
}

impl ModuleScope {
    pub(crate) fn get(&self, ns: Ns, name: &str) -> Option<&Entry<DefId>> {
        self.names[ns as usize].get(name)
    }

    pub(crate) fn names(&self, ns: Ns) -> impl Iterator<Item = &SmolStr> {
        self.names[ns as usize].keys()
    }
}

/// 写す段に渡す宣言．
pub(crate) struct Pending {
    pub(crate) module: ModuleId,
    pub(crate) def: DefId,
    pub(crate) item: ast::Item,
    /// バリアント，effect の操作，trait の関数．宣言の順．
    pub(crate) children: Vec<DefId>,
    /// type と handler の構成子．
    pub(crate) ctor: Option<DefId>,
}

/// `.` の左の解決先．
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DotLeft {
    Module(ModRef),
    Trait(DefId),
}

pub(crate) struct Resolver<'a> {
    pub(crate) modules: &'a ModuleMap,
    builtins: &'a dyn BuiltinModules,
    builtin_modules: Vec<SmolStr>,
    builtin_defs: FxHashMap<(SmolStr, SmolStr), DefId>,
    pub(crate) program: Program,
    pub(crate) scopes: ArenaMap<ModuleId, ModuleScope>,
    prelude: [FxHashMap<SmolStr, DefId>; 3],
    /// trait の関数と effect の操作．名前で引く．
    pub(crate) members: FxHashMap<DefId, Vec<DefId>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// 全モジュールの名前を解決する．`files` にないモジュール（読めなかったファイル）は空とみなす．
///
/// 診断はモジュールの順，その中では位置の順に並ぶ．
pub fn resolve(
    modules: &ModuleMap,
    files: &ArenaMap<ModuleId, ast::SourceFile>,
    builtins: &dyn BuiltinModules,
) -> (Program, Vec<Diagnostic>) {
    let mut resolver = Resolver::new(modules, builtins);
    let mut pending = Vec::new();
    for (module, _) in modules.iter() {
        resolver.scopes.insert(module, ModuleScope::default());
        resolver.program.modules.insert(module, Default::default());
        if let Some(file) = files.get(module) {
            pending.extend(resolver.collect(module, file));
        }
    }
    for (module, _) in modules.iter() {
        if let Some(file) = files.get(module) {
            resolver.imports(module, file);
        }
        resolver.warn_prelude_shadowing(module);
    }
    for pending in pending {
        crate::lower::lower_item(&mut resolver, pending);
    }
    let Resolver {
        program,
        mut diagnostics,
        ..
    } = resolver;
    // モジュールの順，同じファイルの中は位置の順に並べ直す．同じ位置なら見つけた順．
    let order: FxHashMap<_, _> = modules
        .iter()
        .enumerate()
        .map(|(i, (_, m))| (m.file.clone(), i))
        .collect();
    diagnostics.sort_by_key(|d| {
        let file = order.get(&d.file).copied().unwrap_or(usize::MAX);
        (file, d.range.map(|r| (r.start(), r.end())))
    });
    (program, diagnostics)
}

/// 構文木から import の一覧を取り出す（import のグラフに渡す）．
pub fn imports(file: &ast::SourceFile) -> Vec<Import> {
    file.imports()
        .filter_map(|import| {
            let path = import.path()?;
            let segments: Vec<SmolStr> = path.segments().map(|t| t.text().into()).collect();
            if segments.is_empty() {
                return None;
            }
            Some(Import {
                path: ModuleName::new(segments),
                range: path.syntax().text_range(),
            })
        })
        .collect()
}

pub(crate) fn token_name(token: &SyntaxToken) -> (SmolStr, TextRange) {
    (token.text().into(), token.text_range())
}

impl<'a> Resolver<'a> {
    fn new(modules: &'a ModuleMap, builtins: &'a dyn BuiltinModules) -> Self {
        let mut program = Program::default();
        build_prelude(&mut program);
        let mut prelude: [FxHashMap<SmolStr, DefId>; 3] = Default::default();
        for &def in &program.prelude.defs {
            let data = &program.defs[def];
            let ns = match data.kind {
                DefKind::Ctor => Ns::Ctor,
                DefKind::Fn => Ns::Value,
                _ => Ns::Type,
            };
            prelude[ns as usize].insert(data.name.clone(), def);
        }
        let mut members: FxHashMap<DefId, Vec<DefId>> = FxHashMap::default();
        for (def, data) in program.defs.iter() {
            if data.kind == DefKind::TraitFn
                && let Some(parent) = data.parent
            {
                members.entry(parent).or_default().push(def);
            }
        }
        Resolver {
            modules,
            builtins,
            builtin_modules: builtins.modules(),
            builtin_defs: FxHashMap::default(),
            program,
            scopes: ArenaMap::default(),
            prelude,
            members,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn error(&mut self, module: ModuleId, range: TextRange, kind: DiagnosticKind) {
        self.diagnostics.push(Diagnostic {
            file: self.modules[module].file.clone(),
            range: Some(range),
            kind,
        });
    }

    pub(crate) fn alloc_def(
        &mut self,
        module: ModuleId,
        name: SmolStr,
        range: TextRange,
        kind: DefKind,
        parent: Option<DefId>,
    ) -> DefId {
        let (is_pub, is_opaque) = match parent {
            Some(p) if kind == DefKind::Ctor => {
                (self.program.defs[p].is_pub, self.program.defs[p].is_opaque)
            }
            _ => (false, false),
        };
        self.program.defs.alloc(DefData {
            name,
            kind,
            module: DefModule::Source(module),
            parent,
            span: Some(Span { module, range }),
            is_pub,
            is_opaque,
        })
    }

    /// モジュールの名前空間に名前を入れる．同じ名前空間に先の名前があれば重複の診断を出し，
    /// 先の方を残す．
    fn insert(&mut self, module: ModuleId, ns: Ns, name: SmolStr, entry: Entry<DefId>) {
        let scope = &mut self.scopes[module];
        match scope.names[ns as usize].get(&name) {
            Some(first) => {
                // 同じ定義を別の import で入れ直しただけなら重複にしない．
                if first.target == entry.target {
                    return;
                }
                let first = first.range;
                if let Some(range) = entry.range {
                    self.error(module, range, DiagnosticKind::Duplicate { name, first });
                }
            }
            None => {
                scope.names[ns as usize].insert(name, entry);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 1. 宣言の収集

    fn collect(&mut self, module: ModuleId, file: &ast::SourceFile) -> Vec<Pending> {
        let mut pending = Vec::new();
        let mut impl_count = 0;
        for item in file.items() {
            let syntax = item.syntax().clone();
            let (kind, name_token, is_pub, is_opaque) = match &item {
                ast::Item::Import(_) => continue,
                ast::Item::Fn(it) => (DefKind::Fn, it.name(), it.is_pub(), false),
                ast::Item::Type(it) => (DefKind::Type, it.name(), it.is_pub(), it.is_opaque()),
                ast::Item::Enum(it) => (DefKind::Enum, it.name(), it.is_pub(), it.is_opaque()),
                ast::Item::Error(it) => (DefKind::Error, it.name(), it.is_pub(), false),
                ast::Item::Const(it) => (DefKind::Const, it.name(), it.is_pub(), false),
                ast::Item::Effect(it) => (DefKind::Effect, it.name(), it.is_pub(), false),
                ast::Item::Handler(it) => (DefKind::Handler, it.name(), it.is_pub(), false),
                ast::Item::Layer(it) => (DefKind::Layer, it.name(), it.is_pub(), false),
                ast::Item::Trait(it) => (DefKind::Trait, it.name(), it.is_pub(), false),
                ast::Item::Impl(_) => (DefKind::Impl, None, false, false),
            };
            let (name, range) = match &name_token {
                Some(token) => token_name(token),
                None if kind == DefKind::Impl => {
                    impl_count += 1;
                    (format!("impl#{impl_count}").into(), syntax.text_range())
                }
                // 名前の読めない宣言は構文の診断が出ている．
                None => continue,
            };
            let def = self.alloc_def(module, name.clone(), range, kind, None);
            self.program.defs[def].is_pub = is_pub;
            self.program.defs[def].is_opaque = is_opaque;
            self.program.modules[module].items.push(def);
            let entry = Entry {
                target: def,
                range: Some(range),
                imported: false,
            };
            match kind {
                DefKind::Fn | DefKind::Const => self.insert(module, Ns::Value, name.clone(), entry),
                DefKind::Error => {
                    self.insert(module, Ns::Type, name.clone(), entry.clone());
                    self.insert(module, Ns::Ctor, name.clone(), entry);
                }
                DefKind::Impl => {}
                _ => self.insert(module, Ns::Type, name.clone(), entry),
            }

            let mut children = Vec::new();
            let mut ctor = None;
            match &item {
                ast::Item::Type(it) if it.field_list().is_some() => {
                    ctor = Some(self.define_ctor(module, def, &name, range));
                }
                ast::Item::Handler(it) if it.field_list().is_some() => {
                    ctor = Some(self.define_ctor(module, def, &name, range));
                }
                ast::Item::Enum(it) => {
                    for variant in it.variants() {
                        let Some(token) = variant.name() else {
                            continue;
                        };
                        let (vname, vrange) = token_name(&token);
                        let v =
                            self.alloc_def(module, vname.clone(), vrange, DefKind::Ctor, Some(def));
                        self.insert(
                            module,
                            Ns::Ctor,
                            vname,
                            Entry {
                                target: v,
                                range: Some(vrange),
                                imported: false,
                            },
                        );
                        children.push(v);
                    }
                }
                ast::Item::Effect(it) => {
                    let names = it.ops().filter_map(|op| op.name());
                    children = self.define_members(module, def, DefKind::Op, names);
                }
                ast::Item::Trait(it) => {
                    let names = it.fns().filter_map(|f| f.name());
                    children = self.define_members(module, def, DefKind::TraitFn, names);
                }
                _ => {}
            }
            if matches!(kind, DefKind::Effect | DefKind::Trait) {
                self.members.insert(def, children.clone());
            }
            pending.push(Pending {
                module,
                def,
                item,
                children,
                ctor,
            });
        }
        pending
    }

    /// type と handler の，同じ名前の構成子．
    fn define_ctor(
        &mut self,
        module: ModuleId,
        owner: DefId,
        name: &SmolStr,
        range: TextRange,
    ) -> DefId {
        let ctor = self.alloc_def(module, name.clone(), range, DefKind::Ctor, Some(owner));
        self.insert(
            module,
            Ns::Ctor,
            name.clone(),
            Entry {
                target: ctor,
                range: Some(range),
                imported: false,
            },
        );
        ctor
    }

    /// effect の操作と trait の関数．名前は宣言の中で重複できない．
    fn define_members(
        &mut self,
        module: ModuleId,
        parent: DefId,
        kind: DefKind,
        names: impl Iterator<Item = SyntaxToken>,
    ) -> Vec<DefId> {
        let mut seen: FxHashMap<SmolStr, TextRange> = FxHashMap::default();
        let mut defs = Vec::new();
        for token in names {
            let (name, range) = token_name(&token);
            if let Some(&first) = seen.get(&name) {
                self.error(
                    module,
                    range,
                    DiagnosticKind::Duplicate {
                        name,
                        first: Some(first),
                    },
                );
                continue;
            }
            seen.insert(name.clone(), range);
            defs.push(self.alloc_def(module, name, range, kind, Some(parent)));
        }
        defs
    }

    // -----------------------------------------------------------------------
    // 2. import

    fn imports(&mut self, module: ModuleId, file: &ast::SourceFile) {
        for import in file.imports() {
            let Some(path) = import.path() else { continue };
            let segments: Vec<SyntaxToken> = path.segments().collect();
            let Some(last) = segments.last() else {
                continue;
            };
            let name = ModuleName::new(segments.iter().map(|t| SmolStr::from(t.text())));
            // 未定義のモジュールは import のグラフが診断を出している．
            let Some(target) = self.modules.lookup(&name) else {
                continue;
            };
            let (alias, alias_range) = token_name(last);
            let scope = &mut self.scopes[module];
            match scope.modules.get(&alias) {
                Some(first) if first.target != ModRef::Source(target) => {
                    let first = first.range;
                    self.error(
                        module,
                        alias_range,
                        DiagnosticKind::Duplicate { name: alias, first },
                    );
                }
                Some(_) => {}
                None => {
                    scope.modules.insert(
                        alias,
                        Entry {
                            target: ModRef::Source(target),
                            range: Some(alias_range),
                            imported: true,
                        },
                    );
                }
            }
            for token in import.names() {
                self.import_name(module, target, &name, &token);
            }
        }
    }

    /// `import A.B.{x}` の `x`．pub な定義をすべての名前空間から取り込む．
    fn import_name(
        &mut self,
        module: ModuleId,
        target: ModuleId,
        target_name: &ModuleName,
        token: &SyntaxToken,
    ) {
        let (name, range) = token_name(token);
        let found: Vec<(Ns, DefId)> = NAMESPACES
            .iter()
            .filter_map(|&ns| {
                let entry = self.scopes[target].get(ns, &name)?;
                (!entry.imported).then_some((ns, entry.target))
            })
            .collect();
        if found.is_empty() {
            let suggestion = best_match(&name, self.exported_names(target));
            self.error(
                module,
                range,
                DiagnosticKind::UndefinedMember {
                    name,
                    owner: target_name.to_string().into(),
                    owner_kind: "module",
                    suggestion,
                },
            );
            return;
        }
        let public: Vec<_> = found
            .into_iter()
            .filter(|&(_, def)| self.program.defs[def].is_pub)
            .collect();
        if public.is_empty() {
            self.error(
                module,
                range,
                DiagnosticKind::Private {
                    name,
                    module: target_name.clone(),
                },
            );
            return;
        }
        for (ns, def) in public {
            self.insert(
                module,
                ns,
                name.clone(),
                Entry {
                    target: def,
                    range: Some(range),
                    imported: true,
                },
            );
        }
    }

    fn exported_names(&self, module: ModuleId) -> Vec<&SmolStr> {
        let scope = &self.scopes[module];
        NAMESPACES
            .iter()
            .flat_map(|&ns| {
                scope.names[ns as usize]
                    .iter()
                    .filter(|(_, e)| !e.imported)
                    .map(|(n, _)| n)
            })
            .collect()
    }

    /// モジュールの定義と import が Prelude の名前を隠していれば警告する．
    fn warn_prelude_shadowing(&mut self, module: ModuleId) {
        let mut shadowing: Vec<(TextRange, SmolStr)> = Vec::new();
        for ns in NAMESPACES {
            for (name, entry) in &self.scopes[module].names[ns as usize] {
                if self.prelude[ns as usize].contains_key(name)
                    && let Some(range) = entry.range
                {
                    shadowing.push((range, name.clone()));
                }
            }
        }
        // 型と構成子の両方を隠す定義（type，error）は1件にまとめる．
        shadowing.sort_by_key(|(range, _)| (range.start(), range.end()));
        shadowing.dedup();
        for (range, name) in shadowing {
            self.error(module, range, DiagnosticKind::ShadowsPrelude { name });
        }
    }

    // -----------------------------------------------------------------------
    // モジュールの水準での名前の引き方

    /// モジュールの名前空間と Prelude から引く．局所変数と型引数は呼ぶ側が先に見る．
    pub(crate) fn lookup(&self, module: ModuleId, ns: Ns, name: &str) -> Option<DefId> {
        self.scopes[module]
            .get(ns, name)
            .map(|e| e.target)
            .or_else(|| self.prelude[ns as usize].get(name).copied())
    }

    /// 「もしかして」の候補．モジュールの名前空間と Prelude の名前．
    pub(crate) fn candidates(&self, module: ModuleId, ns: Ns) -> Vec<SmolStr> {
        self.scopes[module]
            .names(ns)
            .chain(self.prelude[ns as usize].keys())
            .cloned()
            .collect()
    }

    /// `.` の左の型名を引く．モジュールの水準（定義と import）で見つからなければ，
    /// 組み込みのモジュールと Prelude の Trait を見る．同じ水準でモジュールと Trait の
    /// 両方が見つかれば曖昧として診断を出す．
    pub(crate) fn lookup_dot_left(
        &mut self,
        module: ModuleId,
        name: &SmolStr,
        range: TextRange,
    ) -> Option<DotLeft> {
        let scope = &self.scopes[module];
        let as_module = scope.modules.get(name).map(|e| e.target.clone());
        let as_trait = scope
            .get(Ns::Type, name)
            .map(|e| e.target)
            .filter(|&d| self.program.defs[d].kind == DefKind::Trait);
        let found = match (as_module, as_trait) {
            (Some(_), Some(_)) => {
                self.error(
                    module,
                    range,
                    DiagnosticKind::Ambiguous { name: name.clone() },
                );
                return None;
            }
            (Some(m), None) => Some(DotLeft::Module(m)),
            (None, Some(t)) => Some(DotLeft::Trait(t)),
            (None, None) => {
                if self.builtin_modules.contains(name) {
                    Some(DotLeft::Module(ModRef::Builtin(name.clone())))
                } else {
                    self.prelude[Ns::Type as usize]
                        .get(name)
                        .copied()
                        .filter(|&d| self.program.defs[d].kind == DefKind::Trait)
                        .map(DotLeft::Trait)
                }
            }
        };
        if found.is_some() {
            return found;
        }
        // 別の種類の名前なら，それを伝える．
        if let Some(def) = self.lookup(module, Ns::Type, name) {
            let found = self.program.defs[def].kind.describe();
            self.error(
                module,
                range,
                DiagnosticKind::WrongKind {
                    name: name.clone(),
                    expected: Expected::ModuleOrTrait,
                    found,
                },
            );
            return None;
        }
        let mut candidates: Vec<SmolStr> = self.scopes[module].modules.keys().cloned().collect();
        candidates.extend(self.builtin_modules.iter().cloned());
        candidates.extend(self.candidates(module, Ns::Type).into_iter().filter(|n| {
            self.lookup(module, Ns::Type, n)
                .is_some_and(|d| self.program.defs[d].kind == DefKind::Trait)
        }));
        let suggestion = best_match(name, &candidates);
        self.error(
            module,
            range,
            DiagnosticKind::UndefinedName {
                name: name.clone(),
                expected: Expected::ModuleOrTrait,
                suggestion,
            },
        );
        None
    }

    /// モジュールの項目を引く．他のモジュールの pub でない定義は診断を出してから返す
    /// （続きの解決は進める）．見つからなければ診断を出して `None`．
    pub(crate) fn module_member(
        &mut self,
        module: ModuleId,
        target: &ModRef,
        ns: Ns,
        name: &SmolStr,
        range: TextRange,
    ) -> Option<DefId> {
        match target {
            ModRef::Source(target) => {
                let target = *target;
                let found = self.scopes[target]
                    .get(ns, name)
                    .filter(|e| !e.imported)
                    .map(|e| e.target);
                let module_name = self.modules[target].name.clone();
                match found {
                    Some(def) => {
                        if target != module && !self.program.defs[def].is_pub {
                            self.error(
                                module,
                                range,
                                DiagnosticKind::Private {
                                    name: name.clone(),
                                    module: module_name,
                                },
                            );
                        }
                        Some(def)
                    }
                    None => {
                        let candidates: Vec<SmolStr> =
                            self.scopes[target].names(ns).cloned().collect();
                        let suggestion = best_match(name, &candidates);
                        self.error(
                            module,
                            range,
                            DiagnosticKind::UndefinedMember {
                                name: name.clone(),
                                owner: module_name.to_string().into(),
                                owner_kind: "module",
                                suggestion,
                            },
                        );
                        None
                    }
                }
            }
            ModRef::Builtin(target) => {
                let kind = self.builtins.lookup(target, name);
                let fits = match (kind, ns) {
                    (Some(BuiltinKind::Fn), Ns::Value) => Some(DefKind::Fn),
                    (Some(BuiltinKind::Type), Ns::Type) => Some(DefKind::BuiltinType),
                    _ => None,
                };
                let Some(kind) = fits else {
                    let candidates = self.builtins.names(target);
                    let suggestion = best_match(name, &candidates);
                    self.error(
                        module,
                        range,
                        DiagnosticKind::UndefinedMember {
                            name: name.clone(),
                            owner: target.clone(),
                            owner_kind: "module",
                            suggestion,
                        },
                    );
                    return None;
                };
                let key = (target.clone(), name.clone());
                if let Some(&def) = self.builtin_defs.get(&key) {
                    return Some(def);
                }
                let def = self.program.defs.alloc(DefData {
                    name: name.clone(),
                    kind,
                    module: DefModule::Builtin(target.clone()),
                    parent: None,
                    span: None,
                    is_pub: true,
                    is_opaque: false,
                });
                self.builtin_defs.insert(key, def);
                Some(def)
            }
        }
    }

    /// 組み込みのモジュール `X` が同じ名前の型 `X` を持てば，それを返す（`Map[K, V]`，5.7）．
    /// 型の位置で，型引数，モジュールの定義と import，Prelude のどれにもないときに使う．
    pub(crate) fn builtin_eponymous_type(
        &mut self,
        module: ModuleId,
        name: &SmolStr,
    ) -> Option<DefId> {
        if !self.builtin_modules.contains(name)
            || self.builtins.lookup(name, name) != Some(BuiltinKind::Type)
        {
            return None;
        }
        let target = ModRef::Builtin(name.clone());
        self.module_member(module, &target, Ns::Type, name, TextRange::default())
    }

    /// trait の関数か effect の操作を名前で引く．
    pub(crate) fn member(&self, owner: DefId, name: &str) -> Option<DefId> {
        self.members
            .get(&owner)?
            .iter()
            .copied()
            .find(|&d| self.program.defs[d].name == name)
    }

    pub(crate) fn member_names(&self, owner: DefId) -> Vec<SmolStr> {
        self.members
            .get(&owner)
            .into_iter()
            .flatten()
            .map(|&d| self.program.defs[d].name.clone())
            .collect()
    }

    /// 定義の持ち主のモジュール名（診断の文面用）．
    pub(crate) fn def_module_name(&self, def: DefId) -> ModuleName {
        match &self.program.defs[def].module {
            DefModule::Source(m) => self.modules[*m].name.clone(),
            DefModule::Prelude => ModuleName::new([crate::PRELUDE]),
            DefModule::Builtin(name) => ModuleName::new([name.clone()]),
        }
    }
}

/// ノードの直下のトークンを種類で探す．
pub(crate) fn child_token(
    node: &SyntaxNode,
    kind: emela_syntax::SyntaxKind,
) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}
