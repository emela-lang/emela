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

/// 1つのモジュール．
#[derive(Debug, Clone, Default)]
pub struct Module {
    pub enums: Arena<EnumDef>,
    pub functions: Arena<Function>,
    pub locals: Arena<LocalData>,
    /// Prelude の `Option` を入れたなら，その ID（`option_enum`）．
    pub option: Option<EnumId>,
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
        })
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
}

/// enum の定義．`type` は構成子1つの enum として持つ．
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    /// 型引数の数．フィールドの型の `Type::Param(i)` が `i` 番目を指す．
    pub params: usize,
    pub variants: Vec<VariantDef>,
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
    Call {
        callee: Callee,
        args: Vec<Expr>,
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
    Lambda {
        params: Vec<Local>,
        body: Box<Expr>,
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
