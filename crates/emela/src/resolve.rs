//! Qualified-name resolution for top-level functions (spec 0018).
//!
//! After import expansion, every top-level function carries the import qualifier
//! it was brought in under (`Function::module_path`). Its *full path* is
//! `module_path + [name]`, and it may be called by any non-empty suffix of that
//! path ending at `name` — bare `f`, `mod.f`, `pkg.mod.f`, etc.
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
    /// function or a module-private helper this is just `[name]`. The module a
    /// reference resolves against compares against `full_path` minus the name
    /// (see [`FnTable::resolve_in`]).
    pub(crate) full_path: Vec<String>,
    /// Whether the function is generic (spec 0014).
    pub(crate) is_generic: bool,
    /// Whether the function is an operation of an `effect` block (spec 0036).
    /// An *imported* effect operation is callable only in qualified form
    /// (`io.print`); a bare name never resolves to one (see [`FnTable::resolve`]).
    pub(crate) is_effect_op: bool,
    /// The backend symbol name: the bare name when it is unique across all
    /// top-level functions, otherwise the mangled full path so that same-named
    /// functions from different modules coexist (spec 0018 Compilation Notes).
    pub(crate) emit_name: String,
}

/// The outcome of resolving a path against the table.
pub(crate) enum Resolved<'a> {
    /// No function matches the path.
    None,
    /// Exactly one function matches.
    One(&'a FnEntry),
    /// Several functions match — the call site must qualify further (spec 0018
    /// R5). Carries the candidates so the diagnostic can list them.
    Ambiguous(Vec<&'a FnEntry>),
    /// A bare name matched only an imported effect operation (spec 0036), which
    /// is callable only in qualified form. Carries the operation so the caller
    /// can point at the qualified spelling (`io.print`). Only returned for a
    /// single-segment (bare) path.
    EffectOpUnqualified(&'a FnEntry),
}

pub(crate) struct FnTable {
    entries: Vec<FnEntry>,
    /// Every non-empty suffix of every function's full path → matching entry
    /// indices.
    by_suffix: HashMap<Vec<String>, Vec<usize>>,
}

impl FnTable {
    pub(crate) fn build(program: &Program) -> FnTable {
        // A bare name shared by more than one function collides and must be
        // mangled (for the imported side). Count occurrences first.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for function in &program.functions {
            *counts.entry(function.name.as_str()).or_default() += 1;
        }

        let mut entries = Vec::with_capacity(program.functions.len());
        for (index, function) in program.functions.iter().enumerate() {
            let is_local = function.module_path.is_empty();
            let mut full_path = function.module_path.clone();
            full_path.push(function.name.clone());
            let collides = counts.get(function.name.as_str()).copied().unwrap_or(0) > 1;
            // A unique bare name (or a local function) keeps its bare symbol so
            // single imports such as `std.io.print` emit unchanged. Only a
            // colliding imported function is mangled to its full path.
            let emit_name = if collides && !is_local {
                full_path.join("__")
            } else {
                function.name.clone()
            };
            entries.push(FnEntry {
                index,
                name: function.name.clone(),
                full_path,
                is_generic: !function.type_params.is_empty(),
                is_effect_op: function.is_effect_op,
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

    /// Resolves a bare/qualified path from the compilation root (spec 0018): a
    /// root-local function shadows imports of the same bare name. This is the
    /// module-agnostic form used where there is no enclosing module (e.g. extern
    /// registration).
    pub(crate) fn resolve(&self, path: &[String]) -> Resolved<'_> {
        self.resolve_in(path, &[])
    }

    /// Resolves a (possibly qualified) call/reference path to a function
    /// (spec 0018), as seen from `current_module` (the module path of the
    /// function containing the reference). For a bare name, a function in the
    /// *referring* module shadows imported candidates of the same name (R6);
    /// otherwise a single suffix match resolves and several matches are
    /// ambiguous. Passing `&[]` scopes to the compilation root, so
    /// [`resolve`](Self::resolve) is the `current_module = root` case and keeps
    /// the original root-local shadowing exactly.
    pub(crate) fn resolve_in(&self, path: &[String], current_module: &[String]) -> Resolved<'_> {
        let Some(indices) = self.by_suffix.get(path) else {
            return Resolved::None;
        };
        if path.len() == 1 {
            // Bare name: a function defined in the referring module shadows any
            // imports of the same name, so a module's internal calls resolve to
            // itself even when another imported module exports the same bare
            // name. Only a multi-segment full path can match a longer path, so
            // this special case only applies to bare names.
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
            // No local shadow. An *imported* effect operation (spec 0036) is
            // qualified-only: a bare name must not resolve to one. Exclude effect
            // operations; if that leaves nothing but one existed, report it
            // distinctly so the caller can suggest the `io.op` spelling.
            let visible: Vec<&FnEntry> = indices
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|entry| !entry.is_effect_op)
                .collect();
            match visible.as_slice() {
                [] => {
                    return match indices
                        .iter()
                        .find_map(|&i| self.entries[i].is_effect_op.then_some(&self.entries[i]))
                    {
                        Some(entry) => Resolved::EffectOpUnqualified(entry),
                        None => Resolved::None,
                    };
                }
                [only] => return Resolved::One(only),
                [_, _, ..] => return Resolved::Ambiguous(visible),
            }
        }
        match indices.as_slice() {
            [] => Resolved::None,
            [only] => Resolved::One(&self.entries[*only]),
            many => Resolved::Ambiguous(many.iter().map(|&i| &self.entries[i]).collect()),
        }
    }
}

/// The module path a function's full path sits in: everything but the final
/// name segment. Empty for a compilation-root or module-private function.
fn module_of(full_path: &[String]) -> &[String] {
    &full_path[..full_path.len() - 1]
}

/// Renders a candidate's full path for an ambiguity diagnostic, e.g.
/// `std.int.to_string`.
pub(crate) fn display_path(segments: &[String]) -> String {
    segments.join(".")
}
