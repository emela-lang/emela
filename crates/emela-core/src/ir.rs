//! Core IR の定義．
//!
//! 型付き構文木を脱糖した後の形で，バックエンド（JS / WASM）はこれだけを見る．
//! 名前はすべて ID で持つ．関数は `FnId`，局所変数は `Local`，型は `EnumId`．
//! 表示用の名前（`name` / `hint`）は出力を読みやすくするためだけに使う．

use la_arena::{Arena, Idx};

use crate::intrinsics::Builtin;

pub type FnId = Idx<Function>;
pub type EnumId = Idx<EnumDef>;
pub type Local = Idx<LocalData>;
pub type EffectId = Idx<EffectDef>;

/// 1つのモジュール．
#[derive(Debug, Clone, Default)]
pub struct Module {
    pub enums: Arena<EnumDef>,
    pub functions: Arena<Function>,
    pub locals: Arena<LocalData>,
    /// エフェクト（8.1）．証拠の表の鍵とハンドラの操作の名前に使う．
    pub effects: Arena<EffectDef>,
    /// Prelude の `Option` を入れたなら，その ID（`option_enum`）．
    pub option: Option<EnumId>,
    /// 出力先の [`crate::CORE_JS_DIR`] の下に書き出す，同梱の JS のファイル．外部関数が同梱の
    /// ファイルを指すときだけ入る．
    pub js_files: Vec<JsFile>,
}

/// 出力に添える JS のファイル．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsFile {
    /// [`crate::CORE_JS_DIR`] からの相対パス（`/` 区切り）．
    pub path: String,
    pub contents: String,
}

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    /// 新しい局所変数を作る．`hint` は出力での名前の手がかりで，一意でなくてよい．
    pub fn local(&mut self, hint: &str) -> Local {
        let hint = if hint.is_empty() { "v" } else { hint };
        self.locals.alloc(LocalData {
            hint: hint.to_owned(),
        })
    }

    /// 関数を宣言する．本体は `define` で後から入れる（相互再帰のため）．
    /// `name` はモジュール内で一意でなければならない．
    pub fn declare(&mut self, name: &str, exported: bool) -> FnId {
        debug_assert!(
            self.functions.iter().all(|(_, f)| f.name != name),
            "関数名 `{name}` が重複している"
        );
        self.functions.alloc(Function {
            name: name.to_owned(),
            exported,
            params: Vec::new(),
            body: Expr::Lit(Lit::Unit),
            external: None,
            suspends: false,
        })
    }

    /// 関数を外部関数（12章）にする．本体は使わない．
    pub fn define_external(&mut self, f: FnId, params: Vec<Local>, external: External) {
        let func = &mut self.functions[f];
        func.params = params;
        func.external = Some(external);
    }

    /// エフェクトを足す．`name` はモジュール内で一意でなければならない．
    pub fn add_effect(&mut self, def: EffectDef) -> EffectId {
        debug_assert!(
            self.effects.iter().all(|(_, e)| e.name != def.name),
            "エフェクト名 `{}` が重複している",
            def.name
        );
        self.effects.alloc(def)
    }

    pub fn define(&mut self, f: FnId, params: Vec<Local>, body: Expr) {
        let func = &mut self.functions[f];
        func.params = params;
        func.body = body;
    }

    pub fn add_enum(&mut self, def: EnumDef) -> EnumId {
        self.enums.alloc(def)
    }

    /// Prelude の `Option[A]`（`Some(A)` / `None` の順）を入れて ID を返す．2回目以降は同じ ID を返す．
    ///
    /// `List.get` や `Int.checked_add` のように Option を返す組み込み関数と，利用者の書く
    /// `Some` / `None` が同じ定義を指すように，モジュールごとに1つだけ持つ．
    pub fn option_enum(&mut self) -> EnumId {
        if let Some(id) = self.option {
            return id;
        }
        let id = self.enums.alloc(EnumDef::option());
        self.option = Some(id);
        id
    }

    /// `Some`．
    pub fn some_ctor(&mut self) -> CtorRef {
        CtorRef {
            enum_id: self.option_enum(),
            variant: OPTION_SOME,
        }
    }

    /// `None`．
    pub fn none_ctor(&mut self) -> CtorRef {
        CtorRef {
            enum_id: self.option_enum(),
            variant: OPTION_NONE,
        }
    }

    pub fn variant(&self, ctor: CtorRef) -> &VariantDef {
        &self.enums[ctor.enum_id].variants[ctor.variant]
    }
}

