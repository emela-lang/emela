//! 型検査の結果（HIR と `ProgramTypes`）から Core IR を作る（lowering）．
//!
//! プログラム全体（利用者のモジュール，Prelude，core）を IR の [`Module`] 1つにする．
//! enum は先に全部割り振る（[`Lowerer::declare_enums`]）．fn は根からたどれるものだけを写す．
//! fn は参照に出会ったときに ID を割り振って作業リストに積み（[`Lowerer::fn_id`]），
//! 作業リストが尽きるまで本体を変換する．

use std::path::Path;

use emela_resolve::hir::{
    Arg, Arm as HirArm, BinaryOp, Block, DefId, DefKind, DefModule, ExprId, ExprKind, Fields, Item,
    Literal, LocalId, LocalKind, PatId, PatKind, Program, Res, Span, Stmt, StrPart as HirStrPart,
    UnaryOp,
};
use emela_resolve::{ModuleId, ModuleMap};
use emela_types::{Prim, ProgramTypes, Row, RowElem, Tag, Ty};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;

use crate::intrinsics::Builtin;
use crate::ir::{
    Arm, Assert, BinOp, Boundary, Callee, CtorRef, EffectDef, EffectId, EnumDef, EnumId, Expr,
    External, ExternalBinding, FnId, JsFile, Lit, Local, Module, OpDef, OpTy, Pat, StrPart, Type,
    UnOp, VariantDef, VariantFields, WithBind,
};
use crate::named;

mod suspend;

use suspend::Specialization;

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
        fns: FxHashMap::default(),
        worklist: Vec::new(),
        current_def: None,
        spec: 0,
        enums: ArenaMap::default(),
        ctors: ArenaMap::default(),
        locals: ArenaMap::default(),
        effects: FxHashMap::default(),
        handler_parts: FxHashMap::default(),
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
                cx.fn_id(def, 0);
            }
        }
    }
    let tests = match roots {
        Roots::Tests => input.tests.iter().map(|&def| cx.fn_id(def, 0)).collect(),
        _ => Vec::new(),
    };
    while let Some((def, spec)) = cx.worklist.pop() {
        cx.define_fn(def, spec);
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
    /// HIR の fn と const と，どの中断用の写しか（`suspend.rs`）→ IR の関数．const は引数のない関数にする．
    /// [`Lowerer::fn_id`] だけが足す．
    fns: FxHashMap<(DefId, Specialization), FnId>,
    /// ID を割り振ったが本体をまだ写していない fn（と，どの中断用の写しか）．
    worklist: Vec<(DefId, Specialization)>,
    /// 本体を写している定義と，どの中断用の写しか．ラムダと呼び出しの「中断しうる」の印を決めるのに使う．
    current_def: Option<DefId>,
    spec: Specialization,
    /// HIR の type / enum / error → IR の enum．
    enums: ArenaMap<DefId, EnumId>,
    /// HIR の構成子 → IR の構成子．error は宣言そのものが構成子．Bool の構成子は持たない．
    ctors: ArenaMap<DefId, CtorRef>,
    /// HIR の局所変数 → IR の局所変数．[`Lowerer::local`] だけが足す．
    locals: ArenaMap<LocalId, Local>,
    /// エフェクトとその型引数の鍵（[`EffectKey`]）→ IR のエフェクト．[`Lowerer::effect_id`] だけが足す．
    effects: FxHashMap<EffectKey, EffectId>,
    /// 状態のある handler → init と release の IR の関数．[`Lowerer::handler_fns`] だけが足す．
    handler_parts: FxHashMap<DefId, (FnId, Option<FnId>)>,
    /// 今の証拠の表（evidence passing，16.5）．R が空でない関数とラムダの最後の引数か，
    /// with で足した表．関数の外や R が空の関数では `None`．
    ev: Option<Local>,
    /// 囲む with が供給したエフェクトとハンドラの値．外側が先．同じ関数（トップレベルの fn か
    /// handler の操作）の中の with だけで，ラムダの中にも引き継ぐ．`use X` は後ろから探す（8.3）．
    bound: Vec<(EffectKey, Local)>,
    paths: &'a ArenaMap<ModuleId, &'a Path>,
    core_js_files: &'a [(&'a str, &'a str)],
    /// 写した外部関数が同梱の JS のファイルを指したか．
    uses_core_js: bool,
    errors: Vec<LowerError>,
}

/// 証拠の表の鍵．エフェクトと，型引数を書いた文字列（型引数のないエフェクトは空）．
/// 型引数の違うエフェクトは別のエフェクト（8.1）なので，型引数ごとに別の IR のエフェクトにする．
/// 型引数 `Bound(i)` は，それを持つ定義の IR の名前と添字で書く（`$modify.0`）．
type EffectKey = (DefId, String);

/// ブロックの文の束縛．
enum Bind {
    /// `let var = value`．
    Var(Local, Expr),
    /// 腕1つの match．
    Pat(Pat, Expr),
}

/// 呼び出しの呼ばれる側．
///
/// `Call` は呼び出しの印（`fails` は `Expr::Call::fails`，`suspends` は `Expr::Call::suspends`）．
enum Target {
    /// `Option<Expr>` は最後の引数に足す証拠の表（R が空でない関数を呼ぶとき）．
    Direct(FnId, Call, Option<Expr>),
    Builtin {
        op: Builtin,
        ty_args: Vec<Type>,
    },
    Ctor(CtorRef),
    /// 関数の値．評価済みで，副作用なく何度でも参照できる式（変数か関数）．
    /// `Option<Expr>` は `Direct` と同じ．
    Value(Expr, Call, Option<Expr>),
    /// 能力の値の操作（`log.info`）．`handler` は能力の値の式．
    Op {
        handler: Expr,
        effect: EffectId,
        op: usize,
        call: Call,
    },
    /// `State.run(initial, body)`（8.8）．ランタイムが供給する組み込みのハンドラ．
    StateRun(Box<StateRun>),
}

