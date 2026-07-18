//! Qualified-name resolution for top-level functions (spec 0037).
//!
//! After import expansion, every top-level function carries the qualifier it is
//! addressed by (`Function::module_path`): the written import path for a
//! function of an imported module, `[EffectName]` for an effect operation, and
//! empty for the compilation root's own functions. Its *full path* is
//! `module_path + [name]`.
//!
//! A bare name resolves only within the referring module (spec 0037 R3); a
//! public function of an imported module is called by any non-empty suffix of
//! its full path ending at `name` (`list.map`, `std.list.map`); an effect
//! operation is called `Io.print` from inside a `uses { Io }` scope. Private
//! functions never resolve across module boundaries (R5).
//!
//! [`FnTable`] indexes every function by all such suffixes so a call/reference
//! path resolves to exactly one function, errors as ambiguous when several
//! match, or is unknown when none match. The same table drives both the type
//! checker (which needs the resolved signature) and lowering (which needs the
//! backend emit name), so the two passes never disagree on what a path denotes.

use std::collections::HashMap;

use crate::ast::Program;

/// One resolvable top-level function and the path it can be called by.
pub(crate) struct FnEntry {
    /// Index into `Program::functions` (entries are built in that order, so
    /// `entries[i].index == i`).
    pub(crate) index: usize,
    /// The bare function name (the last path segment).
    pub(crate) name: String,
    /// The full qualified path: `module_path + [name]`. For a compilation-root
    /// function this is just `[name]`. The module a reference resolves against
    /// compares against `full_path` minus the name (see [`FnTable::resolve_in`]).
    pub(crate) full_path: Vec<String>,
    /// Whether the function is generic (spec 0014).
    pub(crate) is_generic: bool,
    /// Whether the function is `pub` (spec 0010). Private functions resolve
    /// only from their own module (spec 0037 R5).
    pub(crate) is_public: bool,
    /// `Some(effect)` for an operation of an `effect` block (spec 0037). An
    /// operation is callable as `Effect.op` (gated on the caller's `uses` row
    /// by the type checker); a bare name never resolves to one except from a
    /// sibling operation of the same effect.
    pub(crate) effect_name: Option<String>,
    /// The backend symbol name: the bare name for a compilation-root function,
    /// otherwise the mangled full path (`std__list__map`), so same-named
    /// functions from different modules coexist (spec 0037 Compilation Notes).
    pub(crate) emit_name: String,
}

