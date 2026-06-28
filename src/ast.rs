use serde::{Deserialize, Serialize};

use crate::error::Span;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Program {
    pub(crate) items: Vec<TopLevelItem>,
}

impl Program {
    pub(crate) fn functions(&self) -> Vec<&Function> {
        self.items
            .iter()
            .filter_map(|item| match item {
                TopLevelItem::Function(function) => Some(function),
                TopLevelItem::Import(_) | TopLevelItem::Struct(_) | TopLevelItem::Enum(_) => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum TopLevelItem {
    Import(ImportDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Function(Function),
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ImportDecl {
    pub(crate) path: Vec<String>,
    pub(crate) name: String,
    pub(crate) origin: ImportOrigin,
    #[serde(skip_serializing)]
    pub(crate) span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum ImportOrigin {
    User,
    Stdlib,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StructDecl {
    pub(crate) name: String,
    #[serde(skip_serializing)]
    pub(crate) name_span: Span,
    pub(crate) type_params: Vec<String>,
    pub(crate) field: StructField,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StructField {
    pub(crate) name: String,
    pub(crate) ty: Type,
    #[serde(skip_serializing)]
    pub(crate) ty_span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EnumDecl {
    pub(crate) name: String,
    #[serde(skip_serializing)]
    pub(crate) name_span: Span,
    pub(crate) type_params: Vec<String>,
    pub(crate) variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EnumVariant {
    pub(crate) name: String,
    #[serde(skip_serializing)]
    pub(crate) name_span: Span,
    pub(crate) payload: Option<Type>,
    #[serde(skip_serializing)]
    pub(crate) payload_span: Option<Span>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Function {
    pub(crate) name: String,
    #[serde(skip_serializing)]
    pub(crate) name_span: Span,
    pub(crate) type_params: Vec<String>,
    pub(crate) params: Vec<FunctionParam>,
    pub(crate) return_annotation: Option<Type>,
    #[serde(skip_serializing)]
    pub(crate) return_annotation_span: Option<Span>,
    pub(crate) requires: Option<Vec<Capability>>,
    pub(crate) body: Block,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FunctionParam {
    pub(crate) name: String,
    #[serde(skip_serializing)]
    pub(crate) name_span: Span,
    pub(crate) ty: Option<Type>,
    #[serde(skip_serializing)]
    pub(crate) ty_span: Option<Span>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Block {
    pub(crate) items: Vec<BlockItem>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum BlockItem {
    Binding {
        name: String,
        ty: Option<Type>,
        #[serde(skip_serializing)]
        ty_span: Option<Span>,
        expr: Expr,
        #[serde(skip_serializing)]
        span: Span,
    },
    Expr(Expr),
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum Expr {
    Int(i32, #[serde(skip_serializing)] Span),
    Bool(bool, #[serde(skip_serializing)] Span),
    String(String, #[serde(skip_serializing)] Span),
    Unit(#[serde(skip_serializing)] Span),
    Var(String, #[serde(skip_serializing)] Span),
    Call {
        name: String,
        type_args: Vec<Type>,
        args: Vec<Expr>,
        #[serde(skip_serializing)]
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        name: String,
        args: Vec<Expr>,
        #[serde(skip_serializing)]
        span: Span,
    },
    FieldAccess {
        receiver: Box<Expr>,
        field: String,
        #[serde(skip_serializing)]
        span: Span,
    },
    StructLiteral {
        name: String,
        field: String,
        value: Box<Expr>,
        #[serde(skip_serializing)]
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        #[serde(skip_serializing)]
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        #[serde(skip_serializing)]
        span: Span,
    },
    Lambda {
        params: Vec<FunctionParam>,
        body: Box<Expr>,
        #[serde(skip_serializing)]
        span: Span,
    },
    Block(Block, #[serde(skip_serializing)] Span),
}

impl Expr {
    pub(crate) fn span(&self) -> &Span {
        match self {
            Expr::Int(_, span)
            | Expr::Bool(_, span)
            | Expr::String(_, span)
            | Expr::Unit(span)
            | Expr::Var(_, span)
            | Expr::Block(_, span) => span,
            Expr::Call { span, .. }
            | Expr::MethodCall { span, .. }
            | Expr::FieldAccess { span, .. }
            | Expr::StructLiteral { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Match { span, .. }
            | Expr::Lambda { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MatchArm {
    pub(crate) pattern: Pattern,
    pub(crate) expr: Expr,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum Pattern {
    Int(i32),
    Bool(bool),
    Unit,
    Var(String),
    Variant {
        name: String,
        payload: Option<Box<Pattern>>,
    },
    Wildcard,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Eq,
    Lt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PrimType {
    I32,
    Bool,
    String,
    Unit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum Type {
    Prim(PrimType),
    Named(String),
    GenericParam(String),
    Apply { name: String, args: Vec<Type> },
    Function(FunctionType),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FunctionType {
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Box<Type>,
    pub(crate) effectful: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
