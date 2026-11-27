//! The declared environment of a program, computed before any body is inferred: type expressions
//! lowered to types and the table of type constructors (`lower`), the declared types of
//! definitions (`signature`), traits and impls (`traits`), the expansion of layers (`layer`),
//! the functions a method call can call (`methods`) and the built-in handlers (`builtin`).

pub(crate) mod builtin;
pub(crate) mod layer;
pub(crate) mod lower;
pub(crate) mod methods;
pub(crate) mod signature;
pub(crate) mod traits;
