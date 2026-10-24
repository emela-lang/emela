//! A small C source writer used by the embedded emitter.

#[derive(Debug, Default)]
pub(crate) struct Writer {
    output: String,
    indent: usize,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
        self.output.push_str(text);
        self.output.push('\n');
    }

    pub(crate) fn assign(&mut self, target: &str, expression: &str) {
        self.line(&format!("{target} = {expression};"));
    }

    pub(crate) fn blank(&mut self) {
        self.output.push('\n');
    }

    pub(crate) fn begin_block(&mut self, head: &str) {
        self.line(&format!("{head} {{"));
        self.indent += 1;
    }

    pub(crate) fn end_block(&mut self) {
        self.indent -= 1;
        self.line("}");
    }

    pub(crate) fn ret(&mut self, value: Option<&str>) {
        match value {
            Some(value) => self.line(&format!("return {value};")),
            None => self.line("return;"),
        }
    }

    pub(crate) fn decl(&mut self, ty: &str, name: &str, init: Option<&str>) {
        match init {
            Some(init) => self.line(&format!("{ty} {name} = {init};")),
            None => self.line(&format!("{ty} {name};")),
        }
    }

    pub(crate) fn finish(self) -> String {
        self.output
    }
}
