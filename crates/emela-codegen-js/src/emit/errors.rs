//! Emits errors (spec chapter 7).
//!
//! Failures are returned as values (spec 16.5). `fail e` returns the error value wrapped in the
//! runtime's `$Fail`, and when a fallible call (`Expr::Call::fails`) yields a `$Fail`, it is
//! passed to the failure target: the innermost `escape`, or the function's caller.
//!
//! `escape` becomes two nested labeled blocks.
//!
//! ```js
//! let $t0, $t1;          // the escape's value, the caught failure
//! $x0: {
//!   $e1: {
//!     ...                // the target expression; on failure `{ $t1 = failure; break $e1; }`
//!     break $x0;
//!   }
//!   const $t2 = $t1.error;
//!   if ($t2.$tag === $err$NotFound) { ...; $t0 = arm value; break $x0; }
//!   return $t1;          // a failure no arm matched propagates outward
//! }
//! ```
//!
//! An error value carries a per-error descriptor (`$err$NotFound`) in `$tag`. The descriptor also
//! has a show function, which the entry module and the test runner use to print unhandled errors.

use emela_core::{Arm, EnumId, ErrorDisplay, Expr, Pat};

use super::{Dest, Emitter, sanitize, unparen};

pub(super) enum FailTarget {
    /// Inside an escape's target expression. Stores the failure in `cell` and breaks out of the
    /// `label` block.
    Escape { label: String, cell: String },
}

impl Emitter<'_> {
    /// The JS expression for an enum value's `$tag`: the descriptor for an error, otherwise the
    /// variant index.
    pub(super) fn tag(&self, id: EnumId, variant: usize) -> String {
        if self.m.enums[id].error {
            descriptor_name(&self.m.enums[id].name)
        } else {
            variant.to_string()
        }
    }

    /// Emits the per-error descriptors at the top of the module. Each has a show function
    /// (`show`).
    ///
    /// An error whose type satisfies Show is shown by its Show: the display of the structure, or
    /// its written impl. Any other error is shown by its name (spec 14.3, `Module::error_display`).
    pub(super) fn error_descriptors(&mut self) {
        let m = self.m;
        let mut any = false;
        for (id, def) in m.enums.iter() {
            if !def.error {
                continue;
            }
            let show = match m.error_display.get(&id) {
                Some(ErrorDisplay::Structure) => self.show_enum(id),
                Some(&ErrorDisplay::Impl(f)) => format!("(v) => {}(v)", self.fn_js(f)),
                // The constructor's name, unqualified like the one the show function writes.
                None => format!("() => {}", super::js_string(&def.variants[0].name)),
            };
            self.line(&format!(
                "const {} = Object.freeze({{ show: {show} }});",
                descriptor_name(&def.name)
            ));
            any = true;
        }
        if any {
            self.line("");
        }
    }

    /// A statement passing the failure `v` (a JS expression for a `$Fail`) to the target.
    fn propagate_stmt(&self, v: &str) -> String {
        match self.fail_targets.last() {
            None => format!("return {v};"),
            Some(FailTarget::Escape { label, cell }) => {
                format!("{{ {cell} = {v}; break {label}; }}")
            }
        }
    }

    /// Binds the result of a fallible call to a temporary and passes a failure to the target.
    /// Returns the temporary's name.
    pub(super) fn checked(&mut self, call: String) -> String {
        let t = self.tmp();
        self.line(&format!("const {t} = {call};"));
        let fail = self.rt("$Fail");
        let propagate = self.propagate_stmt(&t);
        self.line(&format!("if ({t} instanceof {fail}) {propagate}"));
        t
    }

    pub(super) fn fail(&mut self, e: &Expr) {
        let v = self.value(e);
        let fail = self.rt("$Fail");
        let stmt = self.propagate_stmt(&format!("new {fail}({v})"));
        self.line(&stmt);
    }

    /// Statements that pass the value of `body escape { arms }` to `dest`.
    pub(super) fn escape(&mut self, body: &Expr, arms: &[Arm], dest: &Dest) {
        // Inside the target expression we cannot return to the caller directly, so the value
        // always goes through a temporary.
        let result = match dest {
            Dest::Assign(t) => t.clone(),
            Dest::Return => {
                let t = self.tmp();
                self.line(&format!("let {t};"));
                t
            }
        };
        let cell = self.tmp();
        self.line(&format!("let {cell};"));
        let done = self.label("x");
        let caught = self.label("e");
        self.line(&format!("{done}: {{"));
        self.indent += 1;

        self.line(&format!("{caught}: {{"));
        self.indent += 1;
        self.fail_targets.push(FailTarget::Escape {
            label: caught,
            cell: cell.clone(),
        });
        self.stmt(body, &Dest::Assign(result.clone()));
        self.fail_targets.pop();
        self.line(&format!("break {done};"));
        self.indent -= 1;
        self.line("}");

        // The arms are outside the target expression, so failures in them go to the outer target.
        let error = self.tmp();
        self.line(&format!("const {error} = {cell}.error;"));
        // Errors fully caught by earlier arms; later arms for the same error never see them.
        let mut total: Vec<EnumId> = Vec::new();
        for arm in arms {
            let Pat::Ctor { ctor, .. } = &arm.pat else {
                panic!("invalid IR: an escape arm's pattern is not an error constructor");
            };
            assert!(
                self.m.enums[ctor.enum_id].error,
                "invalid IR: an escape arm's pattern is not an error constructor"
            );
            if total.contains(&ctor.enum_id) {
                continue;
            }
            let mut conds = vec![format!(
                "{error}.$tag === {}",
                self.tag(ctor.enum_id, ctor.variant)
            )];
            let mut binds = Vec::new();
            self.pattern(&arm.pat, error.clone(), &mut conds, &mut binds);
            if conds.len() == 1 && arm.guard.is_none() {
                total.push(ctor.enum_id);
            }
            self.line(&format!("if ({}) {{", conds.join(" && ")));
            self.indent += 1;
            for (l, access) in binds {
                let name = self.local(l);
                self.line(&format!("const {name} = {access};"));
            }
            let mut opened = 0;
            if let Some(g) = &arm.guard {
                let gv = self.value(g);
                self.line(&format!("if ({}) {{", unparen(&gv)));
                self.indent += 1;
                opened += 1;
            }
            self.stmt(&arm.body, &Dest::Assign(result.clone()));
            self.line(&format!("break {done};"));
            for _ in 0..opened {
                self.indent -= 1;
                self.line("}");
            }
            self.indent -= 1;
            self.line("}");
        }
        // A failure no arm matched (an error with no arm, or a value that partial arms did not
        // match) propagates outward (spec 7.3).
        let propagate = self.propagate_stmt(&cell);
        self.line(&propagate);

        self.indent -= 1;
        self.line("}");
        if let Dest::Return = dest {
            self.line(&format!("return {result};"));
        }
    }
}

fn descriptor_name(error: &str) -> String {
    format!("$err${}", sanitize(error))
}
