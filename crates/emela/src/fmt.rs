//! The canonical formatter, `emela fmt` (spec 0035): one style, zero
//! configuration. The formatter works on the token stream (plus the comments
//! collected by `lex_with_comments`), not on the AST — the AST desugars
//! `&& || !` into `if` and drops redundant parentheses, so it cannot
//! reproduce the source. The parsed AST is still used twice: to tell
//! comparison `<`/`>` apart from generic angle brackets, and to verify that
//! the formatted output parses to the identical program before anything is
//! written.
//!
//! The token stream is shaped into a tree of lines by bracket balance alone:
//! newlines split lines (the lexer suppresses them inside `(...)`/`[...]`,
//! spec 0034), `(...)`/`[...]`/the `uses { ... }` row become comma-separated
//! groups, and `{ ... }` becomes a block of nested lines. Rendering is
//! width-aware: a line that exceeds [`MAX_WIDTH`] breaks its groups in a
//! deterministic order (the trailing body block first, then the remaining
//! groups left to right), each broken group putting one element per line
//! with a trailing comma.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{self, BinaryOp};
use crate::error::{Error, Result};
use crate::lexer::{Comment, Token, TokenKind, lex_with_comments};
use crate::parser::parse_program;

/// The maximum line width (spec 0035 F3). A constant, not a setting.
pub(crate) const MAX_WIDTH: usize = 100;

/// One indentation level (spec 0035 F2).
const INDENT: &str = "    ";

// ---------------------------------------------------------------------------
// CLI entry
// ---------------------------------------------------------------------------

/// Formats every `.emel` file under `paths` in place (spec 0035 C1), or, with
/// `check`, reports the files that need formatting without writing (C2).
pub(crate) fn run(paths: &[PathBuf], check: bool) -> Result<()> {
    let mut files = Vec::new();
    for path in paths {
        collect_files(path, &mut files)?;
    }
    files.sort();
    files.dedup();
    let mut changed = 0usize;
    let mut failed = 0usize;
    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("error: failed to read {}: {error}", file.display());
                failed += 1;
                continue;
            }
        };
        match format_source(&file.display().to_string(), &source) {
            Ok(formatted) => {
                if formatted != source {
                    changed += 1;
                    println!("{}", file.display());
                    if !check && let Err(error) = fs::write(file, formatted) {
                        eprintln!("error: failed to write {}: {error}", file.display());
                        failed += 1;
                    }
                }
            }
            // A file that does not lex/parse is reported and skipped; the
            // remaining files are still formatted (spec 0035 C3).
            Err(error) => {
                eprintln!("{error}");
                eprintln!();
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(Error::new(format!(
            "{failed} file(s) could not be formatted"
        )));
    }
    if check && changed > 0 {
        return Err(Error::new(format!("{changed} file(s) need formatting")));
    }
    Ok(())
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_dir() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Skip hidden directories and build output, but never a directory the
        // user named explicitly on the command line.
        let entries = fs::read_dir(path)
            .map_err(|error| Error::new(format!("failed to read {}: {error}", path.display())))?;
        let _ = name;
        let mut children: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        children.sort();
        for child in children {
            let child_name = child.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if child.is_dir() {
                if child_name.starts_with('.') || child_name == "target" {
                    continue;
                }
                collect_files(&child, files)?;
            } else if child_name.ends_with(".emel") {
                files.push(child);
            }
        }
        Ok(())
    } else if path.exists() {
        files.push(path.to_path_buf());
        Ok(())
    } else {
        Err(Error::new(format!(
            "no such file or directory: {}",
            path.display()
        )))
    }
}

// ---------------------------------------------------------------------------
// Formatting pipeline
// ---------------------------------------------------------------------------

