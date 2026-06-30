use crate::error::Span;

// The type-system types are part of the IR contract and live in
// `emela-codegen`; the frontend AST re-uses them.
pub(crate) use emela_codegen::{BinaryOp, EffectRow, FunctionType, Type};

#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) module: Option<String>,
    pub(crate) imports: Vec<Import>,
    pub(crate) functions: Vec<Function>,
    pub(crate) externs: Vec<Extern>,
}

/// A platform-function declaration (`extern fn`, spec 0013). It has no body; the
/// backend supplies the implementation. `module` is the declaring file's module
/// path, used with `name` to form the canonical platform name.
#[derive(Debug, Clone)]
pub(crate) struct Extern {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) module: Option<String>,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
    pub(crate) effects: EffectRow,
}

impl Extern {
    /// The canonical platform name, e.g. `io.write_stdout`.
    pub(crate) fn canonical(&self) -> String {
        match &self.module {
            Some(module) if !module.is_empty() => format!("{module}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Import {
    pub(crate) path: Vec<String>,
    pub(crate) span: Span,
}

impl Import {
    pub(crate) fn item_name(&self) -> &str {
        self.path.last().map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) is_public: bool,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
    pub(crate) effects: EffectRow,
    pub(crate) body: Block,
}

#[derive(Debug, Clone)]
pub(crate) struct Param {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) ty: Type,
}

#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub(crate) items: Vec<BlockItem>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum BlockItem {
    Let {
        name: String,
        name_span: Span,
        ty: Option<Type>,
        value: Expr,
    },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Int(i32, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(String, Span),
    Array(Vec<Expr>, Span),
    Unit(Span),
    Var(String, Span),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Fn {
        params: Vec<Param>,
        ret: Type,
        effects: EffectRow,
        body: Block,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Block(Block),
}

impl Expr {
    pub(crate) fn span(&self) -> Span {
        match self {
            Expr::Int(_, span)
            | Expr::Float(_, span)
            | Expr::Bool(_, span)
            | Expr::String(_, span)
            | Expr::Array(_, span)
            | Expr::Unit(span)
            | Expr::Var(_, span) => span.clone(),
            Expr::Call { span, .. } | Expr::Fn { span, .. } | Expr::Binary { span, .. } => {
                span.clone()
            }
            Expr::Block(block) => block.span.clone(),
        }
    }
}