/// The outcome of resolving a path against the table.
pub(crate) enum Resolved<'a> {
    /// No function matches the path.
    None,
    /// Exactly one function matches.
    One(&'a FnEntry),
    /// Several functions match — the call site must qualify further (spec 0037
    /// R4). Carries the candidates so the diagnostic can list them.
    Ambiguous(Vec<&'a FnEntry>),
    /// A bare name matched only an effect operation (spec 0037), which is
    /// callable only as `Effect.op`. Carries the operation so the diagnostic
    /// can point at the qualified spelling (`Io.print`).
    EffectOpUnqualified(&'a FnEntry),
    /// A bare name matched only imported public functions (spec 0037 R3),
    /// which are callable only in qualified form. Carries the candidates so
    /// the diagnostic can point at the qualified spelling (`list.map`).
    BareImported(Vec<&'a FnEntry>),
    /// A qualified path matched only functions that are private to another
    /// module (spec 0037 R5).
    Private(Vec<&'a FnEntry>),
}

pub(crate) struct FnTable {
    entries: Vec<FnEntry>,
    /// Every non-empty suffix of every function's full path → matching entry
    /// indices.
    by_suffix: HashMap<Vec<String>, Vec<usize>>,
}

impl FnTable {
    pub(crate) fn build(program: &Program) -> FnTable {
        let mut entries = Vec::with_capacity(program.functions.len());
        for (index, function) in program.functions.iter().enumerate() {
            let mut full_path = function.module_path.clone();
            full_path.push(function.name.clone());
            // Every non-root function is mangled to its full path (spec 0037
            // Compilation Notes), so same-named functions from different
            // modules never collide in the backend. Root functions keep their
            // bare symbol (`main` stays `main`).
            let emit_name = if function.module_path.is_empty() {
                function.name.clone()
            } else {
                full_path.join("__")
            };
            entries.push(FnEntry {
                index,
                name: function.name.clone(),
                full_path,
                is_generic: !function.type_params.is_empty(),
                is_public: function.is_public,
                effect_name: function.effect_name.clone(),
                emit_name,
            });
        }

        let mut by_suffix: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            let path = &entry.full_path;
            for start in 0..path.len() {
                by_suffix.entry(path[start..].to_vec()).or_default().push(i);
            }
        }

        FnTable { entries, by_suffix }
    }

    /// The backend emit name of the function at `index` in `Program::functions`.
    pub(crate) fn emit_name(&self, index: usize) -> &str {
        &self.entries[index].emit_name
    }

    /// Resolves a bare/qualified path from the compilation root. This is the
    /// module-agnostic form used where there is no enclosing module (e.g.
    /// extern registration).
    pub(crate) fn resolve(&self, path: &[String]) -> Resolved<'_> {
        self.resolve_in(path, &[])
    }

    /// Resolves a (possibly qualified) call/reference path to a function, as
    /// seen from `current_module` (the `module_path` of the function containing
    /// the reference — `[EffectName]` inside an effect operation, `[]` at the
    /// compilation root).
    ///
    /// A bare name resolves only to functions of the referring module (spec
    /// 0037 R3); when it matches something that exists but must be qualified,
    /// the distinct variants ([`Resolved::EffectOpUnqualified`],
    /// [`Resolved::BareImported`]) let the caller name the right spelling. A
    /// qualified path resolves by suffix over public functions (plus the
    /// referring module's own, R5), erring as ambiguous on several matches (R4).
    pub(crate) fn resolve_in(&self, path: &[String], current_module: &[String]) -> Resolved<'_> {
        let Some(indices) = self.by_suffix.get(path) else {
            return Resolved::None;
        };
        if path.len() == 1 {
            // Bare name: only the referring module's own functions (spec 0037
            // R3). This is also what lets sibling operations of one effect
            // call each other by bare name — their shared module is the
            // effect itself.
            let same_module: Vec<&FnEntry> = indices
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|entry| module_of(&entry.full_path) == current_module)
                .collect();
            match same_module.as_slice() {
                [only] => return Resolved::One(only),
                [_, _, ..] => return Resolved::Ambiguous(same_module),
                [] => {}
            }
            // Nothing in the referring module. Whatever else matched is
            // qualified-only; report what kind, so the diagnostic can spell
            // out `Io.print(...)` / `list.map(...)`. Private helpers of other
            // modules stay invisible entirely.
            if let Some(entry) = indices
                .iter()
                .map(|&i| &self.entries[i])
                .find(|entry| entry.effect_name.is_some())
            {
                return Resolved::EffectOpUnqualified(entry);
            }
            let imported: Vec<&FnEntry> = indices
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|entry| entry.is_public)
                .collect();
            if imported.is_empty() {
                Resolved::None
            } else {
                Resolved::BareImported(imported)
            }
        } else {
            // Qualified path: public functions, plus the referring module's
            // own (spec 0037 R5).
            let visible: Vec<&FnEntry> = indices
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|entry| entry.is_public || module_of(&entry.full_path) == current_module)
                .collect();
            match visible.as_slice() {
                [] => {
                    let hidden: Vec<&FnEntry> = indices.iter().map(|&i| &self.entries[i]).collect();
                    if hidden.is_empty() {
                        Resolved::None
                    } else {
                        Resolved::Private(hidden)
                    }
                }
                [only] => Resolved::One(only),
                [_, _, ..] => Resolved::Ambiguous(visible),
            }
        }
    }
}

/// The module path a function's full path sits in: everything but the final
/// name segment. Empty for a compilation-root function; `[EffectName]` for an
/// effect operation.
fn module_of(full_path: &[String]) -> &[String] {
    &full_path[..full_path.len() - 1]
}

/// Renders a candidate's full path for an ambiguity diagnostic, e.g.
/// `std.int.to_string`.
pub(crate) fn display_path(segments: &[String]) -> String {
    segments.join(".")
}
