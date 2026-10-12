//! 型検査の結果（HIR と `ProgramTypes`）から Core IR を作る（lowering）．
//!
//! プログラム全体（利用者のモジュール，Prelude，core）を IR の [`Module`] 1つにする．
//! enum は先に全部割り振る（[`Lowerer::declare_enums`]）．fn は根からたどれるものだけを写す．
//! fn は参照に出会ったときに ID を割り振って作業リストに積み（[`Lowerer::fn_id`]），
//! 作業リストが尽きるまで本体を変換する．

use std::path::Path;

use emela_resolve::hir::{
    Arg, Arm as HirArm, BinaryOp, Block, DefId, DefKind, DefModule, ExprId, ExprKind, Fields, Item,
    Literal, LocalId, PatId, PatKind, Program, Res, Span, Stmt, StrPart as HirStrPart, UnaryOp,
};
use emela_resolve::{ModuleId, ModuleMap};
use emela_types::{Prim, ProgramTypes, Ty};
use la_arena::ArenaMap;

use crate::intrinsics::Builtin;
use crate::ir::{
    Arm, Assert, BinOp, Boundary, Callee, CtorRef, EffectDef, EffectId, EnumDef, EnumId, Expr,
    External, ExternalBinding, FnId, JsFile, Lit, Local, Module, OpDef, OpTy, Pat, StrPart, Type,
    UnOp, VariantDef, VariantFields,
};
use crate::named;

/// 型検査を通ったが IR にできなかった理由．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// alpha.1 ではまだ IR にできない．`what` は英語で何が，を書く．
    Unsupported { what: String, span: Option<Span> },
    /// エントリからたどれる外部関数に，ビルド対象のターゲットの `@external` がない（12.1）．
    /// `span` は関数の名前の位置．
    MissingExternal {
        name: String,
        target: String,
        span: Option<Span>,
    },
    /// 同梱の core の `@external(js, "./x.mjs", …)` の指すファイルが，同梱の JS のファイル
    /// （`LowerInput::core_js_files`）にない．同梱のソースの誤り．`path` は書かれたままの指定．
    MissingCoreFile { path: String, span: Option<Span> },
}

/// どの fn から写し始めるか．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roots {
    /// エントリのモジュールの export する fn（`pub` と `main`．テスト関数を除く）．通常のビルド．
    Exported,
    /// テスト関数（`LowerInput::tests`）．`emela test`．
    Tests,
    /// 本体のある全部の fn と const．使われない fn も写す．lowering のテストに使う．
    All,
}

/// lowering の結果．
#[derive(Debug)]
pub struct Lowered {
    pub module: Module,
    /// `LowerInput::tests` の各テスト関数の IR の関数．同じ順．`Roots::Tests` のときだけ埋める．
    pub tests: Vec<FnId>,
}

/// lowering の入力．
pub struct LowerInput<'a> {
    pub hir: &'a Program,
    pub types: &'a ProgramTypes,
    /// モジュール名を引く（IR の名前に付ける）．
    pub modules: &'a ModuleMap,
    /// モジュールのソース．assert（13.2）の失敗に出す式の字面を切り出す．
    pub sources: ArenaMap<ModuleId, &'a str>,
    /// エントリのモジュール．その定義には IR の名前にモジュール名を付けない．
    pub entry: ModuleId,
    /// `@test` の fn（13.1）．通常のビルドでは export しない．通常のコードから参照しないことは
    /// 呼び出し側が確かめてある（E0227）ので，通常のビルドではたどれず，取り除かれる．
    pub tests: Vec<DefId>,
    /// モジュールのソースのファイルのパス．`@external` のファイルを指すモジュール（`"./ffi.mjs"`）を
    /// 宣言を含むソースの隣のファイルに直すのに使う（12.1）．ないモジュールは直さない．
    pub paths: ArenaMap<ModuleId, &'a Path>,
    /// 同梱の JS のファイル（`lib/` からの相対パスと中身．`crate::core_js_files`）．core の
    /// モジュールのファイルの指定はソースの隣でなくここから引き，出力先の
    /// [`crate::CORE_JS_DIR`] の下に書き出すものとして `Module::js_files` に入れる．
    pub core_js_files: &'a [(&'a str, &'a str)],
    /// ビルド対象のターゲット（`"js"`）．`Some` なら，エントリ（`Roots::Exported` では main，
    /// `Roots::Tests` ではテスト関数）からたどれる外部関数にそのターゲットの `@external` が
    /// あることを確かめる（12.1）．check と LSP はターゲットを決めないので `None`．
    pub target: Option<&'a str>,
}

/// プログラム全体を Core IR にする．IR にできない箇所は全部集めて `Err` で返す．
pub fn lower(input: &LowerInput<'_>, roots: Roots) -> Result<Lowered, Vec<LowerError>> {
    let hir = input.hir;
    let mut cx = Lowerer {
        hir,
        types: input.types,
        modules: input.modules,
        sources: &input.sources,
        entry: input.entry,
        tests: &input.tests,
        pat_guards: Vec::new(),
        true_def: hir.prelude_def("True"),
        false_def: hir.prelude_def("False"),
        module: Module::new(),
        fns: ArenaMap::default(),
        worklist: Vec::new(),
        enums: ArenaMap::default(),
        ctors: ArenaMap::default(),
        locals: ArenaMap::default(),
        effects: ArenaMap::default(),
        ev: None,
        bound: Vec::new(),
        paths: &input.paths,
        core_js_files: input.core_js_files,
        uses_core_js: false,
        errors: Vec::new(),
    };
    cx.declare_enums();
    cx.define_ctors();
    for (_, items) in hir.modules.iter() {
        for &def in &items.items {
            let is_root = match (roots, hir.item(def)) {
                (Roots::Exported, Some(Item::Fn(_))) => cx.is_exported(def),
                (Roots::All, Some(Item::Fn(f))) => f.body.is_some() || f.is_external,
                (Roots::All, Some(Item::Const(_) | Item::Handler(_))) => true,
                _ => false,
            };
            if is_root {
                cx.fn_id(def);
            }
        }
    }
    let tests = match roots {
        Roots::Tests => input.tests.iter().map(|&def| cx.fn_id(def)).collect(),
        _ => Vec::new(),
    };
    while let Some(def) = cx.worklist.pop() {
        cx.define_fn(def);
    }
    if let Some(target) = input.target {
        let entries: Vec<FnId> = match roots {
            Roots::Exported => cx.entry_main().into_iter().collect(),
            Roots::Tests => tests.clone(),
            Roots::All => Vec::new(),
        };
        cx.check_target(&entries, target);
    }
    if cx.uses_core_js {
        // 同梱のファイルが相対で import し合っても動くよう，1つでも使えば全部を並べて書き出す．
        cx.module.js_files = input
            .core_js_files
            .iter()
            .map(|(path, contents)| JsFile {
                path: (*path).to_owned(),
                contents: (*contents).to_owned(),
            })
            .collect();
    }
    if cx.errors.is_empty() {
        Ok(Lowered {
            module: cx.module,
            tests,
        })
    } else {
        Err(cx.errors)
    }
}

