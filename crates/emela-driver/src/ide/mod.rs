//! The queries of the language server (`[lsp]`): completion, hover, navigation, code
//! actions, hints, semantic tokens, structure and links, and the doc comments they show.

pub(crate) mod actions;
pub(crate) mod complete;
pub(crate) mod documentation;
pub(crate) mod hints;
pub(crate) mod hover;
pub(crate) mod links;
pub(crate) mod nav;
pub(crate) mod signature;
pub(crate) mod structure;
pub(crate) mod tokens;
