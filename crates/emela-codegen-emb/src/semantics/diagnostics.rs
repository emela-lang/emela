//! Stable backend diagnostics and names used in unsupported-input errors.
use crate::contract::ir::{Term, Value};
use crate::{Error, ErrorKind};

pub(crate) fn invalid_flow_ir(function: &str, reason: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message: format!("invalid Flow IR in function `{function}`: {reason}"),
    }]
}

pub(crate) fn statement_name(statement: &crate::contract::ir::Stmt) -> &'static str {
    match statement {
        crate::contract::ir::Stmt::Let { .. } => "Stmt::Let",
        crate::contract::ir::Stmt::If { .. } => "Stmt::If",
        crate::contract::ir::Stmt::Switch { .. } => "Stmt::Switch",
        crate::contract::ir::Stmt::Loop { .. } => "Stmt::Loop",
        crate::contract::ir::Stmt::Block { .. } => "Stmt::Block",
        crate::contract::ir::Stmt::Protect { .. } => "Stmt::Protect",
        crate::contract::ir::Stmt::Dup(_) => "Stmt::Dup",
        crate::contract::ir::Stmt::Drop(_) => "Stmt::Drop",
    }
}

pub(crate) fn value_name(value: &Value) -> &'static str {
    match value {
        Value::Const(_) => "Value::Const",
        Value::Prim { .. } => "Value::Prim",
        Value::Call { .. } => "Value::Call",
        Value::Builtin { .. } => "Value::Builtin",
        Value::Make { .. } => "Value::Make",
        Value::Field { .. } => "Value::Field",
        Value::Tag(_) => "Value::Tag",
        Value::Tuple(_) => "Value::Tuple",
        Value::TupleGet { .. } => "Value::TupleGet",
        Value::List { .. } => "Value::List",
        Value::ListIsEmpty(_) => "Value::ListIsEmpty",
        Value::ListHead(_) => "Value::ListHead",
        Value::ListTail(_) => "Value::ListTail",
        Value::FnRef(_) => "Value::FnRef",
        Value::Closure(_) => "Value::Closure",
        Value::MakeClosure { .. } => "Value::MakeClosure",
        Value::EnvGet { .. } => "Value::EnvGet",
        Value::EvLookup { .. } => "Value::EvLookup",
        Value::EvExtend { .. } => "Value::EvExtend",
        Value::Handler { .. } => "Value::Handler",
        Value::Fiber { .. } => "Value::Fiber",
        Value::IsError { .. } => "Value::IsError",
        Value::AsError { .. } => "Value::AsError",
        Value::Runtime { .. } => "Value::Runtime",
        Value::Coerce { .. } => "Value::Coerce",
    }
}

pub(crate) fn term_name(term: &Term) -> &'static str {
    match term {
        Term::Yield(_) => "Term::Yield",
        Term::Return(_) => "Term::Return",
        Term::Fail(_) => "Term::Fail",
        Term::Break { .. } => "Term::Break",
        Term::Recur(_) => "Term::Recur",
        Term::Defect(_) => "Term::Defect",
        Term::Unwind => "Term::Unwind",
        Term::Unreachable => "Term::Unreachable",
    }
}

pub(crate) fn unsupported(function: &str, construct: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::Unsupported,
        message: format!(
            "embedded C codegen does not support {construct} in function `{function}` yet"
        ),
    }]
}