/// Formats one source file to its canonical form. Requires only that the file
/// parses — not that it resolves imports or type-checks (spec 0035 C3). The
/// output is verified to parse to the identical program before it is
/// returned (F10); on a mismatch the source is left untouched.
pub(crate) fn format_source(label: &str, source: &str) -> Result<String> {
    // Formatting requires a clean parse (spec 0035 C3); with multi-error
    // collection (spec 0033) the first error is surfaced and the file is left
    // untouched.
    let (program, errors) = parse_program(label, source);
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    let (tokens, comments) = lex_with_comments(label, source)?;
    let comparisons = comparison_offsets(&program, &tokens);
    let lines = normalize_attributes(Builder::new(source, tokens, comments).build_top());
    let printer = Printer {
        src: source,
        comparisons,
    };
    let output = printer.render_program(&lines);
    let (reparsed, reparse_errors) = parse_program(label, &output);
    if let Some(error) = reparse_errors.into_iter().next() {
        return Err(Error::new(format!(
            "internal: emela fmt produced unparsable output for {label}; \
             the file was left unchanged\n\n{}",
            error.render()
        )));
    }
    if ast_dump(&reparsed) != ast_dump(&program) {
        return Err(Error::new(format!(
            "internal: emela fmt would change the meaning of {label}; \
             the file was left unchanged"
        )));
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// Tree building
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum GroupKind {
    /// `( ... )`
    Paren,
    /// `[ ... ]`
    Bracket,
    /// The `uses { ... }` effect row: braces as list delimiters (spec 0034).
    Brace,
}

impl GroupKind {
    fn open(self) -> char {
        match self {
            GroupKind::Paren => '(',
            GroupKind::Bracket => '[',
            GroupKind::Brace => '{',
        }
    }

    fn close(self) -> char {
        match self {
            GroupKind::Paren => ')',
            GroupKind::Bracket => ']',
            GroupKind::Brace => '}',
        }
    }

    fn open_kind(self) -> TokenKind {
        match self {
            GroupKind::Paren => TokenKind::LParen,
            GroupKind::Bracket => TokenKind::LBracket,
            GroupKind::Brace => TokenKind::LBrace,
        }
    }

    fn close_kind(self) -> TokenKind {
        match self {
            GroupKind::Paren => TokenKind::RParen,
            GroupKind::Bracket => TokenKind::RBracket,
            GroupKind::Brace => TokenKind::RBrace,
        }
    }
}

#[derive(Debug)]
enum Atom {
    Tok(Token),
    Group(Group),
    Block(BlockNode),
}

/// A comma-separated bracket group.
#[derive(Debug)]
struct Group {
    kind: GroupKind,
    elements: Vec<Element>,
    /// Comments between the last element and the closer.
    dangling: Vec<Comment>,
}

#[derive(Debug, Default)]
struct Element {
    /// Own-line comments before the element.
    leading: Vec<Comment>,
    atoms: Vec<Atom>,
    /// Comments on the element's line; rendered after the comma.
    trailing: Vec<Comment>,
}

impl Element {
    fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.atoms.is_empty() && self.trailing.is_empty()
    }
}

/// A `{ ... }` block: nested lines.
#[derive(Debug)]
struct BlockNode {
    lines: Vec<Line>,
    /// True when the block spanned multiple source lines. Such a block is
    /// never re-joined onto one line: the statement structure belongs to the
    /// author (spec 0035 F4).
    multi_line: bool,
}

#[derive(Debug)]
struct Line {
    /// A single blank line precedes this line (runs of blanks collapse, F6).
    blank_before: bool,
    /// Empty for an own-line comment line (the comment is in `trailing`).
    atoms: Vec<Atom>,
    trailing: Vec<Comment>,
}

enum Item {
    Tok(Token),
    Com(Comment),
}

impl Item {
    fn start(&self) -> usize {
        match self {
            Item::Tok(token) => token.span.start,
            Item::Com(comment) => comment.span.start,
        }
    }
}

struct Builder<'a> {
    src: &'a str,
    items: Vec<Item>,
    pos: usize,
}

impl<'a> Builder<'a> {
    fn new(src: &'a str, tokens: Vec<Token>, comments: Vec<Comment>) -> Self {
        let mut items: Vec<Item> = tokens
            .into_iter()
            .map(Item::Tok)
            .chain(comments.into_iter().map(Item::Com))
            .collect();
        items.sort_by_key(Item::start);
        Builder { src, items, pos: 0 }
    }

    fn peek(&self) -> Option<&Item> {
        self.items.get(self.pos)
    }

    fn bump(&mut self) -> &Item {
        let item = &self.items[self.pos];
        self.pos += 1;
        item
    }

    fn build_top(&mut self) -> Vec<Line> {
        self.build_lines().0
    }

    /// Builds lines until `Eof` or an unconsumed closing `}`. Returns the
    /// lines and whether a newline was seen directly at this level.
    fn build_lines(&mut self) -> (Vec<Line>, bool) {
        let mut lines: Vec<Line> = Vec::new();
        let mut atoms: Vec<Atom> = Vec::new();
        let mut trailing: Vec<Comment> = Vec::new();
        let mut blank_pending = false;
        let mut newline_run = 0usize;
        let mut saw_newline = false;
        loop {
            let stop = match self.peek() {
                None => true,
                Some(Item::Tok(token)) => {
                    matches!(token.kind, TokenKind::Eof | TokenKind::RBrace)
                }
                Some(Item::Com(_)) => false,
            };
            if stop {
                finalize_line(&mut lines, &mut atoms, &mut trailing, &mut blank_pending);
                return (lines, saw_newline);
            }
            match self.bump() {
                Item::Com(comment) => {
                    let comment = comment.clone();
                    if atoms.is_empty() && trailing.is_empty() {
                        // An own-line comment is a line of its own.
                        lines.push(Line {
                            blank_before: std::mem::take(&mut blank_pending),
                            atoms: Vec::new(),
                            trailing: vec![comment],
                        });
                        newline_run = 0;
                    } else {
                        trailing.push(comment);
                    }
                }
                Item::Tok(token) => {
                    let token = token.clone();
                    match token.kind {
                        TokenKind::Newline => {
                            saw_newline = true;
                            if atoms.is_empty() && trailing.is_empty() {
                                newline_run += 1;
                                // Blanks collapse to one; blanks before the
                                // first line of a block are stripped (F6).
                                if newline_run >= 2 && !lines.is_empty() {
                                    blank_pending = true;
                                }
                            } else {
                                finalize_line(
                                    &mut lines,
                                    &mut atoms,
                                    &mut trailing,
                                    &mut blank_pending,
                                );
                                newline_run = 1;
                            }
                        }
                        TokenKind::LParen => {
                            atoms.push(Atom::Group(self.build_group(GroupKind::Paren)));
                        }
                        TokenKind::LBracket => {
                            atoms.push(Atom::Group(self.build_group(GroupKind::Bracket)));
                        }
                        TokenKind::LBrace => {
                            if ends_with_uses(&atoms) {
                                atoms.push(Atom::Group(self.build_group(GroupKind::Brace)));
                            } else {
                                atoms.push(Atom::Block(self.build_block()));
                            }
                        }
                        _ => atoms.push(Atom::Tok(token)),
                    }
                }
            }
        }
    }

