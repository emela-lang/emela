//! The warning for `[pome].license` of the Pome being built (spec `[pome.manifest]`, W0205).
//!
//! emela-pome reads `license` for every Pome and treats a value that is not an SPDX license
//! expression as no license. Only the Pome being built reports it, at the value, so the check is
//! here, where `Pome.toml` can be added to the sources for its position.

use std::path::Path;

use line_index::{TextRange, TextSize};
use serde::Deserialize;
use toml::Spanned;

use crate::code;
use crate::diagnostic::{Diagnostic, Location, Span};
use crate::source::SourceDb;

const HELP: &str = "write an SPDX license expression, combining license identifiers with `AND`, `OR` and `WITH`, as in `MIT OR Apache-2.0`";

#[derive(Deserialize)]
struct File {
    pome: Option<PomeSection>,
}

#[derive(Deserialize)]
struct PomeSection {
    license: Option<Spanned<toml::Value>>,
}

/// W0205 for `[pome].license` in `text`, the `Pome.toml` at `path`, when it is not a string or not
/// an SPDX license expression. `Pome.toml` is added to `sources` only then. A file that is not
/// TOML, or whose `[pome]` is not a table, is left to the stages that read the rest of it.
pub(crate) fn check(text: &str, path: &Path, sources: &mut SourceDb) -> Option<Diagnostic> {
    let license = toml::from_str::<File>(text).ok()?.pome?.license?;
    let message = match license.get_ref() {
        toml::Value::String(s) if emela_pome::is_license_expression(s) => return None,
        toml::Value::String(s) => format!("`{s}` is not an SPDX license expression"),
        _ => "`[pome].license` is not a string".to_string(),
    };
    let range = license.span();
    let file = sources.add(path, text);
    let range = TextRange::new(
        TextSize::new(range.start as u32),
        TextSize::new(range.end as u32),
    );
    Some(
        Diagnostic::warning(message)
            .with_code(code::LICENSE_NOT_AN_EXPRESSION)
            .with_help(HELP)
            .at(Location::Span(Span::new(file, range))),
    )
}