/// 変換の途中の状態．
struct Lowerer<'a> {
    hir: &'a Program,
    types: &'a ProgramTypes,
    modules: &'a ModuleMap,
    sources: &'a ArenaMap<ModuleId, &'a str>,
    entry: ModuleId,
    tests: &'a [DefId],
    /// パターンを写す間に const のパターンから集めた比較（`x == LIMIT()`）．腕のガードの先頭に置く．
    pat_guards: Vec<Expr>,
    /// Prelude の `True` と `False`．IR では `Lit::Bool` にする．
    true_def: Option<DefId>,
    false_def: Option<DefId>,
    /// 作りかけの IR．
    module: Module,
    /// HIR の fn と const → IR の関数．const は引数のない関数にする．[`Lowerer::fn_id`] だけが足す．
    fns: ArenaMap<DefId, FnId>,
    /// ID を割り振ったが本体をまだ写していない fn．
    worklist: Vec<DefId>,
    /// HIR の type / enum / error → IR の enum．
    enums: ArenaMap<DefId, EnumId>,
    /// HIR の構成子 → IR の構成子．error は宣言そのものが構成子．Bool の構成子は持たない．
    ctors: ArenaMap<DefId, CtorRef>,
    /// HIR の局所変数 → IR の局所変数．[`Lowerer::local`] だけが足す．
    locals: ArenaMap<LocalId, Local>,
    /// HIR の effect → IR のエフェクト．[`Lowerer::effect_id`] だけが足す．
    effects: ArenaMap<DefId, EffectId>,
    /// 今の証拠の表（evidence passing，16.5）．R が空でない関数とラムダの最後の引数か，
    /// with で足した表．関数の外や R が空の関数では `None`．
    ev: Option<Local>,
    /// 囲む with が供給したエフェクトとハンドラの値．外側が先．同じ関数（トップレベルの fn か
    /// handler の操作）の中の with だけで，ラムダの中にも引き継ぐ．`use X` は後ろから探す（8.3）．
    bound: Vec<(DefId, Local)>,
    paths: &'a ArenaMap<ModuleId, &'a Path>,
    core_js_files: &'a [(&'a str, &'a str)],
    /// 写した外部関数が同梱の JS のファイルを指したか．
    uses_core_js: bool,
    errors: Vec<LowerError>,
}

/// ブロックの文の束縛．
enum Bind {
    /// `let var = value`．
    Var(Local, Expr),
    /// 腕1つの match．
    Pat(Pat, Expr),
}

/// 呼び出しの呼ばれる側．
enum Target {
    /// `fails` は呼ばれる側が失敗しうるか（`Expr::Call::fails`）．
    /// `Option<Expr>` は最後の引数に足す証拠の表（R が空でない関数を呼ぶとき）．
    Direct(FnId, bool, Option<Expr>),
    Builtin {
        op: Builtin,
        ty_args: Vec<Type>,
    },
    Ctor(CtorRef),
    /// 関数の値．評価済みで，副作用なく何度でも参照できる式（変数か関数）．
    /// `bool` は呼ばれる側が失敗しうるか．`Option<Expr>` は `Direct` と同じ．
    Value(Expr, bool, Option<Expr>),
    /// 能力の値の操作（`log.info`）．`handler` は能力の値の式．
    Op {
        handler: Expr,
        effect: EffectId,
        op: usize,
        fails: bool,
    },
}

impl Target {
    /// 位置の順に並んだ引数で呼ぶ式．
    fn apply(self, args: Vec<Expr>) -> Expr {
        match self {
            Target::Direct(f, fails, ev) => Expr::Call {
                callee: Callee::Direct(f),
                args: args.into_iter().chain(ev).collect(),
                fails,
                suspends: false,
            },
            Target::Builtin { op, ty_args } => Expr::Builtin { op, ty_args, args },
            Target::Ctor(ctor) => Expr::Ctor { ctor, args },
            Target::Value(f, fails, ev) => Expr::Call {
                callee: Callee::Indirect(Box::new(f)),
                args: args.into_iter().chain(ev).collect(),
                fails,
                suspends: false,
            },
            Target::Op {
                handler,
                effect,
                op,
                fails,
            } => Expr::Call {
                callee: Callee::Op {
                    handler: Box::new(handler),
                    effect,
                    op,
                },
                args,
                fails,
                suspends: false,
            },
        }
    }
}

impl<'a> Lowerer<'a> {
    /// 全部の type / enum / error に ID と名前を割り振る．構成子はまだ入れない．
    fn declare_enums(&mut self) {
        let bool_def = self.hir.prelude_def("Bool");
        let option_def = self.hir.prelude_def("Option");
        for (_, items) in self.hir.modules.iter() {
            for &def in &items.items {
                match self.hir.item(def) {
                    // Bool は IR では組み込みの型（`Type::Bool`）．
                    Some(Item::Type(_) | Item::Enum(_)) if Some(def) == bool_def => {}
                    // Prelude の Option は組み込み関数と同じ定義を指す（`Module::option_enum`）．
                    Some(Item::Enum(_)) if Some(def) == option_def => {
                        let id = self.module.option_enum();
                        self.enums.insert(def, id);
                    }
                    Some(Item::Type(t)) => {
                        let id = self.declare_enum(def, t.type_params.len());
                        self.enums.insert(def, id);
                    }
                    Some(Item::Enum(e)) => {
                        let id = self.declare_enum(def, e.type_params.len());
                        self.enums.insert(def, id);
                    }
                    // error は type と同じく同じ名前の型と構成子を定義する（7.1）．型引数は持たない．
                    Some(Item::Error(_)) => {
                        let id = self.declare_enum(def, 0);
                        self.module.enums[id].error = true;
                        self.enums.insert(def, id);
                    }
                    _ => {}
                }
            }
        }
    }

    /// enum に構成子を入れ，構成子の表（`ctors`）を作る．
    ///
    /// 構成子のフィールドの型は，型検査の構成子の宣言の型から引く．構成子の型引数は
    /// 作る型の型引数と同じ順なので，`Ty::Bound(i)` がそのまま `Type::Param(i)` になる．
    fn define_ctors(&mut self) {
        // `self.enums` を借りたまま `self.module` を書き換えられないので，先に組を写しておく．
        let enums: Vec<(DefId, EnumId)> = self.enums.iter().map(|(d, &e)| (d, e)).collect();
        for (def, enum_id) in enums {
            let ctors = match self.hir.item(def) {
                Some(Item::Type(t)) => t.ctor.into_iter().collect(),
                Some(Item::Enum(e)) => e.variants.clone(),
                Some(Item::Error(_)) => vec![def],
                _ => Vec::new(),
            };
            for (variant, ctor) in ctors.into_iter().enumerate() {
                self.ctors.insert(ctor, CtorRef { enum_id, variant });
                // Option の構成子は `EnumDef::option` が入れてある．
                if Some(enum_id) != self.module.option {
                    let v = self.variant_def(ctor);
                    self.module.enums[enum_id].variants.push(v);
                }
            }
        }
    }

    fn variant_def(&self, ctor: DefId) -> VariantDef {
        let fields = match self.hir.item(ctor) {
            Some(Item::Ctor(c)) => &c.fields,
            Some(Item::Error(e)) => &e.fields,
            _ => &Fields::None,
        };
        let fields = match fields {
            Fields::None => VariantFields::Unit,
            Fields::Positional(tys) => VariantFields::Positional(tys.len()),
            Fields::Named(fs) => {
                VariantFields::Named(fs.iter().map(|f| f.name.to_string()).collect())
            }
        };
        // フィールドのない構成子の型は作る型そのもので，関数型でない．
        let tys = match self.types.sigs.get(ctor).map(|sig| &sig.scheme.ty) {
            Some(Ty::Fn(f)) => f.params.iter().map(|t| self.ty(t)).collect(),
            _ => Vec::new(),
        };
        VariantDef {
            name: self.hir.defs[ctor].name.to_string(),
            fields,
            tys,
        }
    }

