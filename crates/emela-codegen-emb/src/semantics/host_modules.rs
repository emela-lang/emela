//! C bindings already carry their resolved symbol. Source module identity does not rename it.
use crate::contract::ir::HostModule;

pub(crate) fn named<'a>(_source: &str, symbol: &'a str) -> &'a str {
    symbol
}
pub(crate) fn file<'a>(_source: &str, symbol: &'a str) -> &'a str {
    symbol
}
pub(crate) fn bundled<'a>(_source: &str, symbol: &'a str) -> &'a str {
    symbol
}

pub(crate) fn symbol<'a>(module: &HostModule, name: &'a str) -> &'a str {
    match module {
        HostModule::Named(source) => named(source, name),
        HostModule::File(source) => file(source, name),
        HostModule::Bundled(source) => bundled(source, name),
    }
}