    /// The opening `{` has been consumed; consumes through the matching `}`.
    fn build_block(&mut self) -> BlockNode {
        let (lines, saw_newline) = self.build_lines();
        // Consume the closing `}` (parse succeeded, so brackets balance).
        if let Some(Item::Tok(token)) = self.peek()
            && token.kind == TokenKind::RBrace
        {
            self.pos += 1;
        }
        BlockNode {
            lines,
            multi_line: saw_newline,
        }
    }

    /// The opener has been consumed; consumes through the matching closer.
    /// Elements split at top-level commas. Newline tokens only occur here for
    /// `Brace` groups (the lexer keeps them inside `{}`); they are separators
    /// nowhere in a group, so they are dropped.
    fn build_group(&mut self, kind: GroupKind) -> Group {
        let closer = kind.close_kind();
        let mut elements: Vec<Element> = Vec::new();
        let mut current = Element::default();
        let mut pending_leading: Vec<Comment> = Vec::new();
        let mut last_code_end = 0usize;
        loop {
            let done = match self.peek() {
                None => true,
                Some(Item::Tok(token)) => token.kind == closer || token.kind == TokenKind::Eof,
                Some(Item::Com(_)) => false,
            };
            if done {
                if let Some(Item::Tok(token)) = self.peek()
                    && token.kind == closer
                {
                    self.pos += 1;
                }
                if !current.is_empty() {
                    current.leading.splice(0..0, pending_leading.drain(..));
                    elements.push(current);
                }
                return Group {
                    kind,
                    elements,
                    dangling: pending_leading,
                };
            }
            match self.bump() {
                Item::Com(comment) => {
                    let comment = comment.clone();
                    if !current.atoms.is_empty() {
                        current.trailing.push(comment);
                    } else if let Some(previous) = elements.last_mut() {
                        // A comment right after a comma on the same source
                        // line belongs to the element before the comma.
                        let between = &self.src[last_code_end..comment.span.start];
                        if between.contains('\n') {
                            pending_leading.push(comment);
                        } else {
                            previous.trailing.push(comment);
                        }
                    } else {
                        pending_leading.push(comment);
                    }
                }
                Item::Tok(token) => {
                    let token = token.clone();
                    last_code_end = token.span.end;
                    match token.kind {
                        TokenKind::Newline => {}
                        TokenKind::Comma => {
                            current.leading.splice(0..0, pending_leading.drain(..));
                            elements.push(std::mem::take(&mut current));
                        }
                        TokenKind::LParen => {
                            current
                                .atoms
                                .push(Atom::Group(self.build_group(GroupKind::Paren)));
                        }
                        TokenKind::LBracket => {
                            current
                                .atoms
                                .push(Atom::Group(self.build_group(GroupKind::Bracket)));
                        }
                        TokenKind::LBrace => {
                            if ends_with_uses(&current.atoms) {
                                current
                                    .atoms
                                    .push(Atom::Group(self.build_group(GroupKind::Brace)));
                            } else {
                                current.atoms.push(Atom::Block(self.build_block()));
                            }
                        }
                        _ => current.atoms.push(Atom::Tok(token)),
                    }
                }
            }
        }
    }
}

/// Whether the pending atoms end with the `uses` keyword, i.e. the `{` that
/// follows opens an effect row (a comma list), not a block.
fn ends_with_uses(atoms: &[Atom]) -> bool {
    matches!(
        atoms.last(),
        Some(Atom::Tok(token)) if token.kind == TokenKind::Uses
    )
}

fn finalize_line(
    lines: &mut Vec<Line>,
    atoms: &mut Vec<Atom>,
    trailing: &mut Vec<Comment>,
    blank_pending: &mut bool,
) {
    if atoms.is_empty() && trailing.is_empty() {
        return;
    }
    // A redundant separator comma at the end of a newline-separated line
    // (enum variants, match arms) is dropped: the newline is the canonical
    // separator (spec 0035 F8).
    if matches!(
        atoms.last(),
        Some(Atom::Tok(token)) if token.kind == TokenKind::Comma
    ) {
        atoms.pop();
    }
    lines.push(Line {
        blank_before: std::mem::take(blank_pending),
        atoms: std::mem::take(atoms),
        trailing: std::mem::take(trailing),
    });
}

/// Canonicalizes attribute placement at the top level (spec 0039 R8): each
/// attribute goes on its own line directly above its declaration — an inline
/// `@test fn ...` is split, and a blank line between an attribute and the
/// declaration (or the next attribute) is removed. Attributes only parse at the
/// top level, so nested lines need no treatment.
fn is_attr_atom(atom: &Atom) -> bool {
    matches!(atom, Atom::Tok(token) if matches!(token.kind, TokenKind::At(_)))
}

fn is_paren_atom(atom: &Atom) -> bool {
    matches!(atom, Atom::Group(group) if group.kind == GroupKind::Paren)
}

fn starts_with_attr(line: &Line) -> bool {
    line.atoms.first().is_some_and(is_attr_atom)
}

fn normalize_attributes(lines: Vec<Line>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for mut line in lines {
        // Split leading attributes onto their own lines, keeping each `@name`
        // with its optional `("...")` argument group (spec 0039 R7). The blank
        // (if any) stays above the first attribute; the declaration ends on the
        // final line.
        loop {
            if !starts_with_attr(&line) {
                break;
            }
            let len = if line.atoms.get(1).is_some_and(is_paren_atom) {
                2
            } else {
                1
            };
            // Nothing follows the attribute: it is already on its own line.
            if line.atoms.len() <= len {
                break;
            }
            let atoms: Vec<Atom> = line.atoms.drain(0..len).collect();
            out.push(Line {
                blank_before: std::mem::take(&mut line.blank_before),
                atoms,
                trailing: Vec::new(),
            });
        }
        if out.last().is_some_and(starts_with_attr) {
            line.blank_before = false;
        }
        out.push(line);
    }
    out
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

struct Printer<'a> {
    src: &'a str,
    /// Byte offsets of `<` / `>` tokens that are comparison operators
    /// (spaced); every other `<` / `>` is a generic bracket (tight).
    comparisons: HashSet<usize>,
}