/// `State.run(initial, body)` を写すのに要るもの．局所変数は先に割り振っておく．
///
/// 初期値の入れ物（`$cellNew`）を作り，それを読み書きするハンドラの値を `State[S]` の鍵で証拠の表に
/// 足して body を呼び，body の値と最後の状態の組を返す．body が失敗すれば，呼び出しの失敗として
/// そのまま伝わり，状態は捨てる．
struct StateRun {
    /// `State[S]` の IR のエフェクト．
    effect: EffectId,
    /// 操作 get，set，update の `EffectDef::ops` の添字．
    ops: [usize; 3],
    /// body を呼ぶときの土台の証拠の表（今の表）．
    base: Option<Expr>,
    /// body の呼び出しの印．
    call: Call,
    cell: Local,
    handler: Local,
    value: Local,
    f: Local,
    result: Local,
}

impl StateRun {
    /// 位置の順の引数（初期値，body）で `State.run` の式を作る．
    fn apply(self, args: Vec<Expr>) -> Expr {
        let [initial, body]: [Expr; 2] = args.try_into().expect("State.run の引数は2つ");
        let cell = || Expr::Var(self.cell);
        let get = || Expr::Builtin {
            op: Builtin::CellGet,
            ty_args: Vec::new(),
            args: vec![cell()],
        };
        let set = |value: Expr| Expr::Builtin {
            op: Builtin::CellSet,
            ty_args: Vec::new(),
            args: vec![cell(), value],
        };
        let lambda = |params: Vec<Local>, body: Expr| Expr::Lambda {
            params,
            body: Box::new(body),
            suspends: false,
        };
        // update に渡す関数は純粋（8.8）なので，読んで呼んで書くまでが一息に終わる．
        let update = set(Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
            args: vec![get()],
            fails: false,
            suspends: false,
        });
        let mut ops = vec![None, None, None];
        ops[self.ops[0]] = Some(lambda(Vec::new(), get()));
        ops[self.ops[1]] = Some(lambda(vec![self.value], set(Expr::Var(self.value))));
        ops[self.ops[2]] = Some(lambda(vec![self.f], update));
        let handler = Expr::Handler {
            name: "State".to_owned(),
            effect: self.effect,
            ops: ops.into_iter().map(|o| o.expect("操作は3つ")).collect(),
        };
        let run = Expr::Call {
            callee: Callee::Indirect(Box::new(body)),
            args: vec![Expr::Provide {
                base: self.base.map(Box::new),
                bindings: vec![(self.effect, Expr::Var(self.handler))],
            }],
            fails: self.call.fails,
            suspends: self.call.suspends,
        };
        let lets = [
            (
                self.cell,
                Expr::Builtin {
                    op: Builtin::CellNew,
                    ty_args: Vec::new(),
                    args: vec![initial],
                },
            ),
            (self.handler, handler),
            (self.result, run),
        ];
        let tuple = Expr::Tuple(vec![Expr::Var(self.result), get()]);
        lets.into_iter()
            .rev()
            .fold(tuple, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }
}

/// 呼び出しの印．
#[derive(Clone, Copy)]
struct Call {
    fails: bool,
    suspends: bool,
}

impl Target {
    /// 位置の順に並んだ引数で呼ぶ式．
    fn apply(self, args: Vec<Expr>) -> Expr {
        match self {
            Target::Direct(f, call, ev) => Expr::Call {
                callee: Callee::Direct(f),
                args: args.into_iter().chain(ev).collect(),
                fails: call.fails,
                suspends: call.suspends,
            },
            Target::Builtin { op, ty_args } => Expr::Builtin { op, ty_args, args },
            Target::Ctor(ctor) => Expr::Ctor { ctor, args },
            Target::Value(f, call, ev) => Expr::Call {
                callee: Callee::Indirect(Box::new(f)),
                args: args.into_iter().chain(ev).collect(),
                fails: call.fails,
                suspends: call.suspends,
            },
            Target::Op {
                handler,
                effect,
                op,
                call,
            } => Expr::Call {
                callee: Callee::Op {
                    handler: Box::new(handler),
                    effect,
                    op,
                },
                args,
                fails: call.fails,
                suspends: call.suspends,
            },
            Target::StateRun(run) => run.apply(args),
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
                    // 状態のあるハンドラの値（self）は，handler の構成子1つの enum（8.4）．
                    Some(Item::Handler(h)) if is_stateful(h) => {
                        let id = self.declare_enum(def, 0);
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
                Some(Item::Handler(h)) => h.ctor.into_iter().collect(),
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
                match self.enums.get(def) {
                    Some(&id) => Type::Enum(id, args.iter().map(|t| self.ty(t)).collect()),
                    // 状態を持たない handler の `self`．値は `()`（`Lowerer::path`）．
                    None => Type::Unit,
                }
            }
        }
    }