    /// 型検査の型を IR の型にする．
    fn ty(&self, ty: &Ty) -> Type {
        match ty {
            Ty::Prim(p) => match p {
                Prim::Int => Type::Int,
                Prim::Int64 => Type::Int64,
                Prim::Float => Type::Float,
                Prim::Bool => Type::Bool,
                Prim::String => Type::String,
                Prim::Unit => Type::Unit,
            },
            Ty::Never => Type::Never,
            // 決まらずに残った型変数（使われない `None` の `Option[?]` など）．どの表現でも
            // 振る舞いは変わらない．`Ty::Error` は型の誤りがあるときだけ現れ，そのときは
            // lowering を呼ばない．
            Ty::Var(_) | Ty::Error => Type::Unit,
            // 名前付き型の集合の型引数（`Handler[E, R]`）は実行時の表現を持たない．型引数の
            // 位置をそろえるために `()` を置く．
            Ty::Set(..) => Type::Unit,
            // 能力の値はハンドラの値（`Expr::Handler`）．表示も比較もできない（8.2）ので，
            // 型は表示関数を作るのに使われない．
            Ty::Cap(_) => Type::Fn,
            Ty::Bound(i) => Type::Param(*i as usize),
            Ty::Fn(_) => Type::Fn,
            Ty::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.ty(t)).collect()),
            Ty::List(t) => Type::list(self.ty(t)),
            Ty::Named(con, args) => {
                let def = self.types.ty_table.def(*con);
                Type::Enum(self.enums[def], args.iter().map(|t| self.ty(t)).collect())
            }
        }
    }

    /// fn の IR の ID．まだなければ割り振って作業リストに積む．fn の ID はここでしか作らない．
    fn fn_id(&mut self, def: DefId) -> FnId {
        if let Some(&id) = self.fns.get(def) {
            return id;
        }
        let name = self.ir_name(def);
        let id = self.module.declare(&name, self.is_exported(def));
        self.fns.insert(def, id);
        self.worklist.push(def);
        id
    }

    /// HIR の局所変数に対応する IR の局所変数．まだなければ作る．
    fn local(&mut self, id: LocalId) -> Local {
        if let Some(&l) = self.locals.get(id) {
            return l;
        }
        let l = self.module.local(&self.hir.locals[id].name);
        self.locals.insert(id, l);
        l
    }

    fn define_fn(&mut self, def: DefId) {
        let hir = self.hir;
        let id = self.fns[def];
        // 関数ごとに証拠の表と with の束縛を始め直す．
        self.ev = None;
        self.bound.clear();
        let (mut params, body) = match hir.item(def) {
            Some(Item::Fn(f)) if f.body.is_none() => {
                // `@external suspend fn` は Promise を返す関数に写す（12.2）．次の段で扱う．
                if f.sig.is_suspend
                    && let Some(span) = hir.defs[def].span
                {
                    self.report("suspend external functions", span);
                }
                let params = f.sig.params.iter().map(|p| self.local(p.local)).collect();
                let external = self.external(def, f);
                self.module.define_external(id, params, external);
                return;
            }
            Some(Item::Fn(f)) => (
                f.sig.params.iter().map(|p| self.local(p.local)).collect(),
                f.body.expect("本体のある fn"),
            ),
            // const の値は純粋な式（5.6）なので，参照のたびに呼んでも結果は同じ．
            Some(Item::Const(c)) => (Vec::new(), c.value),
            Some(Item::Handler(_)) => {
                let (params, body) = self.handler(def);
                self.module.define(id, params, body);
                return;
            }
            _ => unreachable!("fn_id に fn，const，handler でない定義が来た"),
        };
        if self.takes_ev(def) {
            let ev = self.module.local("ev");
            params.push(ev);
            self.ev = Some(ev);
        }
        let body = self.expr(body);
        self.module.define(id, params, body);
    }

    /// fn が証拠の表を最後の引数に受けるか．宣言の型（汎化した型）の R が空でなければ受ける．
    /// 中身の分からない集合（`R(f)`，汎化した集合）も空でないので受ける（引数の関数へ渡すため）．
    fn takes_ev(&self, def: DefId) -> bool {
        matches!(self.types.sigs.get(def).map(|s| &s.scheme.ty), Some(Ty::Fn(f)) if !f.uses.is_empty())
    }

    /// 関数型の値を呼ぶときに証拠の表を渡すか．値の型の R が空でなければ渡す．
    fn ty_takes_ev(ty: &Ty) -> bool {
        matches!(ty, Ty::Fn(f) if !f.uses.is_empty())
    }

    /// 呼び出しの最後の引数に渡す今の証拠の表．表がなければ `()`（呼ばれる側は使わない．
    /// 型検査が R を確かめてあるので，表の要る効果を持つ呼び出しは表のある所にしかない）．
    fn ev_arg(&self) -> Expr {
        match self.ev {
            Some(ev) => Expr::Var(ev),
            None => Expr::Lit(Lit::Unit),
        }
    }

    /// エフェクトの IR の ID．まだなければ作る．
    fn effect_id(&mut self, def: DefId) -> EffectId {
        if let Some(&id) = self.effects.get(def) {
            return id;
        }
        let ops = match self.hir.item(def) {
            Some(Item::Effect(e)) => e
                .ops
                .iter()
                .map(|&op| OpDef {
                    name: self.hir.defs[op].name.to_string(),
                    suspend: matches!(self.hir.item(op), Some(Item::Op(o)) if o.sig.is_suspend),
                })
                .collect(),
            _ => Vec::new(),
        };
        let name = self.ir_name(def);
        let id = self.module.add_effect(EffectDef { name, ops });
        self.effects.insert(def, id);
        id
    }

    /// エフェクトの操作の添字（`EffectDef::ops`）．
    fn op_index(&self, effect: DefId, op: DefId) -> Option<usize> {
        match self.hir.item(effect) {
            Some(Item::Effect(e)) => e.ops.iter().position(|&o| o == op),
            _ => None,
        }
    }

    /// 外部関数の指定．ファイルを指すモジュールは宣言を含むソースの隣のファイルに直す（12.1）．
    /// 同梱の core の JS の指定は，同梱の JS のファイルを出力先に書き出した場所に直す．
    fn external(&mut self, def: DefId, f: &emela_resolve::hir::FnItem) -> External {
        let module = match &self.hir.defs[def].module {
            DefModule::Source(m) => Some(*m),
            _ => None,
        };
        let is_core = module.is_some_and(|m| self.modules.is_core(m));
        let dir = module
            .and_then(|m| self.paths.get(m))
            .and_then(|p| p.parent());
        let mut bindings = Vec::new();
        for e in &f.externals {
            let mut args = e.args.clone();
            if args.len() == 2 && (args[0].starts_with("./") || args[0].starts_with("../")) {
                if is_core {
                    if e.target == "js" {
                        args[0] = self.core_js_file(&args[0], self.hir.defs[def].span);
                    }
                } else if let Some(dir) = dir {
                    args[0] = normalize(&dir.join(&args[0]));
                }
            }
            bindings.push(ExternalBinding {
                target: e.target.to_string(),
                args,
            });
        }
        let (params, ret) = match self.types.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => (f.params.iter().map(boundary).collect(), boundary(&f.ret)),
            _ => (Vec::new(), Boundary::Plain),
        };
        External {
            bindings,
            params,
            ret,
        }
    }

    /// 同梱の core の JS のファイルの指定（`./x.mjs`）を，出力先の [`crate::CORE_JS_DIR`] に
    /// 書き出したファイルへの，エントリの出力からの相対パス（`./emela_core/x.mjs`）にする．
    /// core のソースは `lib/` の直下にあるとみなし，`lib/` からの相対パスで同梱のファイルを引く．
    fn core_js_file(&mut self, spec: &str, span: Option<Span>) -> String {
        let found =
            lib_relative(spec).filter(|path| self.core_js_files.iter().any(|(p, _)| p == path));
        match found {
            Some(path) => {
                self.uses_core_js = true;
                format!("./{}/{path}", crate::CORE_JS_DIR)
            }
            None => {
                self.errors.push(LowerError::MissingCoreFile {
                    path: spec.to_owned(),
                    span,
                });
                spec.to_owned()
            }
        }
    }

    /// 状態を持たないハンドラを，ハンドラの値を返す関数にする．依存（操作の本体の R，8.4）が
    /// あれば，with の位置の証拠の表を引数に受ける．操作は宣言の順のラムダ．
    fn handler(&mut self, def: DefId) -> (Vec<Local>, Expr) {
        let hir = self.hir;
        let Some(Item::Handler(h)) = hir.item(def) else {
            unreachable!("handler の定義");
        };
        let span = hir.defs[def].span;
        let stateful = !h.fields.is_empty() || h.init.is_some() || h.release.is_some();
        let Res::Def(effect) = h.effect else {
            return (
                Vec::new(),
                self.unsupported_at("handler of an unknown effect", span),
            );
        };
        if stateful {
            return (Vec::new(), self.unsupported_at("handlers with state", span));
        }
        let effect_id = self.effect_id(effect);
        let mut params = Vec::new();
        if self
            .types
            .handler_uses
            .get(def)
            .is_some_and(|r| !r.is_empty())
        {
            let ev = self.module.local("ev");
            params.push(ev);
            self.ev = Some(ev);
        }
        let n = self.module.effects[effect_id].ops.len();
        let mut ops: Vec<Option<Expr>> = vec![None; n];
        for op in &h.ops {
            let index = match op.op {
                Res::Def(o) => self.op_index(effect, o),
                _ => None,
            };
            let (Some(index), Some(body)) = (index, op.body) else {
                continue;
            };
            if op.is_suspend {
                self.report("suspend operations", hir.exprs[body].span);
                continue;
            }
            let ps = op.params.iter().map(|p| self.local(p.local)).collect();
            // 操作の本体の失敗は戻り値として操作の呼び出し元へ返る（ラムダと同じ）．
            ops[index] = Some(Expr::Lambda {
                params: ps,
                body: Box::new(self.expr(body)),
                suspends: false,
            });
        }
        if ops.iter().any(Option::is_none) {
            // 足りない操作は型検査が E0404 を出している．
            return (
                params,
                self.unsupported_at("handler with missing operations", span),
            );
        }
        let ops = ops.into_iter().map(Option::unwrap).collect();
        (
            params,
            Expr::Handler {
                name: hir.defs[def].name.to_string(),
                effect: effect_id,
                ops,
            },
        )
    }

    /// `with H1, ..., Hn { body }`（8.3，14.2）．
    ///
    /// ハンドラの値を依存の順に作り，今の証拠の表に供給するエフェクトを足した表で本体を評価する．
    /// 同じエフェクトは後ろのハンドラが優先．ハンドラの依存は，同じ with の別のエフェクトを供給する
    /// ハンドラ（同じエフェクトが複数なら後ろのもの）が満たし，残りと自分のエフェクトへの依存は
    /// 外側の表が満たす．依存の循環は型検査が E0411 にしてある．
    fn with(&mut self, handlers: &[(Res, Span)], body: ExprId) -> Expr {
        let hir = self.hir;
        // （handler，供給するエフェクト）．
        let mut hs: Vec<(DefId, DefId)> = Vec::new();
        for &(res, span) in handlers {
            let Res::Def(def) = res else {
                return self.unsupported("unresolved handler", span);
            };
            match hir.item(def) {
                Some(Item::Handler(h)) => {
                    if !h.fields.is_empty() || h.init.is_some() || h.release.is_some() {
                        return self.unsupported("handlers with state", span);
                    }
                    if h.ops.iter().any(|op| op.is_suspend) {
                        return self.unsupported("suspend operations", span);
                    }
                    let Res::Def(effect) = h.effect else {
                        return self.unsupported("handler of an unknown effect", span);
                    };
                    hs.push((def, effect));
                }
                Some(Item::Layer(_)) => return self.unsupported("layer", span),
                _ => return self.unsupported("this kind of handler", span),
            }
        }
        // 各ハンドラの依存のうち，同じ with の別のハンドラが満たすもの（エフェクト，添字）．
        let deps: Vec<Vec<(DefId, usize)>> = hs
            .iter()
            .map(|&(h, own)| {
                let row = self.types.handler_uses.get(h).cloned().unwrap_or_default();
                row.tags()
                    .filter(|&y| y != own)
                    .filter_map(|y| hs.iter().rposition(|&(_, e)| e == y).map(|j| (y, j)))
                    .collect()
            })
            .collect();
        // 依存が先に来る順．
        let mut order = Vec::new();
        let mut state = vec![0u8; hs.len()];
        fn visit(i: usize, deps: &[Vec<(DefId, usize)>], state: &mut [u8], order: &mut Vec<usize>) {
            if state[i] != 0 {
                return;
            }
            state[i] = 1;
            for &(_, j) in &deps[i] {
                visit(j, deps, state, order);
            }
            state[i] = 2;
            order.push(i);
        }
        for i in 0..hs.len() {
            visit(i, &deps, &mut state, &mut order);
        }
        let outer = self.ev;
        let mut locals: Vec<Option<Local>> = vec![None; hs.len()];
        let mut binds = Vec::new();
        for i in order {
            let (h, _) = hs[i];
            let f = self.fn_id(h);
            let args = if self
                .types
                .handler_uses
                .get(h)
                .is_some_and(|r| !r.is_empty())
            {
                let bindings: Vec<(EffectId, Expr)> = deps[i]
                    .iter()
                    .map(|&(y, j)| {
                        let l = locals[j].expect("依存のハンドラを先に作ってある");
                        (self.effect_id(y), Expr::Var(l))
                    })
                    .collect();
                let ctx = if bindings.is_empty() {
                    self.ev_arg()
                } else {
                    Expr::Provide {
                        base: outer.map(|l| Box::new(Expr::Var(l))),
                        bindings,
                    }
                };
                vec![ctx]
            } else {
                Vec::new()
            };
            let l = self.module.local("h");
            locals[i] = Some(l);
            binds.push((
                l,
                Expr::Call {
                    callee: Callee::Direct(f),
                    args,
                    fails: false,
                    suspends: false,
                },
            ));
        }
        let ctx = self.module.local("ev");
        let depth = self.bound.len();
        for (&(_, effect), l) in hs.iter().zip(&locals) {
            self.bound.push((effect, l.unwrap()));
        }
        self.ev = Some(ctx);
        let body = self.expr(body);
        self.ev = outer;
        self.bound.truncate(depth);
        // 本体が表を使わなければ（`use` が with のハンドラを直接指すだけなら）作らない．
        if mentions(&body, ctx) {
            let bindings = hs
                .iter()
                .zip(&locals)
                .map(|(&(_, effect), l)| (self.effect_id(effect), Expr::Var(l.unwrap())))
                .collect();
            binds.push((
                ctx,
                Expr::Provide {
                    base: outer.map(|l| Box::new(Expr::Var(l))),
                    bindings,
                },
            ));
        }
        Expr::With {
            binds,
            body: Box::new(body),
        }
    }

    /// `use X`（8.2，8.3）．同じ関数の中で X を供給する最も内側の with のハンドラ．なければ
    /// 呼び出し元から渡された証拠の表から引く．
    fn use_(&mut self, effect: Res, span: Span) -> Expr {
        let Res::Def(effect) = effect else {
            return self.unsupported("unresolved effect", span);
        };
        if let Some(&(_, h)) = self.bound.iter().rev().find(|(e, _)| *e == effect) {
            return Expr::Var(h);
        }
        let Some(ev) = self.ev else {
            // R を確かめた型検査を通っていれば来ない．
            return self.unsupported("`use` without a provided effect", span);
        };
        Expr::Use {
            ctx: Box::new(Expr::Var(ev)),
            effect: self.effect_id(effect),
        }
    }

    /// ラムダ．型の R が空でなければ，証拠の表を最後の引数に受ける．囲む with の束縛は，受けた表に
    /// 足して本体の表にする（`use X` は囲む with を先に見る，8.3）．
    fn lambda(&mut self, id: ExprId, params: &[emela_resolve::hir::Param], body: ExprId) -> Expr {
        let mut ps: Vec<Local> = params.iter().map(|p| self.local(p.local)).collect();
        if !Self::ty_takes_ev(self.expr_ty(id)) {
            return Expr::Lambda {
                params: ps,
                body: Box::new(self.expr(body)),
                suspends: false,
            };
        }
        let outer = self.ev;
        let p = self.module.local("ev");
        ps.push(p);
        let (ctx, prelude) = if self.bound.is_empty() {
            (p, None)
        } else {
            let c = self.module.local("ev");
            let bindings = self
                .bound
                .clone()
                .into_iter()
                .map(|(e, h)| (self.effect_id(e), Expr::Var(h)))
                .collect();
            (
                c,
                Some(Expr::Provide {
                    base: Some(Box::new(Expr::Var(p))),
                    bindings,
                }),
            )
        };
        self.ev = Some(ctx);
        let mut body = self.expr(body);
        self.ev = outer;
        if let Some(value) = prelude.filter(|_| mentions(&body, ctx)) {
            body = Expr::Let {
                var: ctx,
                value: Box::new(value),
                body: Box::new(body),
            };
        }
        Expr::Lambda {
            params: ps,
            body: Box::new(body),
            suspends: false,
        }
    }

    /// 能力の値の操作を，呼ばずに値として使う（`List.each(xs, log.info)`）．それを呼ぶだけの
    /// ラムダにする．能力の値は参照の位置で評価する．
    fn op_value(&mut self, id: ExprId, base: ExprId, op: DefId, span: Span) -> Expr {
        let Some(Ty::Cap(effect)) = self.types.exprs.get(base).cloned() else {
            return self.unsupported("effect operation", span);
        };
        let Some(index) = self.op_index(effect, op) else {
            return self.unsupported("effect operation", span);
        };
        let effect_id = self.effect_id(effect);
        let fails = self.callee_fails(id);
        let (arity, ev) = match self.expr_ty(id) {
            Ty::Fn(f) => (f.params.len(), !f.uses.is_empty()),
            _ => unreachable!("操作の参照の型は関数型"),
        };
        let handler = self.expr(base);
        let h = self.module.local("h");
        let mut params: Vec<Local> = (0..arity).map(|_| self.module.local("x")).collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        if ev {
            params.push(self.module.local("ev"));
        }
        Expr::Let {
            var: h,
            value: Box::new(handler),
            body: Box::new(Expr::Lambda {
                params,
                body: Box::new(Expr::Call {
                    callee: Callee::Op {
                        handler: Box::new(Expr::Var(h)),
                        effect: effect_id,
                        op: index,
                    },
                    args,
                    fails,
                    suspends: false,
                }),
                suspends: false,
            }),
        }
    }

    /// エントリのモジュールの `main` の IR の関数．
    fn entry_main(&self) -> Option<FnId> {
        let items = self.hir.modules.get(self.entry)?;
        items.items.iter().find_map(|&def| {
            let data = &self.hir.defs[def];
            (data.name == "main" && data.kind == DefKind::Fn)
                .then(|| self.fns.get(def).copied())
                .flatten()
        })
    }

    /// `entries` からたどれる外部関数のうち，`target` の `@external` がないものを誤りにする（12.1）．
    /// with に書いたハンドラの操作の本体から呼ぶ関数もたどる（ハンドラの値を作る関数を呼ぶので）．
    fn check_target(&mut self, entries: &[FnId], target: &str) {
        let mut seen = vec![false; self.module.functions.len()];
        let mut stack: Vec<FnId> = entries.to_vec();
        let mut missing = Vec::new();
        while let Some(f) = stack.pop() {
            let i = f.into_raw().into_u32() as usize;
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            let func = &self.module.functions[f];
            if let Some(ext) = &func.external {
                if ext.binding(target).is_none() {
                    missing.push(f);
                }
                continue;
            }
            collect_fn_refs(&func.body, &mut stack);
        }
        let mut defs: Vec<DefId> = missing
            .into_iter()
            .map(|f| {
                self.fns
                    .iter()
                    .find(|(_, id)| **id == f)
                    .map(|(d, _)| d)
                    .expect("IR の関数は HIR の定義から作る")
            })
            .collect();
        // 宣言の位置の順に出す．
        defs.sort_by_key(|&d| {
            self.hir.defs[d]
                .span
                .map(|s| (s.module.into_raw().into_u32(), s.range.start()))
        });
        for def in defs {
            self.errors.push(LowerError::MissingExternal {
                name: self.hir.defs[def].name.to_string(),
                target: target.to_owned(),
                span: self.hir.defs[def].span,
            });
        }
    }

    /// まだ扱えない式．診断を積み，代わりに `()` を返して変換を続ける．
    fn unsupported(&mut self, what: &str, span: Span) -> Expr {
        self.report(what, span);
        Expr::Lit(Lit::Unit)
    }

    /// 位置のないこともある [`Lowerer::unsupported`]．
    fn unsupported_at(&mut self, what: &str, span: Option<Span>) -> Expr {
        self.errors.push(LowerError::Unsupported {
            what: what.to_owned(),
            span,
        });
        Expr::Lit(Lit::Unit)
    }

    fn report(&mut self, what: &str, span: Span) {
        self.errors.push(LowerError::Unsupported {
            what: what.to_owned(),
            span: Some(span),
        });
    }

    /// 式の型．型検査は全部の式に型を付けている．
    fn expr_ty(&self, id: ExprId) -> &Ty {
        &self.types.exprs[id]
    }

    fn expr(&mut self, id: ExprId) -> Expr {
        // `self.hir` は `&'a Program` なので，写しておけば `self` を借りずに HIR を読める．
        let hir = self.hir;
        let e = &hir.exprs[id];
        match &e.kind {
            ExprKind::Lit(lit) => self.lit(id, lit),
            ExprKind::Path(res) => self.path(id, *res, e.span),
            ExprKind::Call { callee, args } => {
                let written = args.iter().map(|a| a.value).collect();
                self.call(id, *callee, written)
            }
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                // `x |> f(a)` は `f(x, a)`，`x |> f` は `f(x)`（6.7）．
                let (callee, mut written) = match &hir.exprs[*rhs].kind {
                    ExprKind::Call { callee, args } => {
                        (*callee, args.iter().map(|a| a.value).collect())
                    }
                    _ => (*rhs, Vec::new()),
                };
                written.insert(0, *lhs);
                self.call(id, callee, written)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, *lhs, *rhs, e.span),
            ExprKind::Unary { op, operand } => {
                let (op, ty) = match op {
                    UnaryOp::Neg => (UnOp::Neg, self.op_ty(*operand)),
                    UnaryOp::Not => (UnOp::Not, Some(OpTy::Bool)),
                };
                let Some(ty) = ty else {
                    return self.unsupported("negation of this type", e.span);
                };
                Expr::Unary {
                    op,
                    ty,
                    operand: Box::new(self.expr(*operand)),
                }
            }
            ExprKind::Unit => Expr::Lit(Lit::Unit),
            ExprKind::Tuple(es) => Expr::Tuple(es.iter().map(|&e| self.expr(e)).collect()),
            ExprKind::List { elems, rest } => Expr::List {
                elems: elems.iter().map(|&e| self.expr(e)).collect(),
                tail: rest.map(|r| Box::new(self.expr(r))),
            },
            ExprKind::Block(block) => self.block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => Expr::If {
                cond: Box::new(self.expr(*cond)),
                then: Box::new(self.expr(*then_branch)),
                else_: Box::new(match else_branch {
                    Some(b) => self.expr(*b),
                    None => Expr::Lit(Lit::Unit),
                }),
            },
            ExprKind::Missing => self.unsupported("missing expression", e.span),
            ExprKind::Interpolated(parts) => self.interpolated(parts),
            ExprKind::Field { base, .. } => self.field(id, *base, e.span),
            ExprKind::Match { scrutinee, arms } => Expr::Match {
                scrutinee: Box::new(self.expr(*scrutinee)),
                arms: arms.iter().map(|arm| self.arm(arm)).collect(),
            },
            ExprKind::Lambda { params, body } => self.lambda(id, params, *body),
            ExprKind::Assert(cond) => self.assert(*cond),
            ExprKind::Update { ctor, fields, base } => {
                self.update(id, *ctor, fields, *base, e.span)
            }
            ExprKind::Escape { expr, arms } => Expr::Escape {
                body: Box::new(self.expr(*expr)),
                arms: arms.iter().map(|arm| self.arm(arm)).collect(),
            },
            ExprKind::Fail(inner) => Expr::Fail(Box::new(self.expr(*inner))),
            ExprKind::Use { effect } => self.use_(*effect, e.span),
            ExprKind::With { handlers, body } => self.with(handlers, *body),
        }
    }

    fn lit(&mut self, id: ExprId, lit: &Literal) -> Expr {
        Expr::Lit(self.lit_value(self.expr_ty(id), lit))
    }

    /// リテラルの値．整数は型（Int か Int64）で幅を決める．範囲は型検査が確かめてある（E0311）．
    fn lit_value(&self, ty: &Ty, lit: &Literal) -> Lit {
        match lit {
            Literal::Int(text) => {
                let n = parse_int(text);
                if *ty == Ty::INT64 {
                    Lit::Int64(n as i64)
                } else {
                    Lit::Int(n as i32)
                }
            }
            Literal::Float(text) => Lit::Float(text.parse().expect("字句解析が確かめた小数")),
            Literal::String(s) => Lit::String(s.clone()),
        }
    }

    /// 呼び出しの外に出てくる名前の参照（値としての参照）．
    fn path(&mut self, id: ExprId, res: Res, span: Span) -> Expr {
        let def = match res {
            Res::Local(l) => return Expr::Var(self.local(l)),
            Res::Def(def) => def,
            Res::Err => return self.unsupported("unresolved name", span),
        };
        if Some(def) == self.true_def {
            return Expr::Lit(Lit::Bool(true));
        }
        if Some(def) == self.false_def {
            return Expr::Lit(Lit::Bool(false));
        }
        match self.hir.item(def) {
            Some(Item::Fn(f)) if f.body.is_some() => Expr::Fn(self.fn_id(def)),
            // 組み込み関数とフィールドのある構成子は IR の値でないので，ラムダで包む．
            Some(Item::Fn(f)) if f.is_intrinsic => self.wrap_in_lambda(id, span),
            Some(Item::Fn(_)) => Expr::Fn(self.fn_id(def)),
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) if self.module.variant(ctor).fields.is_empty() => Expr::Ctor {
                    ctor,
                    args: Vec::new(),
                },
                _ => self.wrap_in_lambda(id, span),
            },
            // const の値は失敗しない（5.6）．
            Some(Item::Const(_)) => Expr::Call {
                callee: Callee::Direct(self.fn_id(def)),
                args: Vec::new(),
                fails: false,
                suspends: false,
            },
            _ => self.unsupported("reference to this kind of definition", span),
        }
    }

    /// 呼び出し．`written` は書かれた順の引数（パイプなら左辺が先頭）．
    /// 型検査が並べ直した仮引数の順（`ProgramTypes::calls`）から各引数の位置を引く．
    fn call(&mut self, id: ExprId, callee: ExprId, written: Vec<ExprId>) -> Expr {
        let span = self.hir.exprs[id].span;
        let slots = &self.types.calls[id];
        let positions: Vec<usize> = written
            .iter()
            .map(|&arg| {
                slots
                    .iter()
                    .position(|s| *s == Some(arg))
                    .expect("型検査が引数を仮引数の位置に置いてある")
            })
            .collect();

        // 呼ばれる側を引数より先に決める（評価順は左から，14.1）．
        let target = match self.callee(callee, span) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let args: Vec<(usize, Expr)> = positions
            .into_iter()
            .zip(written)
            .map(|(pos, arg)| (pos, self.expr(arg)))
            .collect();
        let (target, binding) = match target {
            Target::Value(f, fails, ev) if !matches!(f, Expr::Var(_) | Expr::Fn(_)) => {
                // 引数を並べ替えて `let` で束縛すると呼ばれる側より先に評価されるので，
                // 呼ばれる側を先に束縛しておく．
                let l = self.module.local("f");
                (Target::Value(Expr::Var(l), fails, ev), Some((l, f)))
            }
            Target::Op {
                handler,
                effect,
                op,
                fails,
            } if !matches!(handler, Expr::Var(_) | Expr::Use { .. }) => {
                // 証拠の表を引くだけの式（`(use X).op()`）は副作用がないので束縛しない．
                let l = self.module.local("h");
                let t = Target::Op {
                    handler: Expr::Var(l),
                    effect,
                    op,
                    fails,
                };
                (t, Some((l, handler)))
            }
            t => (t, None),
        };
        let call = named::in_written_order(&mut self.module, args, |args| target.apply(args));
        match binding {
            Some((var, value)) => Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(call),
            },
            None => call,
        }
    }

    /// 組み込み関数か構成子の参照（`id`）を，それを呼ぶだけのラムダにする．
    /// `List.map(xs, Some)` は `List.map(xs, fn(x) { Some(x) })` になる．引数の数は参照の型から取る．
    fn wrap_in_lambda(&mut self, id: ExprId, span: Span) -> Expr {
        let target = match self.callee(id, span) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let arity = match self.expr_ty(id) {
            Ty::Fn(f) => f.params.len(),
            _ => unreachable!("呼べる定義の参照の型は関数型"),
        };
        let params: Vec<Local> = (0..arity).map(|_| self.module.local("x")).collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        Expr::Lambda {
            params,
            body: Box::new(target.apply(args)),
            suspends: false,
        }
    }

    /// 呼ばれる側．IR にできなければ診断を積んで `Err` に代わりの式を入れる．
    fn callee(&mut self, callee: ExprId, span: Span) -> Result<Target, Expr> {
        let hir = self.hir;
        let fails = self.callee_fails(callee);
        let ev = Self::ty_takes_ev(self.expr_ty(callee)).then(|| self.ev_arg());
        // 能力の値の操作（`log.info(...)`）．
        if let ExprKind::Field { base, .. } = &hir.exprs[callee].kind
            && let Some(&op) = self.types.ops.get(callee)
            && let Some(Ty::Cap(effect)) = self.types.exprs.get(*base).cloned()
            && let Some(index) = self.op_index(effect, op)
        {
            let effect = self.effect_id(effect);
            let handler = self.expr(*base);
            return Ok(Target::Op {
                handler,
                effect,
                op: index,
                fails,
            });
        }
        let ExprKind::Path(Res::Def(def)) = hir.exprs[callee].kind else {
            return Ok(Target::Value(self.expr(callee), fails, ev));
        };
        match hir.item(def) {
            Some(Item::Fn(f)) if f.body.is_some() => {
                let ev = self.takes_ev(def).then(|| self.ev_arg());
                Ok(Target::Direct(self.fn_id(def), fails, ev))
            }
            Some(Item::Fn(f)) if f.is_intrinsic => {
                let data = &hir.defs[def];
                let module = match &data.module {
                    DefModule::Source(m) => self.modules[*m].name.to_string(),
                    _ => String::new(),
                };
                let op = Builtin::lookup_intrinsic(&module, &data.name)
                    .expect("@intrinsic の宣言は組み込み関数の表と一致する");
                let ty_args = match self.types.type_args.get(callee) {
                    Some(tys) => tys.iter().map(|t| self.ty(t)).collect(),
                    None => Vec::new(),
                };
                Ok(Target::Builtin { op, ty_args })
            }
            // 外部関数は失敗せず（12.5），R も空（12.2）．
            Some(Item::Fn(_)) => Ok(Target::Direct(self.fn_id(def), false, None)),
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) => Ok(Target::Ctor(ctor)),
                None => Err(self.unsupported("this constructor", span)),
            },
            _ => Ok(Target::Value(self.expr(callee), fails, ev)),
        }
    }

    /// 呼ばれる側が失敗しうるか．呼ばれる側の式の関数型の E（型検査が解を入れたもの）が空でなければ
    /// 真．集合の型引数や汎化した集合変数のように中身の分からない集合も，空でないので真にする．
    /// 組み込み関数と構成子の型の E は空．
    fn callee_fails(&self, callee: ExprId) -> bool {
        matches!(self.expr_ty(callee), Ty::Fn(f) if !f.fails.is_empty())
    }

    fn binary(&mut self, op: BinaryOp, lhs: ExprId, rhs: ExprId, span: Span) -> Expr {
        let operand = self.op_ty(lhs);
        let (op, ty) = match op {
            BinaryOp::Add => (BinOp::Add, operand),
            BinaryOp::Sub => (BinOp::Sub, operand),
            BinaryOp::Mul => (BinOp::Mul, operand),
            BinaryOp::Div => (BinOp::Div, operand),
            BinaryOp::Rem => (BinOp::Rem, operand),
            // 基本型でない値の等値比較は構造で比べる．
            BinaryOp::Eq => (BinOp::Eq, operand.or(Some(OpTy::Structural))),
            BinaryOp::Ne => (BinOp::Ne, operand.or(Some(OpTy::Structural))),
            // 基本型でない値の大小比較（Ord）は Trait の段まで持たない．
            BinaryOp::Lt => (BinOp::Lt, operand),
            BinaryOp::Le => (BinOp::Le, operand),
            BinaryOp::Gt => (BinOp::Gt, operand),
            BinaryOp::Ge => (BinOp::Ge, operand),
            BinaryOp::And => (BinOp::And, Some(OpTy::Bool)),
            BinaryOp::Or => (BinOp::Or, Some(OpTy::Bool)),
            BinaryOp::Pipe => unreachable!("パイプは呼び出しとして写す"),
        };
        let Some(ty) = ty else {
            return self.unsupported("this operator on this type", span);
        };
        Expr::Binary {
            op,
            ty,
            lhs: Box::new(self.expr(lhs)),
            rhs: Box::new(self.expr(rhs)),
        }
    }

    /// 演算子のオペランドの型．基本型でなければ `None`．
    fn op_ty(&self, operand: ExprId) -> Option<OpTy> {
        prim_op_ty(self.expr_ty(operand))
    }

    /// `assert 式`（13.2）．式が比較なら両辺の値を表示できる形（`Assert::compare`）にする．
    /// 両辺の型が表示できない（型引数や関数を含む）ときは，値を出さない形（`Assert::bool`）に落とす．
    fn assert(&mut self, cond: ExprId) -> Expr {
        let hir = self.hir;
        let c = &hir.exprs[cond];
        let text = self.source_text(c.span);
        if let ExprKind::Binary { op, lhs, rhs } = &c.kind
            && let Some(bin) = comparison(*op)
        {
            let ty = self.ty(self.expr_ty(*lhs));
            let op_ty = match (self.op_ty(*lhs), bin) {
                (Some(t), _) => Some(t),
                (None, BinOp::Eq | BinOp::Ne) => Some(OpTy::Structural),
                (None, _) => None,
            };
            if let Some(op_ty) = op_ty
                && unshowable(&ty).is_none()
            {
                let (lhs, rhs) = (self.expr(*lhs), self.expr(*rhs));
                return Assert::compare(bin, op_ty, ty, lhs, rhs, text);
            }
        }
        let cond = self.expr(cond);
        Assert::bool(cond, text)
    }

    /// ソースの字面．ソースのない位置なら空．
    ///
    /// 戻り値は `self` でなくソース（`'a`）を借りる．`self` を借りたままにしないので，
    /// 字面を持ったまま `self.expr(..)`（`&mut self`）を呼べる．
    fn source_text(&self, span: Span) -> &'a str {
        let range = usize::from(span.range.start())..usize::from(span.range.end());
        let sources: &'a ArenaMap<ModuleId, &'a str> = self.sources;
        sources
            .get(span.module)
            .and_then(|src| src.get(range))
            .unwrap_or_default()
    }

    /// フィールドの参照．フィールドを持つのは構成子が1つの型（type，error）だけなので，
    /// 構成子はその型の唯一のもの．添字は型検査が引いてある（`ProgramTypes::fields`）．
    fn field(&mut self, id: ExprId, base: ExprId, span: Span) -> Expr {
        // 表にないのは能力の操作（`log.info`）．呼ばずに値として使っている．
        let Some(&index) = self.types.fields.get(id) else {
            return match self.types.ops.get(id) {
                Some(&op) => self.op_value(id, base, op, span),
                None => self.unsupported("effect operation", span),
            };
        };
        let Ty::Named(con, _) = self.expr_ty(base) else {
            unreachable!("フィールドを引けた値の型は名前付き型");
        };
        let ctor = CtorRef {
            enum_id: self.enums[self.types.ty_table.def(*con)],
            variant: 0,
        };
        Expr::Field {
            base: Box::new(self.expr(base)),
            ctor,
            index,
        }
    }

    /// 部分更新 `C(f: e, ..base)`（5.3）．書いたフィールドを書かれた順に評価し，`..base` を
    /// 最後に評価する（14.1）．書いていないフィールドは `base` のフィールドから引き継ぐ．
    fn update(
        &mut self,
        id: ExprId,
        ctor: ExprId,
        fields: &[Arg],
        base: ExprId,
        span: Span,
    ) -> Expr {
        let ExprKind::Path(Res::Def(def)) = self.hir.exprs[ctor].kind else {
            unreachable!("部分更新の構成子は構成子の参照");
        };
        // 表にないのは handler の構成子．handler は alpha.2．
        let Some(&ctor) = self.ctors.get(def) else {
            return self.unsupported("record update of this constructor", span);
        };
        let slots = &self.types.calls[id];
        let mut args: Vec<Option<Expr>> = vec![None; slots.len()];
        let mut lets = Vec::new();
        for field in fields {
            let pos = slots
                .iter()
                .position(|s| *s == Some(field.value))
                .expect("型検査がフィールドを位置に置いてある");
            let value = self.expr(field.value);
            args[pos] = Some(if is_atomic(&value) {
                value
            } else {
                let l = self.module.local("arg");
                lets.push((l, value));
                Expr::Var(l)
            });
        }
        let b = self.module.local("base");
        let base = self.expr(base);
        lets.push((b, base));
        let args = args
            .into_iter()
            .enumerate()
            .map(|(index, arg)| {
                arg.unwrap_or_else(|| Expr::Field {
                    base: Box::new(Expr::Var(b)),
                    ctor,
                    index,
                })
            })
            .collect();
        lets.into_iter()
            .rev()
            .fold(Expr::Ctor { ctor, args }, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }

    /// 文字列の補間．埋め込む値には型を付ける（バックエンドが型から表示関数を作る）．
    fn interpolated(&mut self, parts: &[HirStrPart]) -> Expr {
        let parts = parts
            .iter()
            .map(|part| match part {
                HirStrPart::Text(s) => StrPart::Lit(s.clone()),
                HirStrPart::Interp(e) => {
                    let ty = self.ty(self.expr_ty(*e));
                    if let Some(what) = unshowable(&ty) {
                        self.report(what, self.hir.exprs[*e].span);
                    }
                    StrPart::Value(self.expr(*e), ty)
                }
            })
            .collect();
        Expr::Concat(parts)
    }

    /// ブロック．文を `let` の入れ子にする．値を使わない文は捨てる用の変数に束縛する．
    /// 変数と `_` でないパターンの束縛（`(a, b) = pair`）は，腕1つの match にして残りの文を
    /// 腕の本体に入れる．束縛のパターンが必ず成功することは型検査が確かめてある（E0317）．
    fn block(&mut self, block: &Block) -> Expr {
        let hir = self.hir;
        let mut binds = Vec::new();
        for stmt in &block.stmts {
            let (pat, value) = match stmt {
                Stmt::Let { pat, value, .. } => (Some(*pat), *value),
                Stmt::Expr(e) => (None, *e),
            };
            let value = self.expr(value);
            let bind = match pat.map(|p| (p, &hir.pats[p].kind)) {
                None | Some((_, PatKind::Wild)) => Bind::Var(self.module.local("_"), value),
                Some((_, PatKind::Bind(l))) => Bind::Var(self.local(*l), value),
                Some((p, _)) => {
                    let pat = self.pat(p);
                    // 必ず成功する束縛（E0317）に const のパターンは来ない．
                    if !std::mem::take(&mut self.pat_guards).is_empty() {
                        self.report("const pattern in binding", hir.pats[p].span);
                    }
                    Bind::Pat(pat, value)
                }
            };
            binds.push(bind);
        }
        let tail = match block.tail {
            Some(e) => self.expr(e),
            None => Expr::Lit(Lit::Unit),
        };
        binds.into_iter().rev().fold(tail, |body, bind| match bind {
            Bind::Var(var, value) => Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            },
            Bind::Pat(pat, value) => Expr::Match {
                scrutinee: Box::new(value),
                arms: vec![Arm {
                    pat,
                    guard: None,
                    body,
                }],
            },
        })
    }

    /// match の腕．const のパターンの比較を，書いたガードより先に評価するガードにする．
    fn arm(&mut self, arm: &HirArm) -> Arm {
        let pat = self.pat(arm.pat);
        let mut guards = std::mem::take(&mut self.pat_guards);
        if let Some(g) = arm.guard {
            guards.push(self.expr(g));
        }
        Arm {
            pat,
            guard: guards.into_iter().reduce(|a, b| Expr::Binary {
                op: BinOp::And,
                ty: OpTy::Bool,
                lhs: Box::new(a),
                rhs: Box::new(b),
            }),
            body: self.expr(arm.body),
        }
    }

    /// パターン．const のパターンは比較を `pat_guards` に積む（呼び出し側がガードにする）．
    fn pat(&mut self, id: PatId) -> Pat {
        let hir = self.hir;
        let p = &hir.pats[id];
        match &p.kind {
            PatKind::Wild => Pat::Wild,
            PatKind::Bind(l) => Pat::Bind(self.local(*l)),
            PatKind::Lit(lit) => Pat::Lit(self.lit_value(&self.types.pats[id], lit)),
            PatKind::Unit => Pat::Lit(Lit::Unit),
            PatKind::Tuple(ps) => Pat::Tuple(ps.iter().map(|&p| self.pat(p)).collect()),
            PatKind::List { elems, rest } => Pat::List {
                elems: elems.iter().map(|&p| self.pat(p)).collect(),
                rest: rest.map(|r| {
                    Box::new(match r.binding {
                        Some(l) => Pat::Bind(self.local(l)),
                        None => Pat::Wild,
                    })
                }),
            },
            PatKind::Ctor {
                res: Res::Def(def), ..
            } => {
                if Some(*def) == self.true_def {
                    return Pat::Lit(Lit::Bool(true));
                }
                if Some(*def) == self.false_def {
                    return Pat::Lit(Lit::Bool(false));
                }
                let Some(&ctor) = self.ctors.get(*def) else {
                    self.report("this constructor pattern", p.span);
                    return Pat::Wild;
                };
                // 型検査がフィールドの位置の順に並べ直してある．省いたフィールドは `None`．
                // 括弧のない構成子のパターン（`Empty`）は表にない．
                let n = self.module.variant(ctor).fields.len();
                let fields = match self.types.pat_fields.get(id) {
                    Some(fs) => fs
                        .iter()
                        .map(|f| match f {
                            Some(f) => self.pat(*f),
                            None => Pat::Wild,
                        })
                        .collect(),
                    None => vec![Pat::Wild; n],
                };
                Pat::Ctor { ctor, fields }
            }
            // const のパターンは新しい変数で受けて，ガードで const の値と比べる．
            PatKind::Const(Res::Def(def)) => {
                let x = self.module.local("c");
                let ty = prim_op_ty(&self.types.pats[id]).unwrap_or(OpTy::Structural);
                let value = Expr::Call {
                    callee: Callee::Direct(self.fn_id(*def)),
                    args: Vec::new(),
                    fails: false,
                    suspends: false,
                };
                self.pat_guards.push(Expr::Binary {
                    op: BinOp::Eq,
                    ty,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(value),
                });
                Pat::Bind(x)
            }
            PatKind::Ctor { .. } | PatKind::Const(_) | PatKind::Missing => {
                self.report("this pattern", p.span);
                Pat::Wild
            }
        }
    }

    fn declare_enum(&mut self, def: DefId, params: usize) -> EnumId {
        let name = self.ir_name(def);
        self.module.add_enum(EnumDef {
            name,
            params,
            variants: Vec::new(),
            error: false,
        })
    }

    /// IR での名前．エントリのモジュールの定義は名前のまま，他はモジュール名を付ける（`List.map`）．
    fn ir_name(&self, def: DefId) -> String {
        let data = &self.hir.defs[def];
        match &data.module {
            DefModule::Source(m) if *m == self.entry => data.name.to_string(),
            DefModule::Source(m) => format!("{}.{}", self.modules[*m].name, data.name),
            DefModule::Prelude => data.name.to_string(),
            DefModule::Builtin(m) => format!("{m}.{}", data.name),
        }
    }

    /// JS で export するか．エントリのモジュールの `pub` の fn と `main`（起動用モジュールが呼ぶ）．
    /// テスト関数は除く（`emela test` はテストランナーが export する）．
    fn is_exported(&self, def: DefId) -> bool {
        let data = &self.hir.defs[def];
        data.kind == DefKind::Fn
            && data.module == DefModule::Source(self.entry)
            && (data.is_pub || data.name == "main")
            && !self.tests.contains(&def)
    }
}