impl Printer<'_> {
    fn render_program(&self, lines: &[Line]) -> String {
        let mut out = String::new();
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
                if line.blank_before {
                    out.push('\n');
                }
            }
            out.push_str(&self.render_line(line, 0));
        }
        // Trailing whitespace is stripped and the file ends with exactly one
        // newline (F11); an empty file stays empty.
        let cleaned: Vec<&str> = out.lines().map(str::trim_end).collect();
        let mut result = cleaned.join("\n");
        while result.ends_with('\n') {
            result.pop();
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result
    }

    /// Renders one line (which may span several output lines once its groups
    /// and blocks break), including the leading indentation.
    fn render_line(&self, line: &Line, indent: usize) -> String {
        if line.atoms.is_empty() {
            // An own-line comment.
            let mut out = pad(indent);
            for (index, comment) in line.trailing.iter().enumerate() {
                if index > 0 {
                    out.push(' ');
                }
                out.push_str(self.comment_text(comment));
            }
            return out;
        }
        let mut out = self.render_atoms_fitted(&line.atoms, indent);
        for comment in &line.trailing {
            out.push(' ');
            out.push_str(self.comment_text(comment));
        }
        out
    }

    /// Renders atoms trying progressively larger break sets until every
    /// output line fits in [`MAX_WIDTH`] (or everything breakable is broken).
    fn render_atoms_fitted(&self, atoms: &[Atom], indent: usize) -> String {
        let breakable = self.breakable_count(atoms);
        let mut last = String::new();
        for broken in 0..=breakable {
            let text = self.render_atoms(atoms, indent, broken);
            if fits(&text) {
                return text;
            }
            last = text;
        }
        last
    }

    fn breakable_count(&self, atoms: &[Atom]) -> usize {
        atoms
            .iter()
            .filter(|atom| match atom {
                Atom::Group(group) => self.flat_group(group).is_some(),
                Atom::Block(block) => self.flat_block(block).is_some(),
                Atom::Tok(_) => false,
            })
            .count()
    }

    /// The order in which a line's atoms break: the trailing body block
    /// first (breaking `fn ... { body }` at the body is almost always
    /// enough), then the remaining groups left to right.
    fn break_order(&self, atoms: &[Atom]) -> Vec<usize> {
        let mut order = Vec::new();
        let breakable = |atom: &Atom| match atom {
            Atom::Group(group) => self.flat_group(group).is_some(),
            Atom::Block(block) => self.flat_block(block).is_some(),
            Atom::Tok(_) => false,
        };
        let last_block = match atoms.last() {
            Some(atom @ Atom::Block(_)) if breakable(atom) => Some(atoms.len() - 1),
            _ => None,
        };
        if let Some(index) = last_block {
            order.push(index);
        }
        for (index, atom) in atoms.iter().enumerate() {
            if Some(index) != last_block && breakable(atom) {
                order.push(index);
            }
        }
        order
    }

    /// Renders atoms with the first `broken` entries of the break order
    /// broken (plus everything that cannot render flat). Includes the leading
    /// indentation of the first line.
    fn render_atoms(&self, atoms: &[Atom], indent: usize, broken: usize) -> String {
        let break_set: HashSet<usize> = self.break_order(atoms)[..broken].iter().copied().collect();
        let mut out = pad(indent);
        let mut previous: Option<(TokenKind, bool)> = None;
        for (index, atom) in atoms.iter().enumerate() {
            let text = match atom {
                Atom::Tok(token) => self.token_text(token).to_string(),
                Atom::Group(group) => match self.flat_group(group) {
                    Some(flat) if !break_set.contains(&index) => flat,
                    _ => self.render_group_broken(group, indent),
                },
                Atom::Block(block) => match self.flat_block(block) {
                    Some(flat) if !break_set.contains(&index) => flat,
                    _ => self.render_block_broken(block, indent),
                },
            };
            let lead = self.lead_kind(atom);
            if let Some(prev) = &previous
                && space_between(&prev.0, prev.1, &lead.0, lead.1)
            {
                out.push(' ');
            }
            out.push_str(&text);
            previous = Some(self.tail_kind(atom));
        }
        out
    }

    fn render_group_broken(&self, group: &Group, indent: usize) -> String {
        let mut out = String::new();
        out.push(group.kind.open());
        for element in &group.elements {
            for comment in &element.leading {
                out.push('\n');
                out.push_str(&pad(indent + 1));
                out.push_str(self.comment_text(comment));
            }
            out.push('\n');
            out.push_str(&self.render_atoms_fitted(&element.atoms, indent + 1));
            out.push(',');
            for comment in &element.trailing {
                out.push(' ');
                out.push_str(self.comment_text(comment));
            }
        }
        for comment in &group.dangling {
            out.push('\n');
            out.push_str(&pad(indent + 1));
            out.push_str(self.comment_text(comment));
        }
        out.push('\n');
        out.push_str(&pad(indent));
        out.push(group.kind.close());
        out
    }

    fn render_block_broken(&self, block: &BlockNode, indent: usize) -> String {
        let mut out = String::new();
        out.push('{');
        if block.lines.is_empty() {
            out.push('}');
            return out;
        }
        for (index, line) in block.lines.iter().enumerate() {
            if index > 0 && line.blank_before {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(&self.render_line(line, indent + 1));
        }
        out.push('\n');
        out.push_str(&pad(indent));
        out.push('}');
        out
    }

    /// The one-line rendering of a group, or `None` when it must break
    /// (it contains a comment or a multi-line block).
    fn flat_group(&self, group: &Group) -> Option<String> {
        if !group.dangling.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for element in &group.elements {
            if !element.leading.is_empty() || !element.trailing.is_empty() {
                return None;
            }
            parts.push(self.flat_atoms(&element.atoms)?);
        }
        let inner = parts.join(", ");
        Some(match group.kind {
            GroupKind::Paren => format!("({inner})"),
            GroupKind::Bracket => format!("[{inner}]"),
            GroupKind::Brace if inner.is_empty() => "{}".to_string(),
            GroupKind::Brace => format!("{{ {inner} }}"),
        })
    }

    /// The one-line rendering of a block, or `None` when it must stay
    /// multi-line (source structure, F4) or contains a comment.
    fn flat_block(&self, block: &BlockNode) -> Option<String> {
        if block.multi_line {
            return None;
        }
        match block.lines.len() {
            0 => Some("{}".to_string()),
            1 => {
                let line = &block.lines[0];
                if !line.trailing.is_empty() || line.atoms.is_empty() {
                    return None;
                }
                Some(format!("{{ {} }}", self.flat_atoms(&line.atoms)?))
            }
            _ => None,
        }
    }

    fn flat_atoms(&self, atoms: &[Atom]) -> Option<String> {
        let mut out = String::new();
        let mut previous: Option<(TokenKind, bool)> = None;
        for atom in atoms {
            let text = match atom {
                Atom::Tok(token) => self.token_text(token).to_string(),
                Atom::Group(group) => self.flat_group(group)?,
                Atom::Block(block) => self.flat_block(block)?,
            };
            let lead = self.lead_kind(atom);
            if let Some(prev) = &previous
                && space_between(&prev.0, prev.1, &lead.0, lead.1)
            {
                out.push(' ');
            }
            out.push_str(&text);
            previous = Some(self.tail_kind(atom));
        }
        Some(out)
    }

    /// The token kind an atom presents to its left neighbour for spacing.
    fn lead_kind(&self, atom: &Atom) -> (TokenKind, bool) {
        match atom {
            Atom::Tok(token) => (token.kind.clone(), self.is_comparison(token)),
            Atom::Group(group) => (group.kind.open_kind(), false),
            Atom::Block(_) => (TokenKind::LBrace, false),
        }
    }

    /// The token kind an atom presents to its right neighbour for spacing.
    fn tail_kind(&self, atom: &Atom) -> (TokenKind, bool) {
        match atom {
            Atom::Tok(token) => (token.kind.clone(), self.is_comparison(token)),
            Atom::Group(group) => (group.kind.close_kind(), false),
            Atom::Block(_) => (TokenKind::RBrace, false),
        }
    }

    fn is_comparison(&self, token: &Token) -> bool {
        matches!(token.kind, TokenKind::Lt | TokenKind::Gt)
            && self.comparisons.contains(&token.span.start)
    }

    /// Every token renders as its exact source text: literals keep their
    /// original spelling (`2.50`, escapes in strings) by construction.
    fn token_text<'s>(&'s self, token: &Token) -> &'s str {
        &self.src[token.span.start..token.span.end]
    }

    fn comment_text<'s>(&'s self, comment: &Comment) -> &'s str {
        self.src[comment.span.start..comment.span.end].trim_end()
    }
}