#[derive(Debug, Clone)]
pub struct LocalData {
    pub hint: String,
}

/// トップレベルの関数．
#[derive(Debug, Clone)]
pub struct Function {
    /// モジュール内で一意な名前．公開するときはこの名前で出す．
    pub name: String,
    pub exported: bool,
    pub params: Vec<Local>,
    pub body: Expr,
    /// 外部関数（12章）なら，ホストの関数の指定．そのとき `body` は使わない．
    pub external: Option<External>,
    /// 中断しうるか（8.1，16.5）．真なら，本体に中断しうる呼び出し（`Expr::Call::suspends`）を
    /// 書ける．外部関数なら `@external suspend fn`（12.2）で，ホストの関数は完了を待つ値
    /// （JS では Promise）を返す．偽の関数の出力は中断を知らない．
    pub suspends: bool,
}

/// 外部関数のホストの関数の指定（12.1）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    /// ターゲットごとの `@external` の引数．書かれた順．
    pub bindings: Vec<ExternalBinding>,
    /// 引数の境界での扱い（12.4，12.5）．`Function::params` と同じ順．
    pub params: Vec<Boundary>,
    /// 戻り値の境界での扱い．
    pub ret: Boundary,
}

impl External {
    /// ターゲット `target`（`js`，`wasm`）の指定．なければ `None`．同じターゲットが2つあれば先のもの．
    pub fn binding(&self, target: &str) -> Option<&ExternalBinding> {
        self.bindings.iter().find(|b| b.target == target)
    }
}

/// `@external(js, "./http_ffi.mjs", "fetchText")` の1つ．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalBinding {
    pub target: String,
    /// 残りの引数．JS では1つならグローバルの参照（`"console.log"`），2つならモジュールと名前．
    /// ファイルを指すモジュール（`./` か `../` で始まる）は，lowering が宣言を含むソースの
    /// ディレクトリからの絶対パスに直してある（12.1）．
    pub args: Vec<String>,
}

/// 外部関数の引数と戻り値の，境界での扱い（12.4，12.5）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Boundary {
    /// ホストから受け取るときに，対になっていないサロゲートを置換文字に直す．
    String,
    /// ホストの返した値を捨てて `()` にする．
    Unit,
    /// コールバック．ホストから呼ばれる引数と，ホストへ返す値を同じ規則で写す．
    Fn {
        params: Vec<Boundary>,
        ret: Box<Boundary>,
    },
    /// そのまま渡す．
    Plain,
}

/// エフェクト（8.1）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDef {
    /// モジュール内で一意な名前．証拠の表の鍵に使う．
    pub name: String,
    /// 操作．宣言の順．`Callee::Op::op` と `Expr::Handler::ops` の添字．
    pub ops: Vec<OpDef>,
}

/// エフェクトの操作（8.1）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpDef {
    pub name: String,
    /// `suspend fn` か．真なら，ハンドラの実装（`Expr::Handler::ops` のラムダ）は最後の引数に
    /// resume を受け取り，操作の呼び出し（`Callee::Op`）は中断しうる呼び出しになる（8.4，14.5）．
    pub suspend: bool,
}

impl OpDef {
    /// `fn` の操作．
    pub fn new(name: &str) -> Self {
        OpDef {
            name: name.to_owned(),
            suspend: false,
        }
    }

    /// `suspend fn` の操作．
    pub fn suspend(name: &str) -> Self {
        OpDef {
            name: name.to_owned(),
            suspend: true,
        }
    }
}

