//! Core IR の定義．
//!
//! 型付き構文木を脱糖した後の形で，バックエンド（JS / WASM）はこれだけを見る．
//! 名前はすべて ID で持つ．関数は `FnId`，局所変数は `Local`，型は `EnumId`．
//! 表示用の名前（`name` / `hint`）は出力を読みやすくするためだけに使う．

use la_arena::{Arena, Idx};

pub type FnId = Idx<Function>;
pub type EnumId = Idx<EnumDef>;
pub type Local = Idx<LocalData>;

/// 1つのモジュール．
#[derive(Debug, Clone, Default)]
pub struct Module {
    pub enums: Arena<EnumDef>,
    pub functions: Arena<Function>,
    pub locals: Arena<LocalData>,
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
    pub variants: Vec<VariantDef>,
}

#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: VariantFields,
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
    Builtin {
        op: Builtin,
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    /// トップレベル関数を直接呼ぶ．
    Direct(FnId),
    /// クロージャの値を呼ぶ．
    Indirect(Box<Expr>),
}

/// 組み込み関数．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// `String.length`．書記素クラスタの数．
    StringLength,
    /// Prelude の `panic(message)`．defect を起こす．
    Panic,
}

impl Builtin {
    pub fn arity(self) -> usize {
        match self {
            Builtin::StringLength | Builtin::Panic => 1,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrKind {
    Int,
    Int64,
    Float,
    Bool,
    String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// 値を文字列にして埋め込む．この段では基本型だけ（Show は 0.21）．
    Value(Expr, StrKind),
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