fn pad(indent: usize) -> String {
    INDENT.repeat(indent)
}

fn fits(text: &str) -> bool {
    text.lines().all(|line| line.chars().count() <= MAX_WIDTH)
}

/// The canonical spacing between two adjacent rendered tokens (spec 0035 F5).
/// `prev_cmp` / `next_cmp` mark `<` / `>` tokens that are comparison
/// operators rather than generic brackets.
fn space_between(prev: &TokenKind, prev_cmp: bool, next: &TokenKind, next_cmp: bool) -> bool {
    use TokenKind::*;
    // Separators and postfix operators attach tightly to the left.
    if matches!(
        next,
        Comma | RParen | RBracket | Question | Dot | ColonColon | Colon
    ) {
        return false;
    }
    // Openers and prefix operators attach tightly to the right.
    if matches!(prev, LParen | LBracket | Dot | ColonColon | Bang) {
        return false;
    }
    // Generic angle brackets are tight (`List<Int>`, `impl<T>`); comparison
    // `<` / `>` fall through to the spaced default.
    if matches!(prev, Lt) && !prev_cmp {
        return false;
    }
    if matches!(next, Lt | Gt) && !next_cmp {
        return false;
    }
    if matches!(prev, Gt) && !prev_cmp {
        // `<T>(x)` — a parameter list attaches tightly to a generic closer.
        return !matches!(next, LParen);
    }
    // Call position: `f(x)`, `f(x)(y)`, `panic(...)`, `match x { ... }(y)`, and
    // an attribute argument `@lang("option")` (spec 0039 R7). Everything else
    // (keywords, operators, `,`) is followed by a space.
    if matches!(next, LParen) {
        return !matches!(
            prev,
            Ident(_) | RParen | RBracket | RBrace | Question | Panic | At(_)
        );
    }
    true
}