/// enum の定義．`type` は構成子1つの enum として持つ．
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    /// 型引数の数．フィールドの型の `Type::Param(i)` が `i` 番目を指す．
    pub params: usize,
    pub variants: Vec<VariantDef>,
    /// error の宣言（7.1）か．構成子は1つで型引数を持たない．`Expr::Fail` に渡せて，
    /// `Expr::Escape` の腕のパターンに書ける．バックエンドは実行時にどのエラーかを見分けられる
    /// 表現にする．
    pub error: bool,
}

/// Prelude の `Option` の `Some` の添字．
pub const OPTION_SOME: usize = 0;
/// Prelude の `Option` の `None` の添字．
pub const OPTION_NONE: usize = 1;

impl EnumDef {
    /// Prelude の `enum Option[A] { Some(A), None }`．
    pub fn option() -> Self {
        EnumDef {
            name: "Option".to_owned(),
            params: 1,
            variants: vec![
                VariantDef::positional("Some", vec![Type::Param(0)]),
                VariantDef::unit("None"),
            ],
            error: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: VariantFields,
    /// フィールドの型．`fields` と同じ順に並ぶ．表示関数（Show の仮実装）を型から作るのに使う．
    pub tys: Vec<Type>,
}

impl VariantDef {
    pub fn unit(name: &str) -> Self {
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Unit,
            tys: Vec::new(),
        }
    }

    pub fn positional(name: &str, tys: Vec<Type>) -> Self {
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Positional(tys.len()),
            tys,
        }
    }

    pub fn named(name: &str, fields: Vec<(&str, Type)>) -> Self {
        let (names, tys) = fields.into_iter().map(|(n, t)| (n.to_owned(), t)).unzip();
        VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Named(names),
            tys,
        }
    }
}

/// IR の値の型．表示（Show の仮実装）と，型で振る舞いの変わる組み込み関数に使う．
///
/// 型検査の `emela_types::Ty` を，IR の名前（`EnumId`）で書き直したもの．型変数は持たない．
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    Int64,
    Float,
    Bool,
    String,
    Unit,
    Never,
    /// 要素は2つ以上．
    Tuple(Vec<Type>),
    List(Box<Type>),
    /// enum / type の適用．引数の数は `EnumDef::params`．
    Enum(EnumId, Vec<Type>),
    /// 囲む `EnumDef` の型引数．フィールドの型の中にだけ現れる．
    Param(usize),
    /// 関数型．表示できない（10.6）ので中身は持たない．
    Fn,
}

impl Type {
    pub fn list(elem: Type) -> Type {
        Type::List(Box::new(elem))
    }

    /// `Type::Param(i)` を `args[i]` に置き換える．
    pub fn subst(&self, args: &[Type]) -> Type {
        match self {
            Type::Param(i) => args[*i].clone(),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| t.subst(args)).collect()),
            Type::List(t) => Type::list(t.subst(args)),
            Type::Enum(id, ts) => Type::Enum(*id, ts.iter().map(|t| t.subst(args)).collect()),
            _ => self.clone(),
        }
    }
}

/// バリアントの3形式．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantFields {
    /// `Red`
    Unit,
    /// `Some(T)`．値は個数．
    Positional(usize),
    /// `Point { x: Int, y: Int }`．定義の順に並ぶ．
    Named(Vec<String>),
}

