//! Snapshot tests for kinds of handlers: built-in handlers, handlers with type parameters, handler
//! members, handlers of several effects, and the calls an operation's implementation makes after
//! resume.

mod builtin;
mod generic;
mod member;
mod multi_effect;
mod resume;