/// 外部関数の引数と戻り値の型の，境界での扱い（12.4，12.5）．
fn boundary(ty: &Ty) -> Boundary {
    match ty {
        Ty::Prim(Prim::String) => Boundary::String,
        Ty::Prim(Prim::Unit) => Boundary::Unit,
        Ty::Fn(f) => Boundary::Fn {
            params: f.params.iter().map(boundary).collect(),
            ret: Box::new(boundary(&f.ret)),
        },
        _ => Boundary::Plain,
    }
}

/// パスの `.` と `..` を字面で畳む（シンボリックリンクは解かない）．
fn normalize(path: &Path) -> String {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out.to_string_lossy().into_owned()
}

/// `lib/` の直下からの相対の指定（`./a/../b.mjs`）を，`.` と `..` を畳んだ `lib/` からの
/// パス（`b.mjs`）にする．`lib/` の外へ出るなら `None`．
fn lib_relative(spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// 式が局所変数 `l` を参照するか．
fn mentions(e: &Expr, l: Local) -> bool {
    if let Expr::Var(v) = e {
        return *v == l;
    }
    let mut found = false;
    e.for_each_child(&mut |c| found = found || mentions(c, l));
    found
}

/// 式の中で参照するトップレベル関数（値としての参照と直接の呼び出し）を `out` に積む．
fn collect_fn_refs(e: &Expr, out: &mut Vec<FnId>) {
    match e {
        Expr::Fn(f) => out.push(*f),
        Expr::Call {
            callee: Callee::Direct(f),
            ..
        } => out.push(*f),
        _ => {}
    }
    e.for_each_child(&mut |c| collect_fn_refs(c, out));
}

/// 演算子の対象の型．基本型でなければ `None`．
fn prim_op_ty(ty: &Ty) -> Option<OpTy> {
    match ty {
        Ty::Prim(Prim::Int) => Some(OpTy::Int),
        Ty::Prim(Prim::Int64) => Some(OpTy::Int64),
        Ty::Prim(Prim::Float) => Some(OpTy::Float),
        Ty::Prim(Prim::Bool) => Some(OpTy::Bool),
        Ty::Prim(Prim::String) => Some(OpTy::String),
        _ => None,
    }
}

/// 比較の演算子．
fn comparison(op: BinaryOp) -> Option<BinOp> {
    Some(match op {
        BinaryOp::Eq => BinOp::Eq,
        BinaryOp::Ne => BinOp::Ne,
        BinaryOp::Lt => BinOp::Lt,
        BinaryOp::Le => BinOp::Le,
        BinaryOp::Gt => BinOp::Gt,
        BinaryOp::Ge => BinOp::Ge,
        _ => return None,
    })
}

/// 評価しても何も起きない式．評価の順を変えても観測できない．
fn is_atomic(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) | Expr::Lambda { .. }
    )
}

/// 補間で表示できない型なら，その理由．型引数の値は Show の辞書渡しか単相化が要る
/// （Trait は 0.21）．関数は表示できない（10.6）が，Show の検査は Trait の段で入る．
fn unshowable(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::Param(_) => Some("showing a value of a type parameter"),
        Type::Fn => Some("showing a function"),
        Type::Tuple(ts) | Type::Enum(_, ts) => ts.iter().find_map(unshowable),
        Type::List(t) => unshowable(t),
        _ => None,
    }
}

/// `_` を取り除いた整数リテラル（`42`，`0xFF`，`0b1010`）の値．
fn parse_int(text: &str) -> u64 {
    let (digits, radix) = if let Some(hex) = text.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(bin) = text.strip_prefix("0b") {
        (bin, 2)
    } else {
        (text, 10)
    };
    u64::from_str_radix(digits, radix).expect("型検査が範囲を確かめた整数")
}