impl VariantFields {
    pub fn len(&self) -> usize {
        match self {
            VariantFields::Unit => 0,
            VariantFields::Positional(n) => *n,
            VariantFields::Named(names) => names.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 構成子の参照．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CtorRef {
    pub enum_id: EnumId,
    /// `EnumDef::variants` の添字．
    pub variant: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    /// 32bit 符号付き．
    Int(i32),
    Int64(i64),
    /// binary64．
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Lit),
    Var(Local),
    /// トップレベル関数を値として使う．
    Fn(FnId),
    /// `let var = value; body`．
    Let {
        var: Local,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// 引数は位置の順に並ぶ．名前付き引数の並べ替えは `named::call_in_written_order` を使う．
    /// `fails` は呼ばれる側が失敗しうるか（関数型の E が空でない，7.2）．偽なら呼び出しの結果を
    /// 確かめなくてよい．真でも失敗するとは限らない．
    ///
    /// `suspends` は呼び出しが中断しうるか（8.1，14.5）．真の呼び出しは，中断しうる関数
    /// （`Function::suspends`，`Lambda::suspends`）の本体の中にしか書けない．呼ばれる側ごとの決まり:
    ///
    /// - `Callee::Direct`: 呼ばれる関数の `Function::suspends` と同じでなければならない．
    /// - `Callee::Indirect`: 呼ばれる側の式の型の R が中断しうるなら真．真のとき，実際に呼ばれる
    ///   関数の値は中断しうるものでもしないものでもよい．偽のときは中断しない関数の値でなければならない．
    /// - `Callee::Op`: suspend fn の操作（`OpDef::suspend`）なら真でなければならない．fn の操作で
    ///   真なら，`Indirect` と同じく実装が中断しうるものでもしないものでもよい．
    Call {
        callee: Callee,
        args: Vec<Expr>,
        fails: bool,
        suspends: bool,
    },
    /// 組み込み関数（`intrinsics`）の呼び出し．`ty_args` は型引数の具体的な型で，
    /// `dbg` のように型で振る舞いの変わるものだけが使う（数は `Builtin::info().ty_params`）．
    Builtin {
        op: Builtin,
        ty_args: Vec<Type>,
        args: Vec<Expr>,
    },
    /// 構成子の構築．引数はフィールドの定義順．
    Ctor {
        ctor: CtorRef,
        args: Vec<Expr>,
    },
    /// フィールドの参照．`base` は `ctor` の値でなければならない（`type` の値を想定）．
    Field {
        base: Box<Expr>,
        ctor: CtorRef,
        index: usize,
    },
    /// 要素2つ以上のタプル．`()` は `Lit::Unit`．
    Tuple(Vec<Expr>),
    /// `[a, b]` または `[a, b, ..tail]`．
    List {
        elems: Vec<Expr>,
        tail: Option<Box<Expr>>,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    /// 対象を1回だけ評価し，腕を上から試す．どの腕にも合わなければ defect．
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<Arm>,
    },
    Binary {
        op: BinOp,
        ty: OpTy,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnOp,
        ty: OpTy,
        operand: Box<Expr>,
    },
    /// ラムダ．自由変数はそのまま捕捉する（capture set は WASM の段で求める）．
    /// `suspends` は `Function::suspends` と同じ意味．
    Lambda {
        params: Vec<Local>,
        body: Box<Expr>,
        suspends: bool,
    },
    /// 文字列の補間 `"a#{x}b"`．部品を左から順に評価して連結する．
    Concat(Vec<StrPart>),
    /// 自己末尾呼び出しのループ化（`tail::loopify`）で作る．
    /// `vars` を `inits` で初期化して `body` を評価する．`vars` は回ごとに新しく束縛する．
    Loop {
        vars: Vec<Local>,
        inits: Vec<Expr>,
        body: Box<Expr>,
    },
    /// 囲む `Loop` の次の回へ進む．`Loop` の本体の末尾位置にだけ現れる．
    Recur(Vec<Expr>),
    /// `assert 式`（仕様 13.2）．値は `()`．偽なら defect．
    Assert(Box<Assert>),
    /// `fail e`（7.2）．`e` を評価して，その値（error の enum の値）で現在の計算を終わらせる．
    /// 失敗は外側へ順に伝わり，最も内側の受ける `Escape` か，関数の呼び出し元へ達する．型は Never．
    Fail(Box<Expr>),
    /// `body escape { arms }`（7.3）．`body` の評価中に起きた失敗を受ける．
    ///
    /// 腕のパターンは error の構成子（`Pat::Ctor`）．失敗したエラーと同じ error の腕を上から試し，
    /// パターンとガードが合った最初の腕の本体を escape の値にする．どの腕にも書かれていない error は
    /// そのまま外側へ伝わる．腕に書かれた error なのにどの腕にも合わなければ defect（match と同じ）．
    /// 腕の中で起きた失敗は外側へ伝わる．`body` は末尾位置でない（6.8）．
    Escape {
        body: Box<Expr>,
        arms: Vec<Arm>,
    },
    /// 証拠の表 `ctx` から，エフェクトの今のハンドラを引く（`use X`，8.2）．
    ///
    /// 証拠の表は，エフェクトからハンドラの値への表で，R が空でない関数とラムダの最後の引数に
    /// 渡す（evidence passing，16.5）．
    Use {
        ctx: Box<Expr>,
        effect: EffectId,
    },
    /// 証拠の表 `base` に `bindings` を足した新しい表．`base` は変えない．同じエフェクトは後ろが優先．
    /// `base` が `None` なら空の表に足す．
    Provide {
        base: Option<Box<Expr>>,
        bindings: Vec<(EffectId, Expr)>,
    },
    /// 状態を持たないハンドラの値（8.4）．`ops` はエフェクトの操作の宣言の順のラムダ．
    ///
    /// suspend fn の操作（`OpDef::suspend`）のラムダは，宣言の引数の後ろに resume を1つ多く受ける．
    /// resume は値を1つ取って `()` を返す関数の値で，呼ぶと中断していた呼び出し元が再開する（8.4，14.5）．
    /// 高々1回しか呼べず，2回目は defect．`name` はハンドラの名前で，その defect の文面に使う．
    Handler {
        name: String,
        effect: EffectId,
        ops: Vec<Expr>,
    },
    /// `with`（8.3，14.2）．`binds` を順に評価して束縛し（ハンドラの値と証拠の表），`body` を評価する．
    /// `Let` と同じ意味だが，`body` は末尾位置でない（6.8）．
    With {
        binds: Vec<(Local, Expr)>,
        body: Box<Expr>,
    },
}

impl Expr {
    /// 直下の部分式すべてに `f` をかける（パターンは除く）．順序は評価の順とは限らない．
    pub fn for_each_child(&self, f: &mut impl FnMut(&Expr)) {
        match self {
            Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) => {}
            Expr::Let { value, body, .. } => {
                f(value);
                f(body);
            }
            Expr::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct(_) => {}
                    Callee::Indirect(c) | Callee::Op { handler: c, .. } => f(c),
                }
                args.iter().for_each(f);
            }
            Expr::Builtin { args, .. }
            | Expr::Ctor { args, .. }
            | Expr::Tuple(args)
            | Expr::Recur(args)
            | Expr::Handler { ops: args, .. } => args.iter().for_each(f),
            Expr::Field { base, .. } | Expr::Use { ctx: base, .. } => f(base),
            Expr::List { elems, tail } => {
                elems.iter().for_each(&mut *f);
                if let Some(t) = tail {
                    f(t);
                }
            }
            Expr::If { cond, then, else_ } => {
                f(cond);
                f(then);
                f(else_);
            }
            Expr::Match {
                scrutinee: body,
                arms,
            }
            | Expr::Escape { body, arms } => {
                f(body);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        f(g);
                    }
                    f(&arm.body);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            Expr::Unary { operand: e, .. } | Expr::Lambda { body: e, .. } | Expr::Fail(e) => f(e),
            Expr::Concat(parts) => {
                for p in parts {
                    if let StrPart::Value(v, _) = p {
                        f(v);
                    }
                }
            }
            Expr::Loop { inits, body, .. } => {
                inits.iter().for_each(&mut *f);
                f(body);
            }
            Expr::Assert(a) => match &a.cond {
                AssertCond::Compare { lhs, rhs, .. } => {
                    f(lhs);
                    f(rhs);
                }
                AssertCond::Bool(e) => f(e),
            },
            Expr::Provide { base, bindings } => {
                if let Some(b) = base {
                    f(b);
                }
                for (_, h) in bindings {
                    f(h);
                }
            }
            Expr::With { binds, body } => {
                for (_, v) in binds {
                    f(v);
                }
                f(body);
            }
        }
    }
}

