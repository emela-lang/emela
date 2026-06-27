#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) functions: Vec<Function>,
}

#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) params: Vec<String>,
    pub(crate) return_annotation: Option<PrimType>,
    pub(crate) requires: Option<Vec<Capability>>,
    pub(crate) body: Block,
}

#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub(crate) items: Vec<BlockItem>,
}

#[derive(Debug, Clone)]
pub(crate) enum BlockItem {
    Binding { name: String, expr: Expr },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Int(i32),
    Bool(bool),
    Unit,
    Var(String),
    Call {
        name: String,
        args: Vec<Expr>,
    },
    MethodCall {
        receiver: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Block(Block),
}

#[derive(Debug, Clone)]
pub(crate) struct MatchArm {
    pub(crate) pattern: Pattern,
    pub(crate) expr: Expr,
}

#[derive(Debug, Clone)]
pub(crate) enum Pattern {
    Int(i32),
    Bool(bool),
    Unit,
    Wildcard,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Eq,
    Lt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimType {
    I32,
    Bool,
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Capability {
    Stdout,
    Stdin,
    Stderr,
    FileRead,
    FileWrite,
    Clock,
    Random,
    Env,
    Process,
    Network,
    HostImport,
}

impl Capability {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "Stdout" => Some(Self::Stdout),
            "Stdin" => Some(Self::Stdin),
            "Stderr" => Some(Self::Stderr),
            "FileRead" => Some(Self::FileRead),
            "FileWrite" => Some(Self::FileWrite),
            "Clock" => Some(Self::Clock),
            "Random" => Some(Self::Random),
            "Env" => Some(Self::Env),
            "Process" => Some(Self::Process),
            "Network" => Some(Self::Network),
            "HostImport" => Some(Self::HostImport),
            _ => None,
        }
    }
}