// ---------------------------------------------------------------------------
// Comparison classification
// ---------------------------------------------------------------------------

/// Byte offsets of the `<` / `>` tokens that are comparison operators. For
/// every `Binary { op: Lt | Gt }` node the operator token is the unique
/// `<` / `>` between the operands' spans; every `<` / `>` outside those
/// ranges is a generic bracket. The desugaring of `&& || !` keeps the
/// comparison `Binary` nodes intact, so the ranges survive it.
fn comparison_offsets(program: &ast::Program, tokens: &[Token]) -> HashSet<usize> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut on_block = |block: &ast::Block| collect_ranges_block(block, &mut ranges);
    for function in &program.functions {
        on_block(&function.body);
    }
    for implementation in &program.impls {
        for method in &implementation.methods {
            on_block(&method.body);
        }
    }
    for declaration in &program.traits {
        for method in &declaration.methods {
            if let Some(body) = &method.default_body {
                on_block(body);
            }
        }
    }
    let mut offsets = HashSet::new();
    for token in tokens {
        if matches!(token.kind, TokenKind::Lt | TokenKind::Gt)
            && ranges
                .iter()
                .any(|(low, high)| token.span.start >= *low && token.span.start < *high)
        {
            offsets.insert(token.span.start);
        }
    }
    offsets
}

fn collect_ranges_block(block: &ast::Block, out: &mut Vec<(usize, usize)>) {
    for item in &block.items {
        match item {
            ast::BlockItem::Let { value, .. } => collect_ranges_expr(value, out),
            ast::BlockItem::Expr(expr) => collect_ranges_expr(expr, out),
        }
    }
}

fn collect_ranges_expr(expr: &ast::Expr, out: &mut Vec<(usize, usize)>) {
    use ast::Expr;
    match expr {
        Expr::Binary {
            op, left, right, ..
        } => {
            if matches!(op, BinaryOp::Lt | BinaryOp::Gt) {
                out.push((left.span().end, right.span().start));
            }
            collect_ranges_expr(left, out);
            collect_ranges_expr(right, out);
        }
        Expr::Array(items, _) => {
            for item in items {
                collect_ranges_expr(item, out);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_ranges_expr(callee, out);
            for arg in args {
                collect_ranges_expr(arg, out);
            }
        }
        Expr::Fn { body, .. } => collect_ranges_block(body, out),
        Expr::Block(block) => collect_ranges_block(block, out),
        Expr::If {
            cond, then, els, ..
        } => {
            collect_ranges_expr(cond, out);
            collect_ranges_block(then, out);
            collect_ranges_block(els, out);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => {
            collect_ranges_expr(value, out);
        }
        Expr::Panic { message, .. } => collect_ranges_expr(message, out),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_ranges_expr(scrutinee, out);
            collect_ranges_arms(arms, out);
        }
        Expr::Try { body, arms, .. } => {
            collect_ranges_block(body, out);
            collect_ranges_arms(arms, out);
        }
        Expr::RecordLiteral { fields, .. } => {
            for (_, _, value) in fields {
                collect_ranges_expr(value, out);
            }
        }
        Expr::Field { target, .. } => collect_ranges_expr(target, out),
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..)
        | Expr::Path { .. }
        | Expr::TypePath { .. } => {}
    }
}

fn collect_ranges_arms(arms: &[ast::MatchArm], out: &mut Vec<(usize, usize)>) {
    for arm in arms {
        if let Some(guard) = &arm.guard {
            collect_ranges_expr(guard, out);
        }
        collect_ranges_expr(&arm.body, out);
    }
}

// ---------------------------------------------------------------------------
// Structural AST dump (the semantic-preservation check)
// ---------------------------------------------------------------------------

/// A span-free structural dump of the program. Two sources format-equal iff
/// their dumps are equal; used to verify the formatter changed nothing but
/// layout. A text dump (rather than `PartialEq`) gives inspectable diffs and
/// sidesteps the `Span`/`Arc<SourceFile>` fields, which must not participate.
fn ast_dump(program: &ast::Program) -> String {
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "module {:?}", program.module);
    for import in &program.imports {
        let _ = writeln!(w, "import {:?}", import.path);
    }
    for function in &program.functions {
        dump_function(w, 0, function);
    }
    for declaration in &program.externs {
        let _ = writeln!(
            w,
            "extern {} intrinsic={:?} module={:?} params={:?} ret={:?} throws={:?} uses={:?}",
            declaration.name,
            declaration.is_intrinsic,
            declaration.module,
            params_dump(&declaration.params),
            declaration.ret,
            declaration.throws,
            declaration.effects,
        );
    }
    for declaration in &program.enums {
        let _ = writeln!(
            w,
            "enum {} module={:?} params={:?}",
            declaration.name, declaration.module, declaration.type_params
        );
        for variant in &declaration.variants {
            let _ = writeln!(w, "  variant {} {:?}", variant.name, variant.fields);
        }
    }
    for declaration in &program.records {
        let _ = writeln!(
            w,
            "record {} module={:?}",
            declaration.name, declaration.module
        );
        for field in &declaration.fields {
            let _ = writeln!(w, "  field {} {:?}", field.name, field.ty);
        }
    }
    for declaration in &program.traits {
        let _ = writeln!(
            w,
            "trait {} module={:?}",
            declaration.name, declaration.module
        );
        for method in &declaration.methods {
            let _ = writeln!(
                w,
                "  method {} params={:?} ret={:?} throws={:?} uses={:?}",
                method.name,
                params_dump(&method.params),
                method.ret,
                method.throws,
                method.effects,
            );
            if let Some(body) = &method.default_body {
                dump_block(w, 2, body);
            }
        }
    }
    for declaration in &program.impls {
        let _ = writeln!(
            w,
            "impl {} for {:?} params={:?} bounds={:?} module={:?}",
            declaration.trait_name,
            declaration.target,
            declaration.type_params,
            bounds_dump(&declaration.bounds),
            declaration.module,
        );
        for method in &declaration.methods {
            dump_function(w, 1, method);
        }
    }
    out
}