    /// fn の写し `spec` の IR の ID．まだなければ割り振って作業リストに積む．fn の ID はここでしか
    /// 作らない．元の関数（0）の名前は定義の IR の名前のまま，中断用の写しは `名前$suspendビット` に
    /// する（`$` は Emela の名前に現れないので重ならない）．公開するのは元の関数だけ．
    fn fn_id(&mut self, def: DefId, spec: Specialization) -> FnId {
        if let Some(&id) = self.fns.get(&(def, spec)) {
            return id;
        }
        let mut name = self.ir_name(def);
        if spec != 0 {
            name = format!("{name}$suspend{spec}");
        }
        let id = self
            .module
            .declare(&name, spec == 0 && self.is_exported(def));
        self.module.functions[id].suspends = self.fn_suspends(def, spec);
        self.fns.insert((def, spec), id);
        self.worklist.push((def, spec));
        id
    }

    /// 式 `expr` で参照した fn の IR の ID．具体化に合う写しを選ぶ．
    fn fn_ref(&mut self, def: DefId, expr: ExprId) -> FnId {
        let spec = self.specialization_of(def, Some(expr));
        self.fn_id(def, spec)
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

    fn define_fn(&mut self, def: DefId, spec: Specialization) {
        let hir = self.hir;
        let id = self.fns[&(def, spec)];
        // 関数ごとに証拠の表と with の束縛を始め直す．局所変数の対応も作り直す（同じ定義の元の関数と
        // 中断用の写しが局所変数を共有しないように．HIR の局所変数はどれか1つの定義の本体のもの）．
        self.ev = None;
        self.bound.clear();
        self.locals = ArenaMap::default();
        self.current_def = Some(def);
        self.spec = spec;
        let (mut params, body) = match hir.item(def) {
            // `@external suspend fn` は，宣言の型の R に Async があるので中断しうる関数になり，
            // ホストの関数の返した Promise を待つ（12.2，`fn_id`）．
            Some(Item::Fn(f)) if f.body.is_none() => {
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

    /// fn が証拠の表を最後の引数に受けるか．宣言の型（汎化した型）の R に Async 以外の要素があれば
    /// 受ける．中身の分からない集合（`R(f)`，汎化した集合）も受ける（引数の関数へ渡すため）．
    /// Async はランタイムが供給し（8.3，13.3），ハンドラの値を持たないので，表は要らない．
    fn takes_ev(&self, def: DefId) -> bool {
        matches!(self.types.sigs.get(def).map(|s| &s.scheme.ty), Some(Ty::Fn(f)) if self.row_needs_ev(&f.uses))
    }

    /// 関数型の値を呼ぶときに証拠の表を渡すか．値の型の R に Async 以外の要素があれば渡す．
    fn ty_takes_ev(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Fn(f) if self.row_needs_ev(&f.uses))
    }

    /// 効果の集合 `row` の関数に証拠の表が要るか．Async 以外の要素があれば要る．
    fn row_needs_ev(&self, row: &Row) -> bool {
        let async_ = self.hir.prelude_def("Async");
        row.elems()
            .iter()
            .any(|e| !matches!(e, RowElem::Tag(t) if Some(t.def) == async_))
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
    fn effect_id(&mut self, key: &EffectKey) -> EffectId {
        if let Some(&id) = self.effects.get(key) {
            return id;
        }
        let (def, args) = key;
        let ops = match self.hir.item(*def) {
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
        let name = if args.is_empty() {
            self.ir_name(*def)
        } else {
            format!("{}[{args}]", self.ir_name(*def))
        };
        let id = self.module.add_effect(EffectDef { name, ops });
        self.effects.insert(key.clone(), id);
        id
    }

    /// タグの証拠の表の鍵（8.1）．`ctx` は型引数（`Bound(i)`）と汎化した型変数を持つ定義．
    fn tag_key(&self, tag: &Tag, ctx: Option<DefId>) -> EffectKey {
        let args: Vec<String> = tag.args.iter().map(|t| self.key_ty(t, ctx)).collect();
        (tag.def, args.join(", "))
    }

    /// 本体を写している定義の中のタグの鍵．
    fn key_here(&self, tag: &Tag) -> EffectKey {
        self.tag_key(tag, self.current_def)
    }

    /// 鍵に書く型．名前付き型は IR の名前，型引数は持ち主の定義の IR の名前と添字．決まらずに残った
    /// 型変数（汎化しなかったもの）は `_`（lowering は `()` として扱う．どれも同じ鍵になる）．
    fn key_ty(&self, ty: &Ty, ctx: Option<DefId>) -> String {
        let list = |tys: &[Ty]| {
            tys.iter()
                .map(|t| self.key_ty(t, ctx))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match ty {
            Ty::Prim(p) => p.name().to_owned(),
            Ty::Never => "Never".to_owned(),
            Ty::List(e) => format!("List[{}]", self.key_ty(e, ctx)),
            Ty::Tuple(es) => format!("({})", list(es)),
            Ty::Named(id, args) => {
                let name = self.ir_name(self.types.ty_table.def(*id));
                if args.is_empty() {
                    name
                } else {
                    format!("{name}[{}]", list(args))
                }
            }
            // 関数型の E と R は鍵に入れない（近似．NOTES.md）．
            Ty::Fn(f) => format!("fn({}) -> {}", list(&f.params), self.key_ty(&f.ret, ctx)),
            Ty::Bound(i) => match ctx {
                Some(def) => format!("${}.{i}", self.ir_name(def)),
                None => format!("${i}"),
            },
            Ty::Var(v) => ctx
                .and_then(|def| self.types.generalized_tys.get(def))
                .and_then(|vars| vars.iter().find(|(w, _)| w == v))
                .map_or_else(|| "_".to_owned(), |&(_, i)| self.key_ty(&Ty::Bound(i), ctx)),
            Ty::Set(..) | Ty::Cap(_) | Ty::Error => "_".to_owned(),
        }
    }

    /// 鍵のエフェクトの今のハンドラ．同じ関数の中で供給する最も内側の with のハンドラか，
    /// 証拠の表から引く（`use X`，8.3）．どちらもなければ `None`．
    fn lookup_effect(&mut self, key: &EffectKey) -> Option<Expr> {
        if let Some((_, h)) = self.bound.iter().rev().find(|(e, _)| e == key) {
            return Some(Expr::Var(*h));
        }
        let ev = self.ev?;
        Some(Expr::Use {
            ctx: Box::new(Expr::Var(ev)),
            effect: self.effect_id(key),
        })
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
                        let lib_dir = module
                            .map(|m| crate::lib_dir(&self.modules[m].name.to_string()))
                            .unwrap_or("core");
                        args[0] = self.core_js_file(lib_dir, &args[0], self.hir.defs[def].span);
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
    /// 書き出したファイルへの，エントリの出力からの相対パス（`./emela_core/std/x.mjs`）にする．
    /// `spec` はソースを置いた `lib/` の中のディレクトリ（`lib_dir`．`std` か `core`）からの相対パス．
    fn core_js_file(&mut self, lib_dir: &str, spec: &str, span: Option<Span>) -> String {
        let found = lib_relative(&format!("{lib_dir}/{spec}"))
            .filter(|path| self.core_js_files.iter().any(|(p, _)| p == path));
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

    /// handler を IR の関数にする（`define_fn` から呼ぶ）．返すのはハンドラの値を返す関数の引数と本体．
    ///
    /// 状態を持たないハンドラは，ハンドラの値（操作は宣言の順のラムダ）を返す関数．依存（操作の本体の
    /// R，8.4）があれば，with の位置の証拠の表を最後の引数に受ける．
    ///
    /// 状態のあるハンドラ（フィールド，init，release のどれかを持つ，8.4）は3つの関数にする．
    /// init の関数（`名前$init`）は状態（self）を返し，ハンドラの値を返す関数は状態を最初の引数に受けて
    /// 操作のラムダに捕まえさせ，release の関数（`名前$release`）は状態を受けて release の本体を評価する．
    /// 証拠の表は3つとも同じ表を最後の引数に受ける（依存(H) は init，release，操作の本体の R の和）．
    fn handler(&mut self, def: DefId) -> (Vec<Local>, Expr) {
        let hir = self.hir;
        let Some(Item::Handler(h)) = hir.item(def) else {
            unreachable!("handler の定義");
        };
        let span = hir.defs[def].span;
        let Res::Def(effect) = h.effect else {
            return (
                Vec::new(),
                self.unsupported_at("handler of an unknown effect", span),
            );
        };
        let Some(tag) = self.types.sigs.handler_effect(def) else {
            return (
                Vec::new(),
                self.unsupported_at("handler of an unknown effect", span),
            );
        };
        let effect_id = self.effect_id(&self.tag_key(tag, None));
        let takes_ev = self
            .types
            .handler_uses
            .get(def)
            .is_some_and(|r| self.row_needs_ev(r));
        let stateful = is_stateful(h);
        if stateful {
            self.handler_parts(def, h, takes_ev);
        }

        let mut params = Vec::new();
        if stateful {
            // 操作ごとの暗黙の self は，どれもこの引数を指す．
            let state = self.module.local("self");
            for op in &h.ops {
                self.locals.insert(op.self_local, state);
            }
            params.push(state);
        }
        self.ev = None;
        if takes_ev {
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
            // suspend fn の実装の引数は，操作の引数と最後の resume（8.4）．resume は普通の
            // 関数の値で，呼んでも中断も失敗もしない（型が `fn(T) -> ()`）．
            let ps = op.params.iter().map(|p| self.local(p.local)).collect();
            let lowered = self.expr(body);
            // 本体が中断しうる（`@external suspend fn` を待つなど）なら，実装もジェネレータにする．
            // resume を後から呼ぶだけの Async（8.7）は本体の R に載らないので，中断しない．
            let row_suspends = self
                .types
                .effects
                .get(body)
                .is_some_and(|r| self.row_suspends(r));
            // fn の操作の実装は，型検査（E0417）と同じく，本体の R が Immediate か中断しうる呼び出しを
            // 含まなければジェネレータにしない．本体の R に Clock があっても，`clock.now()` しか
            // 呼ばなければ中断しない．R が Immediate で中に真の呼び出しがある（その場で resume する
            // ハンドラの with の中）ときは，中断しない関数の本体と同じくバックエンドがその場で進める．
            let suspends = if op.is_suspend {
                row_suspends
            } else {
                row_suspends && suspend::has_suspending_call(&lowered)
            };
            // 操作の本体の失敗は戻り値として操作の呼び出し元へ返る（ラムダと同じ）．
            ops[index] = Some(Expr::Lambda {
                params: ps,
                body: Box::new(lowered),
                suspends,
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

    /// 状態のあるハンドラの init と release の関数の本体を写す．
    fn handler_parts(&mut self, def: DefId, h: &emela_resolve::hir::HandlerItem, takes_ev: bool) {
        let (init, release) = self.handler_fns(def);
        let ev_param = |cx: &mut Self| {
            cx.ev = None;
            takes_ev.then(|| {
                let ev = cx.module.local("ev");
                cx.ev = Some(ev);
                ev
            })
        };

        // init の値がハンドラの状態．init のない（フィールドのない）ハンドラの状態は，構成子があれば
        // その値（`ConsoleLogger`），なければ `()`．
        let params: Vec<Local> = ev_param(self).into_iter().collect();
        let body = match (h.init, h.ctor.and_then(|c| self.ctors.get(c).copied())) {
            (Some(body), _) => self.expr(body),
            (None, Some(ctor)) => Expr::Ctor {
                ctor,
                args: Vec::new(),
            },
            (None, None) => Expr::Lit(Lit::Unit),
        };
        self.module.define(init, params, body);

        if let (Some(f), Some((self_local, body))) = (release, h.release) {
            let state = self.local(self_local);
            let mut params = vec![state];
            params.extend(ev_param(self));
            let body = self.expr(body);
            self.module.define(f, params, body);
        }
    }

    /// 状態のあるハンドラの init と release（あれば）の IR の関数．まだなければ宣言する．本体は
    /// ハンドラの値を返す関数（`fn_id(def, 0)`）と一緒に `define_fn` で写す．
    ///
    /// 中断しうるかは本体の式の R で決める（init が `@external suspend fn` を待つなど）．
    fn handler_fns(&mut self, def: DefId) -> (FnId, Option<FnId>) {
        if let Some(&fns) = self.handler_parts.get(&def) {
            return fns;
        }
        self.fn_id(def, 0);
        let Some(Item::Handler(h)) = self.hir.item(def) else {
            unreachable!("handler の定義");
        };
        let name = self.ir_name(def);
        let suspends = |cx: &Self, body: ExprId| {
            cx.types
                .effects
                .get(body)
                .is_some_and(|r| r.tags().any(|t| !cx.is_immediate_effect(t.def)))
        };
        let init = self.module.declare(&format!("{name}$init"), false);
        self.module.functions[init].suspends = h.init.is_some_and(|b| suspends(self, b));
        let release = h.release.map(|(_, body)| {
            let f = self.module.declare(&format!("{name}$release"), false);
            self.module.functions[f].suspends = suspends(self, body);
            f
        });
        self.handler_parts.insert(def, (init, release));
        (init, release)
    }

    /// `with H1, ..., Hn { body }`（8.3，14.2）．
    ///
    /// ハンドラを型検査の決めた初期化の順（`ProgramTypes::with_handlers`．layer は展開し，後ろに
    /// 並べたもので上書きされたハンドラは除いてある）に作り，今の証拠の表に供給するエフェクトを足した
    /// 表で本体を評価する．ハンドラの依存は，同じ with の別のエフェクトを供給するハンドラが満たし，
    /// 残りと自分のエフェクトへの依存は外側の表が満たす．依存の循環は型検査が E0411 にしてある．
    ///
    /// 状態のあるハンドラは，init の値（状態）を release のない束縛に，状態を受けて作ったハンドラの
    /// 値を release のある束縛（`WithBind::managed`）にする．release の関数がなければ release は `()`．
    fn with(&mut self, id: ExprId, handlers: &[(Res, Span)], body: ExprId, span: Span) -> Expr {
        let hir = self.hir;
        let Some(order) = self.types.with_handlers.get(id) else {
            return self.unsupported("with without handlers", span);
        };
        // （handler，供給するエフェクトの鍵）．
        let mut hs: Vec<(DefId, EffectKey)> = Vec::new();
        for &def in order {
            // 誤りはハンドラを書いた位置に出す．layer から展開したものは with の位置．
            let span = handlers
                .iter()
                .find(|(res, _)| *res == Res::Def(def))
                .map_or(span, |&(_, s)| s);
            match hir.item(def) {
                Some(Item::Handler(_)) => {
                    let Some(tag) = self.types.sigs.handler_effect(def) else {
                        return self.unsupported("handler of an unknown effect", span);
                    };
                    hs.push((def, self.tag_key(tag, None)));
                }
                _ => return self.unsupported("this kind of handler", span),
            }
        }
        let outer = self.ev;
        let mut locals: Vec<Local> = Vec::with_capacity(hs.len());
        let mut binds = Vec::new();
        for (i, (h, own)) in hs.iter().enumerate() {
            let h = *h;
            let f = self.fn_id(h, 0);
            let ctx = if self
                .types
                .handler_uses
                .get(h)
                .is_some_and(|r| self.row_needs_ev(r))
            {
                // 依存のうち，同じ with の先に初期化した別のエフェクトのハンドラが満たすもの．
                let row = self.types.handler_uses.get(h).cloned().unwrap_or_default();
                let keys: Vec<EffectKey> = row
                    .tags()
                    .map(|t| self.tag_key(t, None))
                    .filter(|y| y != own)
                    .collect();
                let bindings: Vec<(EffectId, Expr)> = keys
                    .into_iter()
                    .filter_map(|y| {
                        let j = hs[..i].iter().position(|(_, e)| *e == y)?;
                        Some((self.effect_id(&y), Expr::Var(locals[j])))
                    })
                    .collect();
                Some(if bindings.is_empty() {
                    self.ev_arg()
                } else {
                    Expr::Provide {
                        base: outer.map(|l| Box::new(Expr::Var(l))),
                        bindings,
                    }
                })
            } else {
                None
            };
            let l = self.module.local("h");
            locals.push(l);
            let Some(Item::Handler(item)) = hir.item(h) else {
                unreachable!("handler の定義");
            };
            if !is_stateful(item) {
                binds.push(WithBind::new(
                    l,
                    direct(f, ctx.into_iter().collect(), false),
                ));
                continue;
            }
            // 状態のあるハンドラは表を3回使うので，変数に入れておく．
            let ctx = ctx.map(|c| match c {
                Expr::Var(_) | Expr::Lit(_) => c,
                c => {
                    let v = self.module.local("ev");
                    binds.push(WithBind::new(v, c));
                    Expr::Var(v)
                }
            });
            let (init, release) = self.handler_fns(h);
            let state = self.module.local("self");
            let init_fails = self
                .types
                .handler_fails
                .get(h)
                .is_some_and(|r| !r.is_empty());
            binds.push(WithBind::new(
                state,
                direct_marked(
                    &self.module,
                    init,
                    ctx.clone().into_iter().collect(),
                    init_fails,
                ),
            ));
            let with_state = |ctx: &Option<Expr>| {
                std::iter::once(Expr::Var(state))
                    .chain(ctx.clone())
                    .collect::<Vec<_>>()
            };
            let release = match release {
                Some(r) => direct_marked(&self.module, r, with_state(&ctx), false),
                None => Expr::Lit(Lit::Unit),
            };
            binds.push(WithBind::managed(
                l,
                direct(f, with_state(&ctx), false),
                &hir.defs[h].name,
                release,
            ));
        }
        let ctx = self.module.local("ev");
        let depth = self.bound.len();
        for ((_, effect), &l) in hs.iter().zip(&locals) {
            self.bound.push((effect.clone(), l));
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
                .map(|((_, effect), &l)| (self.effect_id(effect), Expr::Var(l)))
                .collect();
            binds.push(WithBind::new(
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
    /// 呼び出し元から渡された証拠の表から引く．型引数のあるエフェクトは型引数まで同じものを引く（8.1）．
    fn use_(&mut self, id: ExprId, span: Span) -> Expr {
        let Some(Ty::Cap(tag)) = self.types.exprs.get(id) else {
            return self.unsupported("unresolved effect", span);
        };
        let key = self.key_here(tag);
        match self.lookup_effect(&key) {
            Some(e) => e,
            // R を確かめた型検査を通っていれば来ない．
            None => self.unsupported("`use` without a provided effect", span),
        }
    }

    /// ラムダ．型の R が空でなければ，証拠の表を最後の引数に受ける．囲む with の束縛は，受けた表に
    /// 足して本体の表にする（`use X` は囲む with を先に見る，8.3）．
    fn lambda(&mut self, id: ExprId, params: &[emela_resolve::hir::Param], body: ExprId) -> Expr {
        let mut ps: Vec<Local> = params.iter().map(|p| self.local(p.local)).collect();
        let suspends = self.ty_suspends(self.expr_ty(id));
        if !self.ty_takes_ev(self.expr_ty(id)) {
            return Expr::Lambda {
                params: ps,
                body: Box::new(self.expr(body)),
                suspends,
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
                .map(|(e, h)| (self.effect_id(&e), Expr::Var(h)))
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
            suspends,
        }
    }

    /// 能力の値の操作を，呼ばずに値として使う（`List.each(xs, log.info)`）．それを呼ぶだけの
    /// ラムダにする．能力の値は参照の位置で評価する．
    fn op_value(&mut self, id: ExprId, base: ExprId, op: DefId, span: Span) -> Expr {
        let Some(Ty::Cap(tag)) = self.types.exprs.get(base).cloned() else {
            return self.unsupported("effect operation", span);
        };
        let Some(index) = self.op_index(tag.def, op) else {
            return self.unsupported("effect operation", span);
        };
        let effect_id = self.effect_id(&self.key_here(&tag));
        let fails = self.callee_fails(id);
        let suspends = self.module.effects[effect_id].ops[index].suspend;
        let (arity, ev) = match self.expr_ty(id) {
            Ty::Fn(f) => (f.params.len(), self.row_needs_ev(&f.uses)),
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
                    suspends,
                }),
                suspends,
            }),
        }
    }

    /// エントリのモジュールの `main` の IR の関数．
    fn entry_main(&self) -> Option<FnId> {
        let items = self.hir.modules.get(self.entry)?;
        items.items.iter().find_map(|&def| {
            let data = &self.hir.defs[def];
            (data.name == "main" && data.kind == DefKind::Fn)
                .then(|| self.fns.get(&(def, 0)).copied())
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
                    .map(|(&(d, _), _)| d)
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
            ExprKind::Use { .. } => self.use_(id, e.span),
            ExprKind::With { handlers, body } => self.with(id, handlers, *body, e.span),
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
            // 状態を持たない handler の `self` は値を持たない（ハンドラの値を作る関数が受けない）．
            Res::Local(l)
                if self.hir.locals[l].kind == LocalKind::SelfParam
                    && self.locals.get(l).is_none() =>
            {
                return Expr::Lit(Lit::Unit);
            }
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
            // 型引数のあるエフェクトを R に持つ汎用の fn は，呼ぶたびに鍵を足す必要があるので包む．
            Some(Item::Fn(f)) if f.body.is_some() && self.has_generic_tags(def) => {
                self.wrap_in_lambda(id, span)
            }
            Some(Item::Fn(f)) if f.body.is_some() => Expr::Fn(self.fn_ref(def, id)),
            // 組み込み関数とフィールドのある構成子は IR の値でないので，ラムダで包む．
            Some(Item::Fn(f)) if f.is_intrinsic => self.wrap_in_lambda(id, span),
            Some(Item::Fn(_)) => Expr::Fn(self.fn_ref(def, id)),
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) if self.module.variant(ctor).fields.is_empty() => Expr::Ctor {
                    ctor,
                    args: Vec::new(),
                },
                _ => self.wrap_in_lambda(id, span),
            },
            // const の値は失敗しない（5.6）．
            Some(Item::Const(_)) => Expr::Call {
                callee: Callee::Direct(self.fn_id(def, 0)),
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
            Target::Value(f, call, ev) if !matches!(f, Expr::Var(_) | Expr::Fn(_)) => {
                // 引数を並べ替えて `let` で束縛すると呼ばれる側より先に評価されるので，
                // 呼ばれる側を先に束縛しておく．
                let l = self.module.local("f");
                (Target::Value(Expr::Var(l), call, ev), Some((l, f)))
            }
            Target::Op {
                handler,
                effect,
                op,
                call,
            } if !matches!(handler, Expr::Var(_) | Expr::Use { .. }) => {
                // 証拠の表を引くだけの式（`(use X).op()`）は副作用がないので束縛しない．
                let l = self.module.local("h");
                let t = Target::Op {
                    handler: Expr::Var(l),
                    effect,
                    op,
                    call,
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
    ///
    /// 参照の型の R が証拠の表を要るなら，ラムダも表を最後の引数に受け，それを呼ぶ側の表にする
    /// （`State.run` と，型引数のあるエフェクトを R に持つ汎用の fn）．
    fn wrap_in_lambda(&mut self, id: ExprId, span: Span) -> Expr {
        let ev = self
            .ty_takes_ev(self.expr_ty(id))
            .then(|| self.module.local("ev"));
        let (outer, bound) = (self.ev, std::mem::take(&mut self.bound));
        if ev.is_some() {
            self.ev = ev;
        }
        let target = self.callee(id, span);
        self.ev = outer;
        self.bound = bound;
        let target = match target {
            Ok(t) => t,
            Err(e) => return e,
        };
        let arity = match self.expr_ty(id) {
            Ty::Fn(f) => f.params.len(),
            _ => unreachable!("呼べる定義の参照の型は関数型"),
        };
        let mut params: Vec<Local> = (0..arity).map(|_| self.module.local("x")).collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        params.extend(ev);
        Expr::Lambda {
            params,
            body: Box::new(target.apply(args)),
            suspends: self.ty_suspends(self.expr_ty(id)),
        }
    }

    /// 呼ばれる側．IR にできなければ診断を積んで `Err` に代わりの式を入れる．
    fn callee(&mut self, callee: ExprId, span: Span) -> Result<Target, Expr> {
        let hir = self.hir;
        // 関数の値の呼び出しは，呼ばれる側の式の型で決める．
        let call = Call {
            fails: self.callee_fails(callee),
            suspends: self.ty_suspends(self.expr_ty(callee)),
        };
        let ev = self
            .ty_takes_ev(self.expr_ty(callee))
            .then(|| self.ev_arg());
        // 能力の値の操作（`log.info(...)`）．
        if let ExprKind::Field { base, .. } = &hir.exprs[callee].kind
            && let Some(&op) = self.types.ops.get(callee)
            && let Some(Ty::Cap(effect)) = self.types.exprs.get(*base).cloned()
            && let Some(index) = self.op_index(effect.def, op)
        {
            let effect = self.effect_id(&self.key_here(&effect));
            let handler = self.expr(*base);
            // suspend fn の操作の呼び出しは中断しうる（8.1）．fn の操作の本体は中断しない（E0417）．
            let call = Call {
                suspends: self.module.effects[effect].ops[index].suspend,
                ..call
            };
            return Ok(Target::Op {
                handler,
                effect,
                op: index,
                call,
            });
        }
        let ExprKind::Path(Res::Def(def)) = hir.exprs[callee].kind else {
            return Ok(Target::Value(self.expr(callee), call, ev));
        };
        match hir.item(def) {
            Some(Item::Fn(f)) if f.body.is_some() => {
                let ev = self
                    .takes_ev(def)
                    .then(|| self.generic_ev(def, callee, self.ev_arg()));
                let f = self.fn_ref(def, callee);
                Ok(Target::Direct(f, self.direct_call(f, call), ev))
            }
            Some(Item::Fn(f)) if f.is_intrinsic && self.is_state_run(def) => self
                .state_run(callee, span)
                .map(|r| Target::StateRun(Box::new(r))),
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
            // 外部関数は失敗せず（12.5），R は空か Async だけ（12.2）なので表も渡さない．
            Some(Item::Fn(_)) => {
                let f = self.fn_ref(def, callee);
                let call = Call {
                    fails: false,
                    ..call
                };
                Ok(Target::Direct(f, self.direct_call(f, call), None))
            }
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) => Ok(Target::Ctor(ctor)),
                None => Err(self.unsupported("this constructor", span)),
            },
            _ => Ok(Target::Value(self.expr(callee), call, ev)),
        }
    }

    /// 型引数を持つタグ（`State[S]` の `S` が型引数）を宣言の R に持つ定義か．
    fn has_generic_tags(&self, def: DefId) -> bool {
        !self.generic_tags(def).is_empty()
    }

    /// 宣言の R の，型引数を含むタグ．
    fn generic_tags(&self, def: DefId) -> Vec<Tag> {
        match self.types.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => f
                .uses
                .tags()
                .filter(|t| t.args.iter().any(has_bound))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// 汎用の定義 `def` を呼ぶときの証拠の表（8.1）．呼ばれる側の本体は自分の型引数で書いた鍵
    /// （`State[$modify.0]`）で引くので，呼ぶ側で具体化した鍵（`State[Int]`）のハンドラを，その鍵
    /// でも引けるように今の表 `ev` に足す．具体化した型引数は参照の式 `callee` の `type_args`．
    fn generic_ev(&mut self, def: DefId, callee: ExprId, ev: Expr) -> Expr {
        let tags = self.generic_tags(def);
        let Some(args) = self.types.type_args.get(callee).cloned() else {
            // まとまりの中の単相の参照（再帰）は型引数を具体化しない．鍵はそのまま通じる．
            return ev;
        };
        let mut bindings = Vec::new();
        for tag in tags {
            let inner = self.tag_key(&tag, Some(def));
            let instance = tag.map_args(|t| emela_types::substitute_bound(t, &args));
            let outer = self.key_here(&instance);
            if inner == outer {
                continue;
            }
            if let Some(h) = self.lookup_effect(&outer) {
                bindings.push((self.effect_id(&inner), h));
            }
        }
        if bindings.is_empty() {
            return ev;
        }
        Expr::Provide {
            base: match ev {
                Expr::Lit(Lit::Unit) => None,
                ev => Some(Box::new(ev)),
            },
            bindings,
        }
    }

    /// 同梱の core の `State.run`（8.8）か．
    fn is_state_run(&self, def: DefId) -> bool {
        let data = &self.hir.defs[def];
        let module = match &data.module {
            DefModule::Source(m) => self.modules[*m].name.to_string(),
            _ => return false,
        };
        crate::intrinsics::is_runtime_handler(&module, &data.name)
    }

    /// `State.run` の呼び出しを写す準備．`callee` は `State.run` の参照の式．
    fn state_run(&mut self, callee: ExprId, span: Span) -> Result<StateRun, Expr> {
        let Ty::Fn(f) = self.expr_ty(callee).clone() else {
            return Err(self.unsupported("State.run", span));
        };
        let Some(Ty::Fn(body)) = f.params.get(1) else {
            return Err(self.unsupported("State.run", span));
        };
        // body の R の State[S]．State.run と同じモジュールの State のエフェクト．
        let module = &self.hir.defs[self.hir_def_of(callee)].module;
        let Some(tag) = body
            .uses
            .tags()
            .find(|t| {
                let d = &self.hir.defs[t.def];
                d.kind == DefKind::Effect && d.name == "State" && d.module == *module
            })
            .cloned()
        else {
            return Err(self.unsupported("State.run", span));
        };
        let effect = self.effect_id(&self.key_here(&tag));
        let index = |name: &str| {
            self.module.effects[effect]
                .ops
                .iter()
                .position(|o| o.name == name)
        };
        let (Some(get), Some(set), Some(update)) = (index("get"), index("set"), index("update"))
        else {
            return Err(self.unsupported("State.run", span));
        };
        let call = Call {
            fails: !body.fails.is_empty(),
            suspends: self.row_suspends(&body.uses),
        };
        let base = self.ev.map(Expr::Var);
        Ok(StateRun {
            effect,
            ops: [get, set, update],
            base,
            call,
            cell: self.module.local("cell"),
            handler: self.module.local("state"),
            value: self.module.local("value"),
            f: self.module.local("f"),
            result: self.module.local("result"),
        })
    }

    /// 参照の式の定義．
    fn hir_def_of(&self, expr: ExprId) -> DefId {
        match self.hir.exprs[expr].kind {
            ExprKind::Path(Res::Def(def)) => def,
            _ => unreachable!("定義の参照"),
        }
    }

    /// 関数 `f` を直接呼ぶときの印．中断しうるかは呼ぶ関数の印と同じにする（`Expr::Call`）．
    fn direct_call(&self, f: FnId, call: Call) -> Call {
        Call {
            suspends: self.module.functions[f].suspends,
            ..call
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
        // 構成子の表にはすべての構成子（状態のある handler のものも）がある．
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
                    callee: Callee::Direct(self.fn_id(*def, 0)),
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

/// 状態のあるハンドラか（フィールド，init，release のどれかを持つ，8.4，8.6）．
fn is_stateful(h: &emela_resolve::hir::HandlerItem) -> bool {
    !h.fields.is_empty() || h.init.is_some() || h.release.is_some()
}

/// 中断しない関数 `f` の直接の呼び出し．
fn direct(f: FnId, args: Vec<Expr>, fails: bool) -> Expr {
    Expr::Call {
        callee: Callee::Direct(f),
        args,
        fails,
        suspends: false,
    }
}

/// 関数 `f` の直接の呼び出し．中断しうるかは関数の印と同じにする．
fn direct_marked(m: &Module, f: FnId, args: Vec<Expr>, fails: bool) -> Expr {
    Expr::Call {
        callee: Callee::Direct(f),
        args,
        fails,
        suspends: m.functions[f].suspends,
    }
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

/// 型の中に型引数（`Ty::Bound`）があるか．
fn has_bound(ty: &Ty) -> bool {
    let mut found = matches!(ty, Ty::Bound(_));
    ty.for_each_child(|c| found |= has_bound(c));
    found
}