/// `assert 式`．
#[derive(Debug, Clone, PartialEq)]
pub struct Assert {
    pub cond: AssertCond,
    /// ソースに書かれた式の字面（`user.name == "alice"`）．失敗の表示に使う．空でもよい．
    pub text: String,
}

/// assert の式の形．コンパイラが式の形を見て，比較なら両辺の値を表示する（13.2）．
#[derive(Debug, Clone, PartialEq)]
pub enum AssertCond {
    /// `lhs op rhs`．`op` は `Eq`，`Ne`，`Lt`，`Le`，`Gt`，`Ge` のどれか．
    /// 両辺を左から1回ずつ評価して比べ，偽なら両辺を表示して defect にする．
    /// `op_ty` は比較の意味（`Expr::Binary` と同じ），`ty` は両辺の型（表示関数を作るのに使う）．
    Compare {
        op: BinOp,
        op_ty: OpTy,
        ty: Type,
        lhs: Expr,
        rhs: Expr,
    },
    /// 比較でない式．式の値だけを見る．
    Bool(Expr),
}

impl Assert {
    /// 比較の assert の式．
    pub fn compare(op: BinOp, op_ty: OpTy, ty: Type, lhs: Expr, rhs: Expr, text: &str) -> Expr {
        Expr::Assert(Box::new(Assert {
            cond: AssertCond::Compare {
                op,
                op_ty,
                ty,
                lhs,
                rhs,
            },
            text: text.to_owned(),
        }))
    }