fn params_dump(params: &[ast::Param]) -> Vec<(String, String)> {
    params
        .iter()
        .map(|param| (param.name.clone(), format!("{:?}", param.ty)))
        .collect()
}

fn bounds_dump(bounds: &[ast::Bound]) -> Vec<(String, Vec<String>)> {
    bounds
        .iter()
        .map(|bound| (bound.param.clone(), bound.traits.clone()))
        .collect()
}

fn dump_function(w: &mut String, depth: usize, function: &ast::Function) {
    let _ = writeln!(
        w,
        "{}fn {} pub={:?} path={:?} tparams={:?} bounds={:?} params={:?} ret={:?} throws={:?} uses={:?}",
        "  ".repeat(depth),
        function.name,
        function.is_public,
        function.module_path,
        function.type_params,
        bounds_dump(&function.bounds),
        params_dump(&function.params),
        function.ret,
        function.throws,
        function.effects,
    );
    dump_block(w, depth + 1, &function.body);
}

fn dump_block(w: &mut String, depth: usize, block: &ast::Block) {
    let indent = "  ".repeat(depth);
    let _ = writeln!(w, "{indent}block");
    for item in &block.items {
        match item {
            ast::BlockItem::Let {
                name, ty, value, ..
            } => {
                let _ = writeln!(w, "{indent}  let {name} ty={ty:?}");
                dump_expr(w, depth + 2, value);
            }
            ast::BlockItem::Expr(expr) => dump_expr(w, depth + 1, expr),
        }
    }
}

fn dump_expr(w: &mut String, depth: usize, expr: &ast::Expr) {
    use ast::Expr;
    let indent = "  ".repeat(depth);
    match expr {
        Expr::Int(value, _) => {
            let _ = writeln!(w, "{indent}int {value}");
        }
        Expr::Float(value, _) => {
            let _ = writeln!(w, "{indent}float {value:?}");
        }
        Expr::Bool(value, _) => {
            let _ = writeln!(w, "{indent}bool {value}");
        }
        Expr::String(value, _) => {
            let _ = writeln!(w, "{indent}string {value:?}");
        }
        Expr::Char(value, _) => {
            let _ = writeln!(w, "{indent}char {value:?}");
        }
        Expr::Unit(_) => {
            let _ = writeln!(w, "{indent}unit");
        }
        Expr::Var(name, _) => {
            let _ = writeln!(w, "{indent}var {name}");
        }
        Expr::Array(items, _) => {
            let _ = writeln!(w, "{indent}array");
            for item in items {
                dump_expr(w, depth + 1, item);
            }
        }
        Expr::Call { callee, args, .. } => {
            let _ = writeln!(w, "{indent}call");
            dump_expr(w, depth + 1, callee);
            for arg in args {
                dump_expr(w, depth + 1, arg);
            }
        }
        Expr::Fn {
            params,
            ret,
            throws,
            effects,
            body,
            ..
        } => {
            let _ = writeln!(
                w,
                "{indent}lambda params={:?} ret={ret:?} throws={throws:?} uses={effects:?}",
                params_dump(params),
            );
            dump_block(w, depth + 1, body);
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let _ = writeln!(w, "{indent}binary {op:?}");
            dump_expr(w, depth + 1, left);
            dump_expr(w, depth + 1, right);
        }
        Expr::Block(block) => dump_block(w, depth, block),
        Expr::If {
            cond, then, els, ..
        } => {
            let _ = writeln!(w, "{indent}if");
            dump_expr(w, depth + 1, cond);
            dump_block(w, depth + 1, then);
            dump_block(w, depth + 1, els);
        }
        Expr::Throw { value, .. } => {
            let _ = writeln!(w, "{indent}throw");
            dump_expr(w, depth + 1, value);
        }
        Expr::Panic { message, .. } => {
            let _ = writeln!(w, "{indent}panic");
            dump_expr(w, depth + 1, message);
        }
        Expr::Question { value, .. } => {
            let _ = writeln!(w, "{indent}question");
            dump_expr(w, depth + 1, value);
        }
        Expr::RecordLiteral { name, fields, .. } => {
            let _ = writeln!(w, "{indent}record_literal {name}");
            for (field_name, _, value) in fields {
                let _ = writeln!(w, "{indent}  field {field_name}");
                dump_expr(w, depth + 2, value);
            }
        }
        Expr::Field { target, name, .. } => {
            let _ = writeln!(w, "{indent}field_access {name}");
            dump_expr(w, depth + 1, target);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let _ = writeln!(w, "{indent}match");
            dump_expr(w, depth + 1, scrutinee);
            dump_arms(w, depth + 1, arms);
        }
        Expr::Try { body, arms, .. } => {
            let _ = writeln!(w, "{indent}try");
            dump_block(w, depth + 1, body);
            dump_arms(w, depth + 1, arms);
        }
        Expr::Path { segments, .. } => {
            let _ = writeln!(w, "{indent}path {segments:?}");
        }
        Expr::TypePath { segments, .. } => {
            let _ = writeln!(w, "{indent}typepath {segments:?}");
        }
    }
}

