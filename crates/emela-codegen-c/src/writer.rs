//! A writer of C source text.
//!
//! The emitter says which C construct it writes (a prototype, a declaration, an `if`, a `goto`)
//! and the writer lays it out: one statement per line, two spaces per level of nesting. The
//! expressions inside stay strings, built with the helpers at the end of this file ([`call`],
//! [`signature`], [`init_list`]) or by the emitter.
//!
//! Compound statements come in two forms: with a closure for the body (`func`, `if_`, `block`)
//! when the body only writes, and as `*_begin` / `end` pairs when the body goes back into the
//! emitter, which holds the writer too.

/// C source being written, and the current nesting.
#[derive(Debug, Default)]
pub(crate) struct Writer {
    out: String,
    indent: usize,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A writer whose lines start `indent` levels in (a function body).
    pub(crate) fn indented(indent: usize) -> Self {
        Writer {
            out: String::new(),
            indent,
        }
    }

    /// An empty writer at the same nesting, for a part written aside and appended later.
    pub(crate) fn aside(&self) -> Self {
        Self::indented(self.indent)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// Appends what `other` wrote, as it is.
    pub(crate) fn append(&mut self, other: Writer) {
        self.out.push_str(&other.out);
    }

    pub(crate) fn finish(self) -> String {
        self.out
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// `head {`, nesting what follows until [`Writer::end`].
    fn open(&mut self, head: &str) {
        self.line(&format!("{head} {{"));
        self.indent += 1;
    }

    /// The `}` closing the innermost `*_begin`.
    pub(crate) fn end(&mut self) {
        self.indent -= 1;
        self.line("}");
    }

    pub(crate) fn blank(&mut self) {
        self.out.push('\n');
    }

    /// `/* text */`. A `*/` in the text is broken up.
    pub(crate) fn comment(&mut self, text: &str) {
        self.line(&format!("/* {} */", text.replace("*/", "* /")));
    }

    /// `#include "file"`.
    pub(crate) fn include(&mut self, file: &str) {
        self.line(&format!("#include \"{file}\""));
    }

    /// `static SIG;`.
    pub(crate) fn static_proto(&mut self, sig: &str) {
        self.line(&format!("static {sig};"));
    }

    /// `extern SIG;`.
    pub(crate) fn extern_proto(&mut self, sig: &str) {
        self.line(&format!("extern {sig};"));
    }

    /// `SIG { body }`.
    pub(crate) fn func(&mut self, sig: &str, body: impl FnOnce(&mut Self)) {
        self.open(sig);
        body(self);
        self.end();
    }

    /// `static SIG { body }`.
    pub(crate) fn static_func(&mut self, sig: &str, body: impl FnOnce(&mut Self)) {
        self.func(&format!("static {sig}"), body);
    }

    /// An expression statement: `expr;`.
    pub(crate) fn stmt(&mut self, expr: &str) {
        self.line(&format!("{expr};"));
    }

    /// `lhs = rhs;`.
    pub(crate) fn assign(&mut self, lhs: &str, rhs: &str) {
        self.line(&format!("{lhs} = {rhs};"));
    }

    /// `TY name = init;`, or `TY name;` without an initializer. A pointer type (`char *`) is
    /// written next to the name.
    pub(crate) fn decl(&mut self, ty: &str, name: &str, init: Option<&str>) {
        let sep = if ty.ends_with('*') { "" } else { " " };
        match init {
            Some(init) => self.line(&format!("{ty}{sep}{name} = {init};")),
            None => self.line(&format!("{ty}{sep}{name};")),
        }
    }

    /// `return value;`, or `return;`.
    pub(crate) fn ret(&mut self, value: Option<&str>) {
        match value {
            Some(v) => self.line(&format!("return {v};")),
            None => self.line("return;"),
        }
    }

    /// `goto label;`.
    pub(crate) fn goto(&mut self, label: &str) {
        self.line(&format!("goto {label};"));
    }

    /// `label:;`, at the current nesting.
    pub(crate) fn label(&mut self, label: &str) {
        self.line(&format!("{label}:;"));
    }

    /// `label:;` at the start of the line, where it stands out (a resume point).
    pub(crate) fn label_at_margin(&mut self, label: &str) {
        self.out.push_str(&format!("{label}:;\n"));
    }

    /// `if (cond) { then }`.
    pub(crate) fn if_(&mut self, cond: &str, then: impl FnOnce(&mut Self)) {
        self.if_begin(cond);
        then(self);
        self.end();
    }

    /// `if (cond) return value;` on one line.
    pub(crate) fn if_return(&mut self, cond: &str, value: Option<&str>) {
        match value {
            Some(v) => self.line(&format!("if ({cond}) return {v};")),
            None => self.line(&format!("if ({cond}) return;")),
        }
    }

    /// `if (cond) {`, up to [`Writer::else_begin`] or [`Writer::end`].
    pub(crate) fn if_begin(&mut self, cond: &str) {
        self.open(&format!("if ({cond})"));
    }

    /// `} else {`, up to [`Writer::end`].
    pub(crate) fn else_begin(&mut self) {
        self.indent -= 1;
        self.open("} else");
    }

    /// `{ body }`: a block for its own declarations.
    pub(crate) fn block(&mut self, body: impl FnOnce(&mut Self)) {
        self.block_begin();
        body(self);
        self.end();
    }

    /// `{`, up to [`Writer::end`].
    pub(crate) fn block_begin(&mut self) {
        self.line("{");
        self.indent += 1;
    }

    /// `switch (scrutinee) { cases }`. The cases are at the nesting of the `switch`.
    pub(crate) fn switch(&mut self, scrutinee: &str, cases: impl FnOnce(&mut Self)) {
        self.switch_begin(scrutinee);
        cases(self);
        self.switch_end();
    }

    /// `switch (scrutinee) {`, up to [`Writer::switch_end`].
    pub(crate) fn switch_begin(&mut self, scrutinee: &str) {
        self.line(&format!("switch ({scrutinee}) {{"));
    }

    /// The `}` closing a `switch`.
    pub(crate) fn switch_end(&mut self) {
        self.line("}");
    }

    /// `case key:` and the body below it, falling through at its end.
    pub(crate) fn case(&mut self, key: &str, body: impl FnOnce(&mut Self)) {
        self.line(&format!("case {key}:"));
        self.indent += 1;
        body(self);
        self.indent -= 1;
    }

    /// `case key: {`, up to [`Writer::end`].
    pub(crate) fn case_begin(&mut self, key: &str) {
        self.open(&format!("case {key}:"));
    }

    /// `default:` and the body below it.
    pub(crate) fn default_case(&mut self, body: impl FnOnce(&mut Self)) {
        self.line("default:");
        self.indent += 1;
        body(self);
        self.indent -= 1;
    }

    /// `default: {`, up to [`Writer::end`].
    pub(crate) fn default_begin(&mut self) {
        self.open("default:");
    }

    /// `default: expr;` on one line.
    pub(crate) fn default_stmt(&mut self, expr: &str) {
        self.line(&format!("default: {expr};"));
    }
}

/// `name(a, b)`.
pub(crate) fn call<S: AsRef<str>>(name: &str, args: impl IntoIterator<Item = S>) -> String {
    format!("{name}({})", join(args))
}

/// `RET name(PARAMS)`, with `void` for no parameters.
pub(crate) fn signature<S: AsRef<str>>(
    ret: &str,
    name: &str,
    params: impl IntoIterator<Item = S>,
) -> String {
    let params = join(params);
    let params = if params.is_empty() { "void" } else { &params };
    format!("{ret} {name}({params})")
}

/// `TY name`, a parameter.
pub(crate) fn param(ty: &str, name: &str) -> String {
    format!("{ty} {name}")
}

/// `{a, b}`, an initializer.
pub(crate) fn init_list<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> String {
    format!("{{{}}}", join(items))
}

fn join<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> String {
    let mut out = String::new();
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(item.as_ref());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let mut w = Writer::new();
        w.comment("f */ g");
        w.static_func(&signature("int32_t", "f", [param("em_ref", "v1")]), |w| {
            w.decl("const char *", "c0", Some(&call("em_cstr", ["v1"])));
            w.if_return("c0 == NULL", Some("0"));
            w.switch("v1", |w| {
                w.case("0", |w| w.goto("l"));
                w.default_case(|w| w.stmt("em_unreachable()"));
            });
            w.label("l");
            w.ret(Some("1"));
        });
        assert_eq!(
            w.finish(),
            "/* f * / g */\n\
             static int32_t f(em_ref v1) {\n  const char *c0 = em_cstr(v1);\n  \
             if (c0 == NULL) return 0;\n  switch (v1) {\n  case 0:\n    goto l;\n  \
             default:\n    em_unreachable();\n  }\n  l:;\n  return 1;\n}\n"
        );
        assert_eq!(signature("void", "g", Vec::<String>::new()), "void g(void)");
        assert_eq!(init_list(["1", "0"]), "{1, 0}");
    }
}