    /// 比較でない assert の式．
    pub fn bool(cond: Expr, text: &str) -> Expr {
        Expr::Assert(Box::new(Assert {
            cond: AssertCond::Bool(cond),
            text: text.to_owned(),
        }))
    }

    /// 部分式に評価の順で `f` をかける．
    pub fn for_each_operand_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match &mut self.cond {
            AssertCond::Compare { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            AssertCond::Bool(e) => f(e),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    /// トップレベル関数を直接呼ぶ．
    Direct(FnId),
    /// クロージャの値を呼ぶ．
    Indirect(Box<Expr>),
    /// ハンドラの値 `handler` の操作を呼ぶ（`log.info(...)`，8.2）．`op` は
    /// `EffectDef::ops` の添字．証拠の表は渡さない（操作の本体の効果は with で決まっている）．
    Op {
        handler: Box<Expr>,
        effect: EffectId,
        op: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pat: Pat,
    /// パターンが合った後に評価する．
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pat {
    Wild,
    Bind(Local),
    Lit(Lit),
    /// 構成子．`fields` はフィールドの定義順で，省いたフィールドは `Wild` で埋める．
    Ctor {
        ctor: CtorRef,
        fields: Vec<Pat>,
    },
    Tuple(Vec<Pat>),
    /// `[]`，`[x, y]`，`[x, ..rest]`，`[x, ..]`．
    /// `rest` が `None` なら長さがちょうど `elems.len()`．
    List {
        elems: Vec<Pat>,
        rest: Option<Box<Pat>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// 値を文字列にして埋め込む．型から決まる表示関数を使う（Show の仮実装．Trait は 0.21）．
    /// 型は `Type::Param` を含まない具体的な型でなければならない．
    Value(Expr, Type),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// 短絡評価．
    And,
    /// 短絡評価．
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// 演算の対象の型．バックエンドが意味を選ぶのに使う．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpTy {
    Int,
    Int64,
    Float,
    Bool,
    String,
    /// 構造の等値比較（`==` / `!=`）だけに使う．
    Structural,
}