fn dump_arms(w: &mut String, depth: usize, arms: &[ast::MatchArm]) {
    let indent = "  ".repeat(depth);
    for arm in arms {
        let pattern = match &arm.pattern {
            ast::Pattern::Wildcard(_) => "_".to_string(),
            ast::Pattern::Binding { name, .. } => format!("bind {name}"),
            ast::Pattern::Variant {
                enum_name,
                variant,
                fields,
                ..
            } => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|field| match field {
                        ast::FieldBinding::Name(name) => name.clone(),
                        ast::FieldBinding::Ignore => "_".to_string(),
                    })
                    .collect();
                format!("variant {enum_name:?}::{variant}({fields:?})")
            }
        };
        let _ = writeln!(w, "{indent}arm {pattern}");
        if let Some(guard) = &arm.guard {
            let _ = writeln!(w, "{indent}  guard");
            dump_expr(w, depth + 2, guard);
        }
        dump_expr(w, depth + 1, &arm.body);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::format_source;

    fn fmt(source: &str) -> String {
        format_source("test.emel", source).expect("format_source failed")
    }

    #[test]
    fn normalizes_spacing_and_indentation() {
        let source = "fn add (x:Int,y:Int)->Int uses {} {\n  x+y\n}\n";
        assert_eq!(
            fmt(source),
            "fn add(x: Int, y: Int) -> Int uses {} {\n    x + y\n}\n"
        );
    }

    #[test]
    fn idempotent() {
        let source = "fn add(x: Int, y: Int) -> Int {\n  x + y\n}\n\nfn main() -> Unit uses {} {\n  let s = add(1, 2)\n  ()\n}\n";
        let once = fmt(source);
        assert_eq!(fmt(&once), once);
    }

    #[test]
    fn preserves_comments() {
        let source = "-- header\n\n-- about add\nfn add(x: Int, y: Int) -> Int {\n    x + y -- trailing\n}\n";
        let expected = "-- header\n\n-- about add\nfn add(x: Int, y: Int) -> Int {\n    x + y -- trailing\n}\n";
        assert_eq!(fmt(source), expected);
    }

    #[test]
    fn collapses_blank_lines() {
        let source = "fn a() -> Int {\n\n\n    1\n\n}\n\n\n\nfn b() -> Int {\n    2\n}\n";
        let expected = "fn a() -> Int {\n    1\n}\n\nfn b() -> Int {\n    2\n}\n";
        assert_eq!(fmt(source), expected);
    }

    #[test]
    fn comparison_vs_generics_spacing() {
        let source = "fn f(xs: Array<Int>, n: Int) -> Bool {\n    n<3\n}\n";
        let expected = "fn f(xs: Array<Int>, n: Int) -> Bool {\n    n < 3\n}\n";
        assert_eq!(fmt(source), expected);
    }

    #[test]
    fn breaks_long_call_with_trailing_comma() {
        let source = "fn f(a: Int, b: Int) -> Int {\n    f(an_extremely_long_argument_name_number_one + 111111111, an_extremely_long_argument_name_number_two + 222222222)\n}\n";
        let expected = "fn f(a: Int, b: Int) -> Int {\n    f(\n        an_extremely_long_argument_name_number_one + 111111111,\n        an_extremely_long_argument_name_number_two + 222222222,\n    )\n}\n";
        assert_eq!(fmt(source), expected);
    }

    #[test]
    fn joins_short_multiline_call() {
        let source =
            "fn add(x: Int, y: Int) -> Int {\n    add(\n        1,\n        2,\n    )\n}\n";
        let expected = "fn add(x: Int, y: Int) -> Int {\n    add(1, 2)\n}\n";
        assert_eq!(fmt(source), expected);
    }

    #[test]
    fn keeps_multiline_blocks() {
        let source = "fn f() -> Int {\n    let x = 1\n    x\n}\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn drops_redundant_separator_commas() {
        let source = "enum Color {\n    Red,\n    Blue,\n}\n";
        let expected = "enum Color {\n    Red\n    Blue\n}\n";
        assert_eq!(fmt(source), expected);
    }

    #[test]
    fn preserves_string_and_float_literals() {
        let source = "fn f() -> Float {\n    let s = \"a\\nb\"\n    2.50\n}\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn lambda_keeps_fn_space() {
        let source = "fn f() -> Int {\n    let g = fn (x: Int) -> Int { x * 2 }\n    g(3)\n}\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn effect_row_spacing() {
        let source = "fn f() -> Unit uses {Stdout,Clock} {\n    ()\n}\n";
        let expected = "fn f() -> Unit uses { Stdout, Clock } {\n    ()\n}\n";
        assert_eq!(fmt(source), expected);
    }

    #[test]
    fn preserves_redundant_parens() {
        let source = "fn f(a: Int, b: Int) -> Int {\n    (a + b) * 2\n}\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn match_and_try_render() {
        let source = "enum E {\n    A\n    B\n}\n\nfn f(e: E) -> Int {\n    match e {\n        A -> 1\n        B -> 2\n    }\n}\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn comment_in_arguments_forces_break() {
        let source = "fn f(a: Int) -> Int {\n    f(\n        -- the seed\n        1,\n    )\n}\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn unparsable_source_is_an_error() {
        assert!(format_source("test.emel", "fn f( -> {").is_err());
    }

    #[test]
    fn empty_file_stays_empty() {
        assert_eq!(fmt(""), "");
    }
}
