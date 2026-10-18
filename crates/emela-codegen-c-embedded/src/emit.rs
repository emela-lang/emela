//! The emitter: Heap IR (stage `RefCounted`, value types on, halting on defects) to freestanding
//! C11.
//!
//! It follows the Heap IR the way the host C emitter does: every IR variable is a C local declared
//! at the top of its function, `yield` and `break` assign the destination of their statement and
//! jump past it, `recur` assigns the loop variables and jumps to the loop head. What differs:
//!
//! - Value aggregates ([`Repr::Agg`]) are C structs passed by value (see [`Types`]). There is no
//!   heap and no runtime; the checker ([`crate::check`]) has rejected every heap value.
//! - Evidence tables and the handlers of board effects have no C value: their parameters,
//!   arguments, `dup` and `drop` are left out, and an operation call through a handler is a call
//!   of the board's C function (`emela_gpio_write`).
//! - `runtime defect(msg)` calls the board's hook `emela_board_defect`, which does not return.

use emela_core::{Builtin, EnumId, Type, TypeId, VariantFields};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, Layout, Module, OnFail, Operand, PrimOp,
    PrimTy, Repr, RuntimeOp, SeqId, Stage, Stmt, StmtId, SwitchOn, Term, Value, VarId,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::board::{self, Peripheral};
use crate::check::{Entry, console, external_name, is_board_op, is_target_external, no_data};
use crate::heap::Boxed;
use crate::names;
use crate::writer::{Writer, call, param, signature};
use crate::{BOARD_H_FILE, EMBEDDED_H_FILE, Error, Prepared};

/// The C files of the program.
pub(crate) struct Files {
    pub main: String,
    /// `emela_board.h`: the hooks the board implements.
    pub board: String,
    /// One header per peripheral, (file name, contents).
    pub peripherals: Vec<(String, String)>,
    /// `emela_embedded.h`: the entries the board calls and the external functions.
    pub embedded: String,
    /// The C files of the project that external functions name: (output name, source path).
    pub copies: Vec<(String, std::path::PathBuf)>,
    /// The bundled C files that external functions name: (output name, contents).
    pub bundled: Vec<(String, String)>,
}

pub(crate) fn emit(
    m: &Module,
    prepared: &Prepared,
    entries: &[Entry],
    reached: &[FnId],
) -> Result<Files, Vec<Error>> {
    assert_eq!(m.stage, Stage::RefCounted, "the emitter reads the Heap IR");
    let mut e = Emitter {
        m,
        types: Types {
            m,
            layouts: &prepared.layouts,
            board_types: &prepared.board_types,
            errors: &prepared.errors,
            boxes: &prepared.boxes,
            static_boxes: Vec::new(),
            names: FxHashMap::default(),
            used: FxHashSet::default(),
            defined: FxHashSet::default(),
            out: Writer::new(),
        },
        consts: &prepared.consts,
        strings: false,
        float_strings: false,
        sinks: sinks(m, reached),
        fn_names: m
            .functions
            .iter()
            .map(|(id, f)| match f.external.as_ref().map(external_name) {
                Some(Ok((name, _))) => name,
                _ => names::function(ix(id), &short_name(&f.name)),
            })
            .collect(),
        errors: Vec::new(),
    };
    let mut peripherals = peripherals(m, reached);
    // The error enums a failure may hold (`PeripheralError` of a board operation).
    for (id, _) in &prepared.errors {
        if let Some((p, _)) = board::split(&m.core.enums[*id].name)
            && !peripherals.contains(&p)
        {
            peripherals.push(p);
        }
    }
    peripherals.sort();
    let mut headers = Vec::new();
    for p in &peripherals {
        headers.push((p.header(), e.types.peripheral_header(p)));
    }
    let mut protos = Writer::new();
    let mut bodies = Writer::new();
    let mut externals = Writer::new();
    // C functions the program's external functions name that `main.c` defines.
    let mut defined = Writer::new();
    let mut copies: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut bundled: Vec<(String, String)> = Vec::new();
    let mut copied = FxHashMap::default();
    for &id in reached {
        let f = &m.functions[id];
        match &f.external {
            // Declared in `emela_board.h` or a peripheral's header, or defined below.
            // Console's line: its pieces, then the newline.
            Some(_) if console(f).is_some() => {
                let hook = console(f).unwrap_or_default();
                let name = &e.fn_names[ix(id)];
                defined.line_raw(&format!(
                    "static inline void {name}(const char *text, uint16_t length) {{ {hook}(text, length); {hook}(\"\\n\", 1); }}"
                ));
            }
            Some(_) if is_target_external(f) => {}
            Some(ext) => {
                externals.extern_proto(&e.external_signature(id, f));
                if let Ok((_, Some(module))) = external_name(ext) {
                    e.copy(module, &mut copied, &mut copies, &mut bundled);
                }
            }
            None => protos.static_proto(&e.signature(id, f)),
        }
    }
    for &id in reached {
        if m.functions[id].external.is_none() {
            e.function(&mut bodies, id, &m.functions[id]);
            bodies.blank();
        }
    }
    let mut embedded = Writer::new();
    embedded.line_raw("#ifndef EMELA_EMBEDDED_H");
    embedded.line_raw("#define EMELA_EMBEDDED_H");
    embedded.blank();
    embedded.include(BOARD_H_FILE);
    for p in &peripherals {
        embedded.include(&p.header());
    }
    embedded.blank();
    if !externals.is_empty() {
        embedded.comment("External functions (@external(embedded, ...)), implemented in C.");
        embedded.append(externals);
        embedded.blank();
    }
    let mut main_fn = None;
    let mut tests: Vec<(&str, FnId)> = Vec::new();
    for entry in entries {
        match &entry.export {
            None if entry.test.is_some() => {
                tests.push((entry.test.as_deref().unwrap_or_default(), entry.function));
            }
            None => main_fn = Some(entry.function),
            Some(export) => {
                let f = &m.functions[entry.function];
                let sig = e.export_signature(export, f);
                if entry.isr {
                    embedded.comment(
                        "Entries called from interrupts; what they reach was checked against the ISR constraints.",
                    );
                }
                embedded.extern_proto(&sig);
                let fails = f.fails.then(|| e.types.define_failure());
                let string_params: Vec<bool> = e
                    .value_params(f)
                    .into_iter()
                    .map(|p| e.is_string(f.vars[p].ty))
                    .collect();
                let returns_string = e.is_string(f.ret);
                bodies.func(&sig, |w| {
                    let mut args: Vec<String> = e
                        .value_params(f)
                        .into_iter()
                        .zip(&string_params)
                        .map(|(p, &string)| match string {
                            true => format!("(EmStr){{{0}, {0}_length, 0}}", var(p)),
                            false => var(p),
                        })
                        .collect();
                    if returns_string {
                        w.decl("EmString", "result", None);
                        args.push("&result".into());
                    }
                    if let Some(err) = &fails {
                        w.decl(err, "em_f", Some("{0}"));
                        args.push("&em_f".into());
                    }
                    let c = call(&e.fn_names[ix(entry.function)], args);
                    if returns_string {
                        // The bytes, then a NUL if there is room for it.
                        w.stmt(&c);
                        if fails.is_some() {
                            w.if_("em_f.tag != 0", |w| w.stmt(UNHANDLED));
                        }
                        w.decl("EmStr", "v", Some("em_view(&result)"));
                        w.if_("v.len > capacity", |w| {
                            w.stmt("emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", 24)")
                        });
                        w.stmt("memcpy(out, v.ptr, v.len)");
                        w.if_("v.len < capacity", |w| w.assign("out[v.len]", "'\\0'"));
                        w.ret(Some("v.len"));
                    } else if e.types.is_void(f.ret) {
                        w.stmt(&c);
                        if fails.is_some() {
                            w.if_("em_f.tag != 0", |w| w.stmt(UNHANDLED));
                        }
                    } else if fails.is_some() {
                        let ty = e.types.ctype(f.ret);
                        w.decl(&ty, "result", Some(&c));
                        w.if_("em_f.tag != 0", |w| w.stmt(UNHANDLED));
                        w.ret(Some("result"));
                    } else {
                        w.ret(Some(&c));
                    }
                });
                bodies.blank();
            }
        }
    }
    if !prepared.boxes.is_empty() {
        embedded.comment(
            "The heap's use, for sizing [embedded.memory] heap_bytes. used is the bytes cut from the\n\
             area: the smallest heap_bytes this run needed (freed boxes are reused for boxes of the\n\
             same size). live is the bytes of the boxes in use. Not callable from interrupts.",
        );
        embedded.line_raw("uint32_t emela_heap_used_bytes(void);");
        embedded.line_raw("uint32_t emela_heap_live_bytes(void);");
        embedded.blank();
    }
    embedded.blank();
    embedded.line_raw("#endif");
    if let Some(main) = main_fn {
        let returns_string = e.is_string(m.functions[main].ret);
        let fails = m.functions[main].fails.then(|| e.types.define_failure());
        bodies.func("int main(void)", |w| {
            w.stmt("emela_board_init()");
            let mut args: Vec<String> = Vec::new();
            if returns_string {
                // `main`'s value is dropped.
                w.line_raw("static EmString result;");
                args.push("&result".into());
            }
            if let Some(err) = &fails {
                w.line_raw(&format!("static {err} em_f;"));
                args.push("&em_f".into());
            }
            w.stmt(&call(&e.fn_names[ix(main)], args));
            if fails.is_some() {
                w.if_("em_f.tag != 0", |w| w.stmt(UNHANDLED));
            }
            w.line_raw("for (;;) {");
            w.line_raw("  emela_board_idle();");
            w.line_raw("}");
        });
    }
    if !tests.is_empty() {
        let names: Vec<String> = tests
            .iter()
            .map(|(name, _)| names::string_literal(name.as_bytes()))
            .collect();
        bodies.line_raw(&format!(
            "static const char *const em_test_names[] = {{{}}};",
            names.join(", ")
        ));
        bodies.func("int main(void)", |w| {
            w.stmt("emela_board_init()");
            w.line_raw(&format!(
                "for (int i = EMELA_TEST_START; i < {}; i++) {{",
                tests.len()
            ));
            w.line_raw("  em_test_current = em_test_names[i];");
            w.line_raw("  bool failed = false;");
            w.line_raw("  switch (i) {");
            for (k, &(_, id)) in tests.iter().enumerate() {
                let f = &m.functions[id];
                let mut args: Vec<String> = Vec::new();
                w.line_raw(&format!("  case {k}: {{"));
                if e.is_string(f.ret) {
                    w.line_raw("    static EmString result;");
                    args.push("&result".into());
                }
                if f.fails {
                    let err = e.types.define_failure();
                    w.line_raw(&format!("    {err} em_f = {{0}};"));
                    args.push("&em_f".into());
                }
                w.line_raw(&format!("    {};", call(&e.fn_names[ix(id)], args)));
                if f.fails {
                    w.line_raw("    failed = em_f.tag != 0;");
                }
                w.line_raw("    break;");
                w.line_raw("  }");
            }
            w.line_raw("  default: break;");
            w.line_raw("  }");
            w.line_raw("  if (failed) {");
            w.line_raw("    em_test_result(\"error\", \"unhandled error\", 15);");
            w.line_raw("  } else {");
            w.line_raw("    em_test_result(\"ok\", \"\", 0);");
            w.line_raw("  }");
            w.line_raw("}");
            w.stmt("em_test_current = NULL");
            w.stmt("em_test_write(\"\\x1e\" \"emela-test {\\\"event\\\":\\\"done\\\"}\\n\")");
            w.line_raw("for (;;) {");
            w.line_raw("  emela_board_idle();");
            w.line_raw("}");
        });
    }
    if !e.errors.is_empty() {
        return Err(e.errors);
    }
    // The heap: the box types (declared first, as boxes point to each other), the allocator and
    // the boxes' drop functions.
    let mut box_types: Vec<TypeId> = prepared.boxes.keys().copied().collect();
    box_types.sort();
    let mut box_decls = Writer::new();
    let mut box_defs = Writer::new();
    if !box_types.is_empty() {
        for &t in &box_types {
            let name = e.types.box_name(t);
            box_decls.line_raw(&format!("typedef struct {name} {name};"));
        }
        for &t in &box_types {
            let def = e.types.box_struct(t);
            box_defs.line_raw(&def);
        }
        let statics = e.types.static_boxes.clone();
        for (t, variant, name) in statics {
            let ty = e.types.box_name(t);
            let tag = if e.types.box_tagged(t) {
                format!(", .tag = {variant}")
            } else {
                String::new()
            };
            box_defs.line_raw(&format!("static {ty} {name} = {{.rc = 0{tag}}};"));
        }
        let union: String = box_types
            .iter()
            .enumerate()
            .map(|(i, &t)| format!(" {} a{i};", e.types.box_name(t)))
            .collect();
        box_defs.line_raw(&format!("typedef union {{{union} }} EmAnyBox;"));
        // A box waiting to be freed is named by its offset and type index in 32 bits.
        if box_types.len() > crate::heap::MAX_BOX_TYPES
            || prepared.heap_bytes > crate::heap::MAX_HEAP_BYTES
        {
            return Err(vec![Error::new(format!(
                "there can be at most {} types of heap values, and `heap_bytes` can be at most {} bytes",
                crate::heap::MAX_BOX_TYPES,
                crate::heap::MAX_HEAP_BYTES
            ))]);
        }
        box_defs.line_raw(&format!("#define EMELA_HEAP_BYTES {}", prepared.heap_bytes));
        box_defs.line_raw(crate::heap::HEAP_C);
        let drain = e.drain_function(&box_types);
        box_defs.line_raw(&drain);
        for (k, &t) in box_types.iter().enumerate() {
            let name = e.types.box_name(t);
            box_defs.line_raw(&format!(
                "static inline void em_drop_{name}({name} *p) {{\n  if (em_release(p)) {{\n    em_defer(p, {k});\n    em_drain();\n  }}\n}}"
            ));
        }
    }
    // The static states, their accessors, and the sections around their operations: critical
    // sections only for the states more than one entry uses.
    let mut states = Writer::new();
    for s in &prepared.states {
        let ty = e.types.ctype(s.ty);
        let init = e.types.initializer(s.ty, &s.value);
        let var = s.load.trim_end_matches("_load").to_owned();
        states.line_raw(&format!("static {ty} {var} = {init};"));
        let (enter, exit) = match s.critical {
            true => (
                " emela_board_critical_enter(); ",
                " emela_board_critical_exit(); ",
            ),
            false => ("", ""),
        };
        states.line_raw(&format!("static inline void {}(void) {{{enter}}}", s.enter));
        states.line_raw(&format!("static inline void {}(void) {{{exit}}}", s.exit));
        states.line_raw(&format!(
            "static inline {ty} {}(void) {{ return {var}; }}",
            s.load
        ));
        states.line_raw(&format!(
            "static inline void {}({ty} value) {{ {var} = value; }}",
            s.store
        ));
    }
    let mut out = Writer::new();
    out.comment("Generated by Emela.");
    out.line_raw("#include <stdbool.h>");
    out.line_raw("#include <stdint.h>");
    out.line_raw("#include <math.h>");
    out.include(EMBEDDED_H_FILE);
    out.blank();
    if !tests.is_empty() {
        out.include(crate::TEST_START_H_FILE);
        out.line_raw(TEST_C);
    }
    out.line_raw(PRELUDE);
    out.append(box_decls);
    if e.strings {
        out.line_raw(&format!(
            "#ifndef EMELA_STRING_CAPACITY\n#define EMELA_STRING_CAPACITY {}\n#endif",
            prepared.string_capacity
        ));
        out.line_raw(crate::strings::STRING_C);
        if e.float_strings {
            out.line_raw(crate::strings::FLOAT_STRING_C);
        }
    }
    out.append(std::mem::take(&mut e.types.out));
    out.blank();
    out.append(box_defs);
    if !states.is_empty() {
        out.comment("Static states (@static).");
        out.append(states);
        out.blank();
    }
    out.append(defined);
    out.append(protos);
    out.blank();
    out.append(bodies);
    Ok(Files {
        main: out.finish(),
        board: BOARD_H.to_owned(),
        peripherals: headers,
        embedded: embedded.finish(),
        copies,
        bundled,
    })
}

/// The board hooks (⑤ boundary hooks).
const BOARD_H: &str = r#"#ifndef EMELA_BOARD_H
#define EMELA_BOARD_H

#include <stdint.h>

/* Generated by Emela. The board implements these hooks. */

/* The most elements a List[Int] given to or taken from a peripheral holds (UART, I2C, SPI). */
#ifndef EMELA_TRANSFER_CAPACITY
#define EMELA_TRANSFER_CAPACITY 256
#endif

typedef enum {
  EMELA_DEFECT_PANIC,
  EMELA_DEFECT_TODO,
  EMELA_DEFECT_DIVISION_BY_ZERO,
  EMELA_DEFECT_NO_MATCH,
  EMELA_DEFECT_FLOAT_TO_INT,
  EMELA_DEFECT_NEGATIVE_SHIFT,
  EMELA_DEFECT_ASSERT,
  EMELA_DEFECT_STRING_CAPACITY,
  EMELA_DEFECT_HEAP_EXHAUSTED,
  /* A failure (an error value, spec 7) reached `main` or an interrupt entry. */
  EMELA_DEFECT_UNHANDLED_ERROR
} EmelaDefectKind;

/* Called once at the start, before Emela's `main` (the clock, the HAL). */
void emela_board_init(void);

/* A defect: the program stops here (there is no unwinding). Called from interrupts too, so it
 * must work there: make the outputs safe, then halt or reset. It does not return. */
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length);

/* Called over and over after `main` returns (wait for an interrupt, for example). */
void emela_board_idle(void);

/* The output of `dbg` (a line, without its newline). Needed only by programs that use `dbg`. */
void emela_board_debug(const char *bytes, uint16_t length);

/* The platform handlers (`with Platform`, spec 15.2), needed only by programs that use them.
 * Console writes pieces of a line (the newline is a piece too), usually to a UART. */
void emela_board_console_write(const char *bytes, uint16_t length);
void emela_board_console_error_write(const char *bytes, uint16_t length);
/* Clock: milliseconds since a start the board chooses (the Unix epoch if it has a clock that is
 * set, its start otherwise), and waiting (not from an interrupt). */
int64_t emela_board_now_milliseconds(void);
void emela_board_sleep_milliseconds(int32_t ms);
/* Random: an Int over its whole range, a Float in [0, 1); the board decides their quality. */
int32_t emela_board_random_int(void);
double emela_board_random_float(void);

/* A critical section around each operation of a static state (@static) that more than one entry
 * uses (`main` and an interrupt entry, for example): enter disables interrupts, exit restores what
 * was before. Sections may nest. Needed only by programs with such states. */
void emela_board_critical_enter(void);
void emela_board_critical_exit(void);

#endif
"#;

/// The result lines of a test build (the format of the host test runners), written through
/// `emela_board_console_write`. A defect writes the running test's result before the board stops.
const TEST_C: &str = r#"#include <string.h>

#ifndef EMELA_TEST_START
#define EMELA_TEST_START 0
#endif

/* The name of the running test, for the result of a defect. */
static const char *em_test_current;

static void em_test_write(const char *s) {
  emela_board_console_write(s, (uint16_t)strlen(s));
}

/* `s` inside a JSON string. */
static void em_test_json(const char *s, uint16_t n) {
  static const char hex[] = "0123456789abcdef";
  uint16_t start = 0;
  for (uint16_t i = 0; i < n; i++) {
    unsigned char c = (unsigned char)s[i];
    if (c != '"' && c != '\\' && c >= 0x20) continue;
    if (i > start) emela_board_console_write(s + start, (uint16_t)(i - start));
    char esc[6] = {'\\', (char)c, '0', '0', hex[c >> 4], hex[c & 15]};
    if (c < 0x20) esc[1] = 'u';
    emela_board_console_write(esc, c < 0x20 ? 6 : 2);
    start = (uint16_t)(i + 1);
  }
  if (n > start) emela_board_console_write(s + start, (uint16_t)(n - start));
}

static void em_test_result(const char *outcome, const char *message, uint16_t length) {
  em_test_write("\x1e" "emela-test {\"event\":\"result\",\"name\":\"");
  em_test_json(em_test_current, (uint16_t)strlen(em_test_current));
  em_test_write("\",\"outcome\":\"");
  em_test_write(outcome);
  em_test_write("\",\"message\":\"");
  em_test_json(message, length);
  em_test_write("\"}\n");
}

static _Noreturn void em_test_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  if (em_test_current != NULL) {
    em_test_result("defect", message, length);
    em_test_current = NULL;
  }
  emela_board_defect(kind, message, length);
}
#define emela_board_defect em_test_defect
"#;

/// What an entry does when a failure reaches it.
const UNHANDLED: &str = "emela_board_defect(EMELA_DEFECT_UNHANDLED_ERROR, \"unhandled error\", 15)";

/// Helpers for the integer semantics of spec 16.1 (wrapping, without undefined behavior).
const PRELUDE: &str = r#"#if defined(__GNUC__)
#define EMELA_UNREACHABLE() __builtin_unreachable()
#else
#define EMELA_UNREACHABLE() for (;;) {}
#endif

static inline int32_t em_iadd(int32_t a, int32_t b) { return (int32_t)((uint32_t)a + (uint32_t)b); }
static inline int32_t em_isub(int32_t a, int32_t b) { return (int32_t)((uint32_t)a - (uint32_t)b); }
static inline int32_t em_imul(int32_t a, int32_t b) { return (int32_t)((uint32_t)a * (uint32_t)b); }
static inline int32_t em_ineg(int32_t a) { return (int32_t)(0u - (uint32_t)a); }
/* The divisor is not 0 (the Heap IR tests it first). INT_MIN / -1 wraps. */
static inline int32_t em_idiv(int32_t a, int32_t b) { return b == -1 ? em_ineg(a) : a / b; }
static inline int32_t em_irem(int32_t a, int32_t b) { return b == -1 ? 0 : a % b; }
static inline int64_t em_ladd(int64_t a, int64_t b) { return (int64_t)((uint64_t)a + (uint64_t)b); }
static inline int64_t em_lsub(int64_t a, int64_t b) { return (int64_t)((uint64_t)a - (uint64_t)b); }
static inline int64_t em_lmul(int64_t a, int64_t b) { return (int64_t)((uint64_t)a * (uint64_t)b); }
static inline int64_t em_lneg(int64_t a) { return (int64_t)(0u - (uint64_t)a); }
static inline int64_t em_ldiv(int64_t a, int64_t b) { return b == -1 ? em_lneg(a) : a / b; }
static inline int64_t em_lrem(int64_t a, int64_t b) { return b == -1 ? 0 : a % b; }

static inline void em_shift_check(int32_t by) {
  if (by < 0) emela_board_defect(EMELA_DEFECT_NEGATIVE_SHIFT, "negative shift amount", 21);
}
static inline int32_t em_ishl(int32_t a, int32_t by) { em_shift_check(by); return by >= 32 ? 0 : (int32_t)((uint32_t)a << by); }
static inline int32_t em_ishr(int32_t a, int32_t by) { em_shift_check(by); return a >> (by > 31 ? 31 : by); }
static inline int32_t em_iushr(int32_t a, int32_t by) { em_shift_check(by); return by >= 32 ? 0 : (int32_t)((uint32_t)a >> by); }
static inline int64_t em_lshl(int64_t a, int32_t by) { em_shift_check(by); return by >= 64 ? 0 : (int64_t)((uint64_t)a << by); }
static inline int64_t em_lshr(int64_t a, int32_t by) { em_shift_check(by); return a >> (by > 63 ? 63 : by); }
static inline int64_t em_lushr(int64_t a, int32_t by) { em_shift_check(by); return by >= 64 ? 0 : (int64_t)((uint64_t)a >> by); }

static inline int32_t em_float_to_int(double r) {
  if (!(r >= -2147483648.0 && r <= 2147483647.0)) {
    emela_board_defect(EMELA_DEFECT_FLOAT_TO_INT, "cannot convert Float to Int", 27);
  }
  return (int32_t)r;
}
"#;

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

/// A function name without its type arguments, for C names: `Option.map$is_pressed` for
/// `Option.map[PinState, Bool, {}, {}]$is_pressed`.
fn short_name(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The Console sinks among the reached functions (see [`Emitter::sinks`]).
fn sinks(m: &Module, reached: &[FnId]) -> FxHashMap<FnId, &'static str> {
    let mut out: FxHashMap<FnId, &'static str> = FxHashMap::default();
    for &id in reached {
        if let Some(hook) = console(&m.functions[id]) {
            out.insert(id, hook);
        }
    }
    loop {
        let mut changed = false;
        for &id in reached {
            if out.contains_key(&id) {
                continue;
            }
            if let Some(hook) = forwards_to(&m.functions[id], &out) {
                out.insert(id, hook);
                changed = true;
            }
        }
        if !changed {
            return out;
        }
    }
}

/// The hook of the sink `f` only passes one of its String parameters on to.
fn forwards_to(f: &Function, sinks: &FxHashMap<FnId, &'static str>) -> Option<&'static str> {
    let seq = &f.seqs[f.body?];
    let [s] = seq.stmts.as_slice() else {
        return None;
    };
    if seq.term != Term::Return(None) {
        return None;
    }
    let Stmt::Let {
        dst: None,
        value:
            Value::Call {
                callee: Callee::Direct(g),
                args,
                on_fail: None,
                ..
            },
        ..
    } = &f.stmts[*s]
    else {
        return None;
    };
    let hook = sinks.get(g)?;
    match args.as_slice() {
        [Operand::Var(p)] if f.params.contains(p) => Some(*hook),
        _ => None,
    }
}

/// The value defining a variable by a `let`.
fn definition(f: &Function, v: VarId) -> Option<&Value> {
    f.stmts.iter().find_map(|(_, s)| match s {
        Stmt::Let {
            dst: Some(d),
            value,
            ..
        } if *d == v => Some(value),
        _ => None,
    })
}

/// The strings of `f` written in pieces to a Console line instead of built: the results of
/// `String.append` and `Int.to_string` used once, by a sink's call or by another such string.
fn streamed(f: &Function, sinks: &FxHashMap<FnId, &'static str>) -> FxHashSet<VarId> {
    let mut out = FxHashSet::default();
    if sinks.is_empty() || f.body.is_none() {
        return out;
    }
    let uses = use_counts(f);
    let streamable = |v: VarId| {
        uses.get(&v) == Some(&1)
            && matches!(
                definition(f, v),
                Some(Value::Builtin {
                    op: Builtin::StringAppend | Builtin::IntToString | Builtin::Int64ToString,
                    ..
                })
            )
    };
    let mut pending: Vec<VarId> = Vec::new();
    for (_, s) in f.stmts.iter() {
        if let Stmt::Let {
            value:
                Value::Call {
                    callee: Callee::Direct(g),
                    args,
                    ..
                },
            ..
        } = s
            && sinks.contains_key(g)
        {
            pending.extend(args.iter().filter_map(|a| match a {
                Operand::Var(v) => Some(*v),
                Operand::Const(_) => None,
            }));
        }
    }
    while let Some(v) = pending.pop() {
        if !streamable(v) || !out.insert(v) {
            continue;
        }
        if let Some(Value::Builtin {
            op: Builtin::StringAppend,
            args,
        }) = definition(f, v)
        {
            pending.extend(args.iter().filter_map(|a| match a {
                Operand::Var(x) => Some(*x),
                Operand::Const(_) => None,
            }));
        }
    }
    out
}

/// How many times each variable of `f` is used.
fn use_counts(f: &Function) -> FxHashMap<VarId, usize> {
    let mut uses: FxHashMap<VarId, usize> = FxHashMap::default();
    let mut count = |v: VarId| *uses.entry(v).or_default() += 1;
    for (_, s) in f.stmts.iter() {
        match s {
            Stmt::Let { value, .. } => value.for_each_var(&mut count),
            Stmt::If {
                cond: Operand::Var(v),
                ..
            } => count(*v),
            Stmt::Switch {
                on: SwitchOn::Tag(v) | SwitchOn::Lit(v),
                ..
            } => count(*v),
            Stmt::Loop { inits, .. } => inits.iter().for_each(|o| {
                if let Operand::Var(v) = o {
                    count(*v)
                }
            }),
            _ => {}
        }
    }
    for (_, seq) in f.seqs.iter() {
        let mut op = |o: &Operand| {
            if let Operand::Var(v) = o {
                count(*v)
            }
        };
        match &seq.term {
            Term::Yield(Some(o)) | Term::Return(Some(o)) | Term::Fail(o) | Term::Defect(o) => op(o),
            Term::Break { value: Some(o), .. } => op(o),
            Term::Recur(os) => os.iter().for_each(&mut op),
            _ => {}
        }
    }
    uses
}

/// Whether the function's body uses the variable.
fn uses_var(f: &Function, v: VarId) -> bool {
    let operand = |o: &Operand| *o == Operand::Var(v);
    f.stmts.iter().any(|(_, s)| match s {
        Stmt::Let { value, .. } => {
            let mut used = false;
            value.for_each_var(|x| used |= x == v);
            used
        }
        Stmt::If { cond, .. } => operand(cond),
        Stmt::Switch {
            on: SwitchOn::Tag(x) | SwitchOn::Lit(x),
            ..
        } => *x == v,
        Stmt::Loop { inits, .. } => inits.iter().any(operand),
        Stmt::Dup(x) | Stmt::Drop(x) => *x == v,
        Stmt::Block { .. } | Stmt::Protect { .. } => false,
    }) || f.seqs.iter().any(|(_, seq)| match &seq.term {
        Term::Yield(Some(o)) | Term::Return(Some(o)) | Term::Fail(o) | Term::Defect(o) => {
            operand(o)
        }
        Term::Break { value: Some(o), .. } => operand(o),
        Term::Recur(os) => os.iter().any(operand),
        _ => false,
    })
}

fn var(v: VarId) -> String {
    format!("v{}", ix(v))
}

fn stmt_label(s: StmtId, what: &str) -> String {
    format!("s{}_{what}", ix(s))
}

/// The peripherals the reached functions use (their effects or types), in path order.
fn peripherals(m: &Module, reached: &[FnId]) -> Vec<Peripheral> {
    let mut out: Vec<Peripheral> = Vec::new();
    for &id in reached {
        let f = &m.functions[id];
        if is_board_op(f) {
            out.extend(board::split_op(&f.name));
        }
        for (_, v) in f.vars.iter() {
            if let Type::Enum(e, _) = m.core.ty(v.ty)
                && let Some((p, _)) = board::split(&m.core.enums[*e].name)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The defect kind for a message the Flow IR raises.
fn defect_kind(message: &str) -> &'static str {
    match message {
        "division by zero" => "EMELA_DEFECT_DIVISION_BY_ZERO",
        "unreachable: no match arm matched" => "EMELA_DEFECT_NO_MATCH",
        "not yet implemented" => "EMELA_DEFECT_TODO",
        m if m.starts_with("assertion failed") => "EMELA_DEFECT_ASSERT",
        _ => "EMELA_DEFECT_PANIC",
    }
}

/// C types of Core types, and the definitions of the structs of value aggregates.
///
/// A value aggregate is one of three shapes:
///
/// - a newtype (one constructor with one field that has a value, like `GpioPin(id: Int)`): the
///   field's C type itself, so `GpioPin` is an `int32_t` on the boundary;
/// - a record (one constructor, or a tuple): `struct { T a; U b; }`;
/// - tagged (several constructors): `struct { uint8_t tag; union { struct { … } v0; … } u; }`,
///   with a member of the union for each constructor that has fields.
///
/// Fields are named as declared, positional ones `f0`, `f1`, …. Fields without a value (`()`) take
/// no member. Enums whose constructors take no arguments are `uint8_t` tags.
struct Types<'m> {
    m: &'m Module,
    layouts: &'m FxHashMap<TypeId, Layout>,
    /// The types of the enums declared in peripheral modules (without type parameters).
    board_types: &'m FxHashMap<EnumId, TypeId>,
    /// The error enums of `EmError`, in tag order.
    errors: &'m [(EnumId, TypeId)],
    /// The boxes of the heap values.
    boxes: &'m FxHashMap<TypeId, Boxed>,
    /// The static boxes of constructors without fields, by (type, constructor): their names.
    static_boxes: Vec<(TypeId, usize, String)>,
    /// The C names of the structs of the program's value aggregates.
    names: FxHashMap<TypeId, String>,
    used: FxHashSet<String>,
    /// The structs already written to `out`.
    defined: FxHashSet<TypeId>,
    /// The definitions of the program's structs, dependencies first.
    out: Writer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A host handle (`@external type`): `void *`.
    Opaque,
    /// A cell (the state of `State.run`): a pointer to a local holding the value.
    Cell,
    /// A String: `EmString` (see `strings.rs`).
    String,
    /// A caught failure: `EmError`, a tag (which error enum, 0 for none) and a union of them.
    Failure,
    Newtype,
    Record,
    Tagged,
}

impl Types<'_> {
    fn core(&self) -> &emela_core::Module {
        &self.m.core
    }

    /// Whether values of the type have no C value: `()`, Never and [`no_data`] tuples.
    fn is_void(&self, ty: TypeId) -> bool {
        self.m.repr(ty) == Repr::Void || no_data(self.m, ty)
    }

    /// The board name of a type declared in a peripheral module.
    fn board_name(&self, ty: TypeId) -> Option<String> {
        let Type::Enum(id, args) = self.core().ty(ty) else {
            return None;
        };
        let def = &self.core().enums[*id];
        if !args.is_empty() {
            return None;
        }
        board::split(&def.name).map(|(p, item)| p.type_name(item))
    }

    fn layout(&self, ty: TypeId) -> &Layout {
        self.layouts
            .get(&ty)
            .unwrap_or_else(|| panic!("no layout for {}", emela_lir::core_ty(self.core(), ty)))
    }

    /// The fields with a value of each constructor (or of the tuple): (field index, C member, type).
    fn members(&self, ty: TypeId) -> Vec<Vec<(usize, String, TypeId)>> {
        let core = self.core();
        match self.layout(ty) {
            Layout::Tuple(ts) => vec![
                ts.iter()
                    .enumerate()
                    .filter(|(_, t)| !self.is_void(**t))
                    .map(|(i, &t)| (i, format!("f{i}"), t))
                    .collect(),
            ],
            Layout::Enum(variants) => {
                let Type::Enum(id, _) = core.ty(ty) else {
                    unreachable!("an enum layout of a non-enum")
                };
                let def = &core.enums[*id];
                variants
                    .iter()
                    .zip(&def.variants)
                    .map(|(tys, v)| {
                        tys.iter()
                            .enumerate()
                            .filter(|(_, t)| !self.is_void(**t))
                            .map(|(i, &t)| {
                                let name = match &v.fields {
                                    VariantFields::Named(ns) => member_name(&ns[i]),
                                    _ => format!("f{i}"),
                                };
                                (i, name, t)
                            })
                            .collect()
                    })
                    .collect()
            }
            Layout::Opaque | Layout::Cell(_) | Layout::String | Layout::Failure => vec![Vec::new()],
        }
    }

    fn shape(&self, ty: TypeId) -> Shape {
        match self.layout(ty) {
            Layout::Opaque => Shape::Opaque,
            Layout::Cell(_) => Shape::Cell,
            Layout::String => Shape::String,
            Layout::Failure => Shape::Failure,
            Layout::Tuple(_) => Shape::Record,
            Layout::Enum(vs) if vs.len() > 1 => Shape::Tagged,
            Layout::Enum(_) => {
                if self.members(ty)[0].len() == 1 {
                    Shape::Newtype
                } else {
                    Shape::Record
                }
            }
        }
    }

    /// The C type of a value of the Core type.
    fn ctype(&mut self, ty: TypeId) -> String {
        if self.is_void(ty) {
            return "void".into();
        }
        match self.core().ty(ty) {
            Type::Int => "int32_t".into(),
            Type::Bool => "bool".into(),
            Type::Int64 => "int64_t".into(),
            Type::Float => "double".into(),
            Type::Unit | Type::Never => "void".into(),
            _ => match self.m.repr(ty) {
                Repr::I32 => self.board_name(ty).unwrap_or_else(|| "uint8_t".into()),
                Repr::Agg => {
                    match self.layout(ty).clone() {
                        Layout::Opaque => return "void *".into(),
                        Layout::Cell(t) => return format!("{} *", self.ctype(t)),
                        Layout::String => return "EmString".into(),
                        Layout::Failure => return self.define_failure(),
                        _ => {}
                    }
                    if let Some(name) = self.board_name(ty) {
                        return name;
                    }
                    if self.shape(ty) == Shape::Newtype {
                        let field = self.members(ty)[0][0].2;
                        return self.ctype(field);
                    }
                    self.define(ty)
                }
                Repr::Ref if self.boxes.contains_key(&ty) => format!("{} *", self.box_name(ty)),
                // Rejected by the checker.
                _ => "void *".into(),
            },
        }
    }

    /// The initializer of a local of the type: `0`, `{0}` for a struct, none for a String (always
    /// set before it is read; zeroing its buffer would cost its capacity).
    fn initial(&self, ty: TypeId) -> Option<&'static str> {
        if matches!(self.core().ty(ty), Type::String) {
            return None;
        }
        if self.m.repr(ty) != Repr::Agg {
            return Some("0");
        }
        match self.shape(ty) {
            Shape::Newtype => self.initial(self.members(ty)[0][0].2),
            Shape::Opaque | Shape::Cell => Some("0"),
            Shape::String => None,
            Shape::Record | Shape::Tagged | Shape::Failure => Some("{0}"),
        }
    }

    /// The C name of the box struct of a heap type: `EmBList_Int`.
    fn box_name(&self, ty: TypeId) -> String {
        format!("EmB{}", mangle(self.core(), ty))
    }

    /// The fields with a value of each constructor of a box (as [`Types::members`]).
    fn box_members(&self, ty: TypeId) -> Vec<Vec<(usize, String, TypeId)>> {
        let keep = |s: &Self, ts: &[TypeId], name: &dyn Fn(usize) -> String| {
            ts.iter()
                .enumerate()
                .filter(|(_, t)| !s.is_void(**t))
                .map(|(i, &t)| (i, name(i), t))
                .collect::<Vec<_>>()
        };
        match &self.boxes[&ty] {
            Boxed::Tuple(ts) => vec![keep(self, ts, &|i| format!("f{i}"))],
            Boxed::List(t) => vec![keep(self, &[*t, ty], &|i| {
                if i == 0 { "head" } else { "tail" }.to_owned()
            })],
            Boxed::Cell(t) => vec![keep(self, &[*t], &|_| "value".to_owned())],
            Boxed::Enum(variants) => {
                let Type::Enum(id, _) = self.core().ty(ty) else {
                    return Vec::new();
                };
                let def = &self.core().enums[*id];
                variants
                    .iter()
                    .zip(&def.variants)
                    .map(|(tys, v)| {
                        keep(self, tys, &|i| match &v.fields {
                            VariantFields::Named(ns) => member_name(&ns[i]),
                            _ => format!("f{i}"),
                        })
                    })
                    .collect()
            }
        }
    }

    fn box_tagged(&self, ty: TypeId) -> bool {
        matches!(&self.boxes[&ty], Boxed::Enum(vs) if vs.len() > 1)
    }

    /// The definition of a box struct.
    fn box_struct(&mut self, ty: TypeId) -> String {
        let name = self.box_name(ty);
        let members = self.box_members(ty);
        let fields = |s: &mut Self, ms: &[(usize, String, TypeId)]| -> String {
            ms.iter()
                .map(|(_, n, t)| format!(" {} {n};", s.ctype(*t)))
                .collect::<String>()
        };
        let body = if self.box_tagged(ty) {
            let mut union = String::new();
            for (i, ms) in members.iter().enumerate() {
                if !ms.is_empty() {
                    union.push_str(&format!(" struct {{{} }} v{i};", fields(self, ms)));
                }
            }
            if union.is_empty() {
                " uint8_t tag;".to_owned()
            } else {
                format!(" uint8_t tag; union {{{union} }} u;")
            }
        } else {
            fields(self, &members[0])
        };
        format!("struct {name} {{ uint32_t rc;{body} }};")
    }

    /// A field of the box `base` of type `ty` (a pointer).
    fn box_field(&self, base: &str, ty: TypeId, variant: usize, index: usize) -> String {
        let members = self.box_members(ty);
        let member = members
            .get(variant)
            .and_then(|ms| ms.iter().find(|(i, _, _)| *i == index))
            .map(|(_, n, _)| n.clone())
            .unwrap_or_default();
        if self.box_tagged(ty) {
            format!("{base}->u.v{variant}.{member}")
        } else {
            format!("{base}->{member}")
        }
    }

    /// The static box of constructor `variant` (without fields) of `ty`.
    fn static_box(&mut self, ty: TypeId, variant: usize) -> String {
        if let Some((_, _, n)) = self
            .static_boxes
            .iter()
            .find(|(t, v, _)| *t == ty && *v == variant)
        {
            return format!("&{n}");
        }
        let name = format!("em_static_{}_{variant}", self.box_name(ty));
        self.static_boxes.push((ty, variant, name.clone()));
        format!("&{name}")
    }

    /// `EmError`, defined (after the error enums' structs) on first use.
    fn define_failure(&mut self) -> String {
        const NAME: &str = "EmError";
        if self.used.insert(NAME.into()) {
            let errors = self.errors.to_vec();
            let mut members = String::new();
            for (i, (_, ty)) in errors.iter().enumerate() {
                if !self.is_void(*ty) {
                    let c = self.ctype(*ty);
                    members.push_str(&format!(" {c} e{};", i + 1));
                }
            }
            let union = if members.is_empty() {
                String::new()
            } else {
                format!(" union {{{members} }} u;")
            };
            self.out
                .line_raw(&format!("typedef struct {{ uint8_t tag;{union} }} {NAME};"));
        }
        NAME.into()
    }

    /// The tag of error enum `id` in `EmError`.
    fn failure_tag(&self, id: EnumId) -> usize {
        self.errors
            .iter()
            .position(|(e, _)| *e == id)
            .map_or(0, |i| i + 1)
    }

    /// The name of the program's struct for `ty`, writing its definition first if needed.
    fn define(&mut self, ty: TypeId) -> String {
        if let Some(n) = self.names.get(&ty) {
            return n.clone();
        }
        let base = format!("Em{}", mangle(self.core(), ty));
        let mut name = base.clone();
        let mut n = 1;
        while !self.used.insert(name.clone()) {
            name = format!("{base}_{n}");
            n += 1;
        }
        self.names.insert(ty, name.clone());
        let body = self.struct_body(ty);
        if self.defined.insert(ty) {
            self.out
                .line_raw(&format!("typedef struct {{{body}}} {name};"));
        }
        name
    }

    /// The members of the struct of `ty`, between its braces.
    fn struct_body(&mut self, ty: TypeId) -> String {
        let members = self.members(ty);
        let fields = |s: &mut Self, ms: &[(usize, String, TypeId)]| -> String {
            ms.iter()
                .map(|(_, n, t)| format!(" {} {n};", s.ctype(*t)))
                .collect::<String>()
        };
        match self.shape(ty) {
            Shape::Tagged => {
                let mut union = String::new();
                for (i, ms) in members.iter().enumerate() {
                    if !ms.is_empty() {
                        union.push_str(&format!(" struct {{{} }} v{i};", fields(self, ms)));
                    }
                }
                format!(" uint8_t tag; union {{{union} }} u; ")
            }
            _ if members[0].is_empty() => " uint8_t empty; ".into(),
            _ => format!("{} ", fields(self, &members[0])),
        }
    }

    /// The C expression of field `index` of constructor `variant` of `base` (of type `ty`).
    fn field(&self, base: &str, ty: TypeId, variant: usize, index: usize) -> String {
        let members = self.members(ty);
        let member = members[variant]
            .iter()
            .find(|(i, _, _)| *i == index)
            .map(|(_, n, _)| n.clone())
            .unwrap_or_default();
        match self.shape(ty) {
            Shape::Newtype | Shape::Opaque | Shape::Cell | Shape::String | Shape::Failure => {
                base.to_owned()
            }
            Shape::Record => format!("{base}.{member}"),
            Shape::Tagged => format!("{base}.u.v{variant}.{member}"),
        }
    }

    /// A value of constructor `variant` of `ty` with the fields' C values (`None` for fields
    /// without a value).
    fn make(&mut self, ty: TypeId, variant: usize, args: &[Option<String>]) -> String {
        if self.m.repr(ty) == Repr::I32 {
            return self.tag_constant(ty, variant);
        }
        let members = self.members(ty);
        let inits = |ms: &[(usize, String, TypeId)]| -> Vec<String> {
            ms.iter()
                .filter_map(|(i, n, _)| {
                    args.get(*i)
                        .cloned()
                        .flatten()
                        .map(|a| format!(".{n} = {a}"))
                })
                .collect()
        };
        let name = self.ctype(ty);
        match self.shape(ty) {
            Shape::Newtype | Shape::Opaque | Shape::Cell | Shape::String | Shape::Failure => args
                .iter()
                .flatten()
                .next()
                .cloned()
                .unwrap_or_else(|| "0".into()),
            Shape::Record if members[0].is_empty() => format!("({name}){{0}}"),
            Shape::Record => format!("({name}){{{}}}", inits(&members[0]).join(", ")),
            Shape::Tagged => {
                let tag = self.tag_constant(ty, variant);
                let ms = &members[variant];
                if ms.is_empty() {
                    format!("({name}){{.tag = {tag}}}")
                } else {
                    format!(
                        "({name}){{.tag = {tag}, .u.v{variant} = {{{}}}}}",
                        inits(ms).join(", ")
                    )
                }
            }
        }
    }

    /// A C initializer (a constant expression) for a value of `ty` known at compile time.
    fn initializer(&mut self, ty: TypeId, v: &emela_lir::ConstValue) -> String {
        use emela_lir::ConstValue;
        match v {
            ConstValue::I32(n) => match self.core().ty(ty) {
                Type::Bool => if *n != 0 { "true" } else { "false" }.into(),
                Type::Enum(..) => self.tag_constant(ty, *n as usize),
                _ => names::int32(*n),
            },
            ConstValue::I64(n) => names::int64(*n),
            ConstValue::F64(x) => names::float(*x),
            ConstValue::Unit => "0".into(),
            ConstValue::Agg { tag, fields } => {
                let members = self.members(ty);
                let inits = |s: &mut Self, ms: &[(usize, String, TypeId)]| -> Vec<String> {
                    ms.iter()
                        .filter_map(|(i, n, t)| {
                            fields
                                .get(*i)
                                .map(|f| format!(".{n} = {}", s.initializer(*t, f)))
                        })
                        .collect()
                };
                match self.shape(ty) {
                    Shape::Newtype => {
                        let (i, _, t) = members[0][0].clone();
                        fields
                            .get(i)
                            .map_or_else(|| "0".into(), |f| self.initializer(t, f))
                    }
                    Shape::Record if members[0].is_empty() => "{0}".into(),
                    Shape::Record => format!("{{{}}}", inits(self, &members[0]).join(", ")),
                    Shape::Tagged => {
                        let constant = self.tag_constant(ty, *tag);
                        let ms = members.get(*tag).cloned().unwrap_or_default();
                        if ms.is_empty() {
                            format!("{{.tag = {constant}}}")
                        } else {
                            format!(
                                "{{.tag = {constant}, .u.v{tag} = {{{}}}}}",
                                inits(self, &ms).join(", ")
                            )
                        }
                    }
                    Shape::Opaque | Shape::Cell | Shape::String | Shape::Failure => "0".into(),
                }
            }
        }
    }

    /// The constructor index as C: a board constant or a number.
    fn tag_constant(&self, ty: TypeId, variant: usize) -> String {
        if let Type::Enum(id, args) = self.core().ty(ty)
            && args.is_empty()
        {
            let def = &self.core().enums[*id];
            if let Some((p, item)) = board::split(&def.name) {
                return p.constant(item, &def.variants[variant].name);
            }
        }
        variant.to_string()
    }

    /// The tag of a value of `ty` held in `base`.
    fn tag(&self, base: &str, ty: TypeId) -> String {
        match self.m.repr(ty) {
            Repr::I32 => base.to_owned(),
            Repr::Ref if self.boxes.contains_key(&ty) => {
                if self.box_tagged(ty) {
                    format!("{base}->tag")
                } else {
                    "0".into()
                }
            }
            _ if self.shape(ty) == Shape::Tagged => format!("{base}.tag"),
            _ => "0".into(),
        }
    }

    /// The header of a peripheral: its types and its effect operations, from the declarations of
    /// its module.
    fn peripheral_header(&mut self, p: &Peripheral) -> String {
        let core = self.core();
        let guard = p.header().replace('.', "_").to_uppercase();
        let mut w = Writer::new();
        w.line_raw(&format!("#ifndef {guard}"));
        w.line_raw(&format!("#define {guard}"));
        w.blank();
        w.line_raw("#include <stdbool.h>");
        w.line_raw("#include <stdint.h>");
        w.blank();
        w.comment(&format!("Generated by Emela from {}.", p.path));
        let mut enums: Vec<TypeId> = Vec::new();
        let ids: Vec<_> = core
            .enums
            .iter()
            .filter(|(_, e)| {
                e.params == 0 && !e.external && board::split(&e.name).is_some_and(|(q, _)| q == *p)
            })
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            if let Some(&ty) = self.board_types.get(&id) {
                enums.push(ty);
            }
        }
        let mut written = FxHashSet::default();
        for ty in enums.clone() {
            self.board_type(&mut w, ty, &enums, &mut written);
        }
        let effects: Vec<_> = self
            .core()
            .effects
            .iter()
            .filter(|(_, e)| board::split(&e.name).is_some_and(|(q, _)| q == *p))
            .map(|(id, _)| id)
            .collect();
        for id in effects {
            let effect = &self.m.core.effects[id];
            w.blank();
            w.comment(&format!(
                "effect {}: the board implements its operations. Those called from an interrupt \
                 must be safe there.",
                board::split(&effect.name).map_or("", |(_, i)| i)
            ));
            for op in &effect.ops {
                match self.op_signature(p, op) {
                    Some(sig) => w.extern_proto(&sig),
                    None => w.comment(&format!(
                        "{} is not available yet (its types need the heap)",
                        op.name
                    )),
                }
            }
        }
        w.blank();
        w.line_raw("#endif");
        w.finish()
    }

    /// Writes the definition of a board type, after the board types it contains.
    fn board_type(
        &mut self,
        w: &mut Writer,
        ty: TypeId,
        board: &[TypeId],
        written: &mut FxHashSet<TypeId>,
    ) {
        if !written.insert(ty) {
            return;
        }
        let Some(name) = self.board_name(ty) else {
            return;
        };
        let Type::Enum(id, _) = self.core().ty(ty) else {
            return;
        };
        let def = self.core().enums[*id].clone();
        match self.m.repr(ty) {
            Repr::I32 => {
                w.line_raw(&format!("typedef uint8_t {name};"));
                let consts: Vec<String> = (0..def.variants.len())
                    .map(|i| format!("{} = {i}", self.tag_constant(ty, i)))
                    .collect();
                w.line_raw(&format!("enum {{ {} }};", consts.join(", ")));
            }
            Repr::Agg => {
                for row in self.members(ty) {
                    for (_, _, t) in row {
                        if board.contains(&t) {
                            self.board_type(w, t, board, written);
                        }
                    }
                }
                if self.shape(ty) == Shape::Newtype {
                    let field = self.members(ty)[0][0].2;
                    let inner = self.ctype(field);
                    w.line_raw(&format!("typedef {inner} {name};"));
                } else {
                    let body = self.struct_body(ty);
                    w.line_raw(&format!("typedef struct {{{body}}} {name};"));
                    if self.shape(ty) == Shape::Tagged {
                        let consts: Vec<String> = (0..def.variants.len())
                            .map(|i| format!("{} = {i}", self.tag_constant(ty, i)))
                            .collect();
                        w.line_raw(&format!("enum {{ {} }};", consts.join(", ")));
                    }
                }
            }
            _ => w.comment(&format!("{name} is not available yet (it needs the heap)")),
        }
    }

    /// `void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state)`, if every type has a
    /// C value.
    ///
    /// An operation that may fail (`fails PeripheralError`) returns an `int32_t` code (0 for
    /// success, any other for `PeripheralError(code)`) and writes a value result to `out`. A
    /// `List[Int]` argument is `const uint16_t *data, uint16_t data_length`; a `List[Int]` result
    /// is written to `out` (of `capacity` elements) with its length in `length`.
    fn op_signature(&mut self, p: &Peripheral, op: &emela_core::OpDef) -> Option<String> {
        let value = |s: &Self, t: TypeId| {
            matches!(
                s.m.repr(t),
                Repr::I32 | Repr::I64 | Repr::F64 | Repr::Agg | Repr::Void
            )
        };
        if op.suspend {
            return None;
        }
        let mut used = FxHashSet::default();
        let mut unique = |name: String| {
            let mut n = 1;
            let mut out = name.clone();
            while !used.insert(out.clone()) {
                out = format!("{name}{n}");
                n += 1;
            }
            out
        };
        let mut params: Vec<String> = Vec::new();
        for (k, &t) in op.params.iter().enumerate() {
            if self.is_void(t) {
                continue;
            }
            let declared = op.param_names.get(k).map(|n| member_name(n));
            if self.is_int_list(t) {
                let name = unique(declared.unwrap_or_else(|| "data".into()));
                params.push(format!("const uint16_t *{name}"));
                params.push(param("uint16_t", &format!("{name}_length")));
            } else if value(self, t) {
                let name = unique(declared.unwrap_or_else(|| self.param_name(p, t)));
                params.push(param(&self.ctype(t), &name));
            } else {
                return None;
            }
        }
        let mut ret = if self.is_int_list(op.ret) {
            params.push("uint16_t *out".into());
            params.push("uint16_t capacity".into());
            params.push("uint16_t *length".into());
            "void".to_owned()
        } else if !value(self, op.ret) {
            return None;
        } else if op.fails && !self.is_void(op.ret) {
            params.push(format!("{} *out", self.ctype(op.ret)));
            "void".to_owned()
        } else {
            self.ctype(op.ret)
        };
        if op.fails {
            ret = "int32_t".into();
        }
        Some(signature(&ret, &p.op_function(&op.name), params))
    }

    /// Whether the type is `List[Int]`, which board operations take as an array.
    fn is_int_list(&self, ty: TypeId) -> bool {
        matches!(self.core().ty(ty), Type::List(e) if matches!(self.core().ty(*e), Type::Int))
    }

    /// A parameter name for a value of the type: the type's name in snake case without the
    /// peripheral (`pin` for `GpioPin`, `state` for `PinState`).
    fn param_name(&self, p: &Peripheral, ty: TypeId) -> String {
        let item = match self.core().ty(ty) {
            Type::Enum(id, _) => {
                let name = &self.core().enums[*id].name;
                name.rsplit('.').next().unwrap_or("value").to_owned()
            }
            Type::Int | Type::Int64 => "value".into(),
            Type::Float => "value".into(),
            Type::Bool => "flag".into(),
            _ => "value".into(),
        };
        let item = item.strip_prefix(&p.short).unwrap_or(&item).to_owned();
        let item = match item.as_str() {
            "PinState" => "State".to_owned(),
            _ => item,
        };
        let mut out = String::new();
        for (i, c) in item.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        member_name(&out)
    }
}

/// A C member name for a field name, avoiding C keywords.
fn member_name(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
        "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register",
        "restrict", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef",
        "union", "unsigned", "void", "volatile", "while", "bool", "true", "false", "tag", "u",
    ];
    if KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_owned()
    }
}

/// A readable C name part for a type: `Option_PinState`, `Tuple2_Int_Bool`.
fn mangle(core: &emela_core::Module, ty: TypeId) -> String {
    match core.ty(ty) {
        Type::Int => "Int".into(),
        Type::Bool => "Bool".into(),
        Type::Int64 => "Int64".into(),
        Type::Float => "Float".into(),
        Type::Unit => "Unit".into(),
        Type::Enum(id, args) => {
            let name = core.enums[*id]
                .name
                .rsplit('.')
                .next()
                .unwrap_or("T")
                .to_owned();
            let mut out: String = name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            for &a in args {
                out.push('_');
                out.push_str(&mangle(core, a));
            }
            out
        }
        Type::Cell(t) => format!("Cell{}", mangle(core, *t)),
        Type::List(t) => format!("List_{}", mangle(core, *t)),
        Type::String => "String".into(),
        Type::Any => "Failure".into(),
        Type::Tuple(ts) => {
            let mut out = format!("Tuple{}", ts.len());
            for &t in ts {
                out.push('_');
                out.push_str(&mangle(core, t));
            }
            out
        }
        _ => "T".into(),
    }
}

/// Where a `yield` goes: the destination of the enclosing statement and the label after it.
#[derive(Clone)]
struct Yield {
    dst: Option<VarId>,
    end: String,
    /// Whether the end of the sequence is right before the label (its final `yield` needs no
    /// `goto`).
    falls: bool,
}

impl Yield {
    fn new(dst: Option<VarId>, end: String) -> Self {
        Yield {
            dst,
            end,
            falls: false,
        }
    }

    fn falling(&self) -> Self {
        Yield {
            falls: true,
            ..self.clone()
        }
    }
}

/// The function being emitted.
struct Cx<'a> {
    f: &'a Function,
    w: Writer,
    labels: FxHashMap<LabelId, Yield>,
    loops: Vec<(Vec<VarId>, String)>,
    used_labels: FxHashSet<String>,
    /// The locals holding cells' values, (C type, name).
    slots: Vec<(String, String)>,
    /// Strings that are not built: their pieces are written where a Console line uses them.
    streamed: FxHashSet<VarId>,
    /// The variables nothing uses: not declared or assigned (calls defining them still run).
    unused: FxHashSet<VarId>,
}

impl Cx<'_> {
    fn goto(&mut self, label: &str) {
        self.used_labels.insert(label.to_owned());
        self.w.goto(label);
    }

    fn label(&mut self, label: &str) {
        if self.used_labels.contains(label) {
            self.w.label(label);
        }
    }
}

struct Emitter<'m> {
    m: &'m Module,
    types: Types<'m>,
    consts: &'m FxHashMap<FnId, emela_lir::ConstValue>,
    fn_names: Vec<String>,
    errors: Vec<Error>,
    /// Whether the program uses strings (then `main.c` gets the String C, `strings.rs`).
    strings: bool,
    /// Whether it formats or parses Floats as strings (which needs the C library's).
    float_strings: bool,
    /// The functions a Console line goes to, with the board hook its pieces are written to: the
    /// external functions of `Std.Console`, and the functions that only pass their String on to
    /// one (the operations of `SystemConsole`).
    sinks: FxHashMap<FnId, &'static str>,
}

impl Emitter<'_> {
    fn is_string(&self, ty: TypeId) -> bool {
        matches!(self.m.core.ty(ty), Type::String)
    }

    /// A String operand as a view (`EmStr`): parameters are views already, other variables
    /// hold an `EmString`.
    fn view(&mut self, cx: &Cx<'_>, o: &Operand) -> String {
        self.strings = true;
        match o {
            Operand::Const(Const::String(s)) => format!(
                "em_lit({}, {})",
                names::string_literal(s.as_bytes()),
                s.len()
            ),
            Operand::Var(v) if cx.f.params.contains(v) => var(*v),
            Operand::Var(v) => format!("em_view(&{})", var(*v)),
            Operand::Const(_) => "em_lit(\"\", 0)".into(),
        }
    }

    /// Whether the operand is a String.
    fn is_string_operand(&self, cx: &Cx<'_>, o: &Operand) -> bool {
        match o {
            Operand::Const(Const::String(_)) => true,
            Operand::Var(v) => self.is_string(cx.f.vars[*v].ty),
            Operand::Const(_) => false,
        }
    }

    /// `dst = o` (a String is copied as `em_str_set`).
    fn set_var(&mut self, cx: &mut Cx<'_>, dst: VarId, o: &Operand) {
        if self.is_string(cx.f.vars[dst].ty) {
            let v = self.view(cx, o);
            cx.w.stmt(&format!("em_str_set(&{}, {v})", var(dst)));
        } else if let Some(v) = self.operand(o) {
            cx.w.assign(&var(dst), &v);
        }
    }

    /// An operand stored into a field of an aggregate: a String becomes an `EmString` value.
    fn field_value(&mut self, cx: &Cx<'_>, o: &Operand) -> Option<String> {
        if self.is_string_operand(cx, o) {
            let v = self.view(cx, o);
            Some(format!("em_string_of({v})"))
        } else {
            self.operand(o)
        }
    }

    /// The C type of a parameter: a String is passed as a view.
    fn param_type(&mut self, ty: TypeId) -> String {
        if self.is_string(ty) {
            self.strings = true;
            "EmStr".into()
        } else {
            self.types.ctype(ty)
        }
    }
    fn internal(&mut self, message: impl Into<String>) {
        self.errors.push(Error::new(format!(
            "internal error in the embedded C emitter: {}",
            message.into()
        )));
    }

    /// Whether the variable has a C value (see [`no_data`]).
    fn has_value(&self, f: &Function, v: VarId) -> bool {
        !no_data(self.m, f.vars[v].ty)
    }

    fn value_params(&self, f: &Function) -> Vec<VarId> {
        f.params
            .iter()
            .copied()
            .filter(|&p| self.has_value(f, p))
            .collect()
    }

    /// A function returning a String writes it to `em_ret`, given by the caller.
    fn signature(&mut self, id: FnId, f: &Function) -> String {
        let mut params: Vec<String> = self
            .value_params(f)
            .into_iter()
            .map(|p| {
                let ty = self.param_type(f.vars[p].ty);
                param(&ty, &var(p))
            })
            .collect();
        let ret = if self.is_string(f.ret) {
            params.push("EmString *em_ret".into());
            "void".into()
        } else {
            self.types.ctype(f.ret)
        };
        if f.fails {
            let e = self.types.define_failure();
            params.push(format!("{e} *em_fail"));
        }
        signature(&ret, &self.fn_names[ix(id)], params)
    }

    /// The return after a failure: any value of the function's type.
    fn dummy_return(&mut self, cx: &mut Cx<'_>) {
        let ret = cx.f.ret;
        if self.types.is_void(ret) || self.is_string(ret) {
            cx.w.ret(None);
        } else if self.types.initial(ret) == Some("0") {
            cx.w.ret(Some("0"));
        } else {
            let ty = self.types.ctype(ret);
            cx.w.ret(Some(&format!("({ty}){{0}}")));
        }
    }

    /// A failure value (`EmError`) of an error value or a caught failure.
    fn failure(&mut self, cx: &Cx<'_>, o: &Operand) -> String {
        let Operand::Var(v) = o else {
            return "(EmError){0}".into();
        };
        let ty = cx.f.vars[*v].ty;
        match self.m.core.ty(ty) {
            Type::Enum(id, _) => {
                let tag = self.types.failure_tag(*id);
                let e = self.types.define_failure();
                if self.types.is_void(ty) {
                    format!("({e}){{.tag = {tag}}}")
                } else {
                    format!("({e}){{.tag = {tag}, .u.e{tag} = {}}}", var(*v))
                }
            }
            _ => var(*v),
        }
    }

    /// The prototype of an external function, with its parameters' names.
    ///
    /// A String argument is `const char *name, uint16_t name_length` (NUL-terminated too); a
    /// String result is written by the C function to `out` (of `capacity` bytes), and it returns
    /// the length (`uint16_t f(..., char *out, uint16_t capacity)`).
    fn external_signature(&mut self, id: FnId, f: &Function) -> String {
        let mut params: Vec<String> = Vec::new();
        for &p in &f.params {
            let name = member_name(&f.vars[p].hint);
            if self.is_string(f.vars[p].ty) {
                params.push(param("const char *", &name).replace("* ", "*"));
                params.push(param("uint16_t", &format!("{name}_length")));
            } else {
                params.push(param(&self.types.ctype(f.vars[p].ty), &name));
            }
        }
        let ret = if self.is_string(f.ret) {
            params.push("char *out".into());
            params.push("uint16_t capacity".into());
            "uint16_t".into()
        } else {
            self.types.ctype(f.ret)
        };
        signature(&ret, &self.fn_names[ix(id)], params)
    }

    /// Records the file an external function names, to be written next to the output under its
    /// own name. `names` maps the output names taken so far to their sources.
    fn copy(
        &mut self,
        module: emela_core::HostModule,
        names: &mut FxHashMap<String, String>,
        copies: &mut Vec<(String, std::path::PathBuf)>,
        bundled: &mut Vec<(String, String)>,
    ) {
        use emela_core::HostModule;
        let (HostModule::File(path) | HostModule::Bundled(path)) = &module else {
            return;
        };
        let file = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match names.get(&file) {
            Some(source) if source == path => return,
            Some(_) => {
                self.errors.push(Error::new(format!(
                    "external functions point to two C files named `{file}`: the output can hold only one file of a name"
                )));
                return;
            }
            None => {
                names.insert(file.clone(), path.clone());
            }
        }
        if matches!(module, HostModule::File(_)) {
            copies.push((file, path.into()));
        } else {
            match self.m.core.bundled_files.iter().find(|b| &b.path == path) {
                Some(b) => bundled.push((file, b.contents.clone())),
                None => self.internal(format!("the bundled file {path} is missing")),
            }
        }
    }

    /// The prototype of an exported function. Strings are passed as to external functions (see
    /// [`Self::external_signature`]).
    fn export_signature(&mut self, export: &str, f: &Function) -> String {
        let mut params: Vec<String> = Vec::new();
        for p in self.value_params(f) {
            if self.is_string(f.vars[p].ty) {
                params.push(param("const char *", &var(p)).replace("* ", "*"));
                params.push(param("uint16_t", &format!("{}_length", var(p))));
            } else {
                params.push(param(&self.types.ctype(f.vars[p].ty), &var(p)));
            }
        }
        let ret = if self.is_string(f.ret) {
            params.push("char *out".into());
            params.push("uint16_t capacity".into());
            "uint16_t".into()
        } else {
            self.types.ctype(f.ret)
        };
        signature(&ret, export, params)
    }

    fn function(&mut self, out: &mut Writer, id: FnId, f: &Function) {
        let Some(body) = f.body else {
            self.internal(format!("fn {} has no body", f.name));
            return;
        };
        // A const evaluated at compile time is read-only data.
        if let Some(value) = self.consts.get(&id) {
            let sig = self.signature(id, f);
            out.comment(&f.name);
            if self.types.is_void(f.ret) {
                out.static_func(&sig, |w| w.ret(None));
                return;
            }
            let ty = self.types.ctype(f.ret);
            let init = self.types.initializer(f.ret, value);
            let data = format!("{}_value", self.fn_names[ix(id)]);
            out.line_raw(&format!("static const {ty} {data} = {init};"));
            out.static_func(&sig, |w| w.ret(Some(&data)));
            return;
        }
        let mut cx = Cx {
            f,
            w: Writer::indented(1),
            labels: FxHashMap::default(),
            loops: Vec::new(),
            used_labels: FxHashSet::default(),
            slots: Vec::new(),
            streamed: streamed(f, &self.sinks),
            unused: {
                let uses = use_counts(f);
                f.vars
                    .iter()
                    .map(|(v, _)| v)
                    .filter(|v| !f.params.contains(v) && !uses.contains_key(v))
                    .collect()
            },
        };
        self.seq(&mut cx, body, None);
        let sig = self.signature(id, f);
        let mut decls = Vec::new();
        for (v, data) in f.vars.iter() {
            if f.params.contains(&v)
                || !self.has_value(f, v)
                || cx.streamed.contains(&v)
                || cx.unused.contains(&v)
            {
                continue;
            }
            let ty = self.types.ctype(data.ty);
            let init = self.types.initial(data.ty);
            decls.push((ty, var(v), init));
        }
        out.comment(&f.name);
        let slots = std::mem::take(&mut cx.slots);
        let params = self.value_params(f);
        out.static_func(&sig, |w| {
            for (ty, name, init) in &decls {
                w.decl(ty, name, *init);
            }
            for (ty, name) in &slots {
                w.decl(ty, name, None);
            }
            // A handler's operation need not use every parameter.
            for &p in &params {
                if !uses_var(f, p) {
                    w.stmt(&format!("(void){}", var(p)));
                }
            }
            w.append(cx.w);
        });
    }

    fn seq(&mut self, cx: &mut Cx<'_>, id: SeqId, y: Option<&Yield>) {
        let f = cx.f;
        let seq = &f.seqs[id];
        for &s in &seq.stmts {
            self.stmt(cx, s);
        }
        self.term(cx, &seq.term, y);
    }

    fn operand(&mut self, o: &Operand) -> Option<String> {
        match o {
            Operand::Var(v) => Some(var(*v)),
            Operand::Const(c) => self.constant(c),
        }
    }

    fn constant(&mut self, c: &Const) -> Option<String> {
        Some(match c {
            Const::Int(n) => names::int32(*n),
            Const::Int64(n) => names::int64(*n),
            Const::Float(x) => names::float(*x),
            Const::Bool(b) => if *b { "true" } else { "false" }.into(),
            Const::String(s) => names::string_literal(s.as_bytes()),
            Const::Unit => return None,
        })
    }

    /// The operand as C (a constant or a variable with a value).
    fn arg(&mut self, cx: &Cx<'_>, o: &Operand) -> Option<String> {
        match o {
            Operand::Var(v) if !self.has_value(cx.f, *v) => None,
            _ => self.operand(o),
        }
    }

    fn term(&mut self, cx: &mut Cx<'_>, term: &Term, y: Option<&Yield>) {
        match term {
            Term::Yield(v) => {
                let Some(y) = y else {
                    self.internal(format!("yield without a target in {}", cx.f.name));
                    return;
                };
                if let (Some(dst), Some(v)) = (y.dst, v)
                    && self.has_value(cx.f, dst)
                    && !cx.unused.contains(&dst)
                {
                    self.set_var(cx, dst, v);
                }
                if !y.falls {
                    cx.goto(&y.end);
                }
            }
            Term::Return(Some(v)) if self.is_string(cx.f.ret) => {
                let v = self.view(cx, v);
                cx.w.stmt(&format!("em_str_set(em_ret, {v})"));
                cx.w.ret(None);
            }
            Term::Return(v) => {
                let v = if no_data(self.m, cx.f.ret) {
                    None
                } else {
                    v.as_ref().and_then(|v| self.operand(v))
                };
                cx.w.ret(v.as_deref());
            }
            Term::Break { label, value } => {
                let Some(target) = cx.labels.get(label).cloned() else {
                    self.internal(format!("break to an unknown label in {}", cx.f.name));
                    return;
                };
                if let (Some(dst), Some(v)) = (target.dst, value)
                    && self.has_value(cx.f, dst)
                    && !cx.unused.contains(&dst)
                {
                    self.set_var(cx, dst, v);
                }
                cx.goto(&target.end);
            }
            Term::Recur(args) => {
                let Some((vars, head)) = cx.loops.last().cloned() else {
                    self.internal(format!("recur outside a loop in {}", cx.f.name));
                    return;
                };
                // Parallel assignment: the new values may read the old ones.
                let values: Vec<Option<String>> = vars
                    .iter()
                    .zip(args)
                    .map(|(&v, a)| {
                        if !self.has_value(cx.f, v) || cx.unused.contains(&v) {
                            None
                        } else if self.is_string(cx.f.vars[v].ty) {
                            Some(self.view(cx, a))
                        } else {
                            self.operand(a)
                        }
                    })
                    .collect();
                cx.w.block_begin();
                for (i, (&v, value)) in vars.iter().zip(&values).enumerate() {
                    if let Some(value) = value {
                        if self.is_string(cx.f.vars[v].ty) {
                            cx.w.decl("EmString", &format!("t{i}"), None);
                            cx.w.stmt(&format!("em_str_set(&t{i}, {value})"));
                        } else {
                            let ty = self.types.ctype(cx.f.vars[v].ty);
                            cx.w.decl(&ty, &format!("t{i}"), Some(value));
                        }
                    }
                }
                for (i, (&v, value)) in vars.iter().zip(&values).enumerate() {
                    if value.is_some() {
                        if self.is_string(cx.f.vars[v].ty) {
                            cx.w.stmt(&format!("em_str_set(&{}, em_view(&t{i}))", var(v)));
                        } else {
                            cx.w.assign(&var(v), &format!("t{i}"));
                        }
                    }
                }
                cx.goto(&head);
                cx.w.end();
            }
            Term::Unreachable => cx.w.stmt("EMELA_UNREACHABLE()"),
            Term::Fail(o) => {
                let e = self.failure(cx, o);
                cx.w.assign("*em_fail", &e);
                self.dummy_return(cx);
            }
            Term::Unwind | Term::Defect(_) => self.internal(format!("{term:?} in {}", cx.f.name)),
        }
    }

    fn capture(&mut self, cx: &mut Cx<'_>, body: impl FnOnce(&mut Self, &mut Cx<'_>)) -> Writer {
        let aside = cx.w.aside();
        let saved = std::mem::replace(&mut cx.w, aside);
        body(self, cx);
        std::mem::replace(&mut cx.w, saved)
    }

    fn stmt(&mut self, cx: &mut Cx<'_>, id: StmtId) {
        let f = cx.f;
        match &f.stmts[id] {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                if on_defect.is_some() || on_suspend.is_some() {
                    self.internal(format!("a defect or suspension edge in {}", f.name));
                }
                if dst.is_some_and(|d| cx.streamed.contains(&d)) {
                    return;
                }
                self.value(cx, *dst, value);
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let y = Yield::new(*dst, stmt_label(id, "end"));
                let cond = self.operand(cond).unwrap_or_else(|| "0".into());
                cx.w.if_begin(&cond);
                self.seq(cx, *then, Some(&y));
                cx.w.else_begin();
                self.seq(cx, *else_, Some(&y.falling()));
                cx.w.end();
                cx.label(&y.end);
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let y = Yield::new(*dst, stmt_label(id, "end"));
                let (scrutinee, ty) = match on {
                    SwitchOn::Tag(v) => {
                        (self.types.tag(&var(*v), f.vars[*v].ty), Some(f.vars[*v].ty))
                    }
                    SwitchOn::Lit(v) => (var(*v), None),
                };
                cx.w.switch_begin(&scrutinee);
                for case in cases {
                    let key = match (&case.key, ty) {
                        (CaseKey::Variant(k), Some(ty)) => self.types.tag_constant(ty, *k),
                        (CaseKey::Variant(k), None) => k.to_string(),
                        (CaseKey::Lit(c), _) => self.constant(c).unwrap_or_default(),
                    };
                    cx.w.case_begin(&key);
                    self.seq(cx, case.body, Some(&y));
                    cx.w.end();
                }
                match default {
                    Some(d) => {
                        cx.w.default_begin();
                        self.seq(cx, *d, Some(&y));
                        cx.w.end();
                    }
                    None => cx.w.default_stmt("EMELA_UNREACHABLE()"),
                }
                cx.w.switch_end();
                cx.label(&y.end);
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                for (&v, init) in vars.iter().zip(inits) {
                    if self.has_value(f, v) && !cx.unused.contains(&v) {
                        self.set_var(cx, v, init);
                    }
                }
                let y = Yield::new(*dst, stmt_label(id, "end")).falling();
                let head = stmt_label(id, "head");
                cx.loops.push((vars.clone(), head.clone()));
                let text = self.capture(cx, |e, cx| {
                    cx.w.block_begin();
                    e.seq(cx, *body, Some(&y));
                    cx.w.end();
                });
                cx.loops.pop();
                cx.label(&head);
                cx.w.append(text);
                cx.label(&y.end);
            }
            Stmt::Block { dst, label, body } => {
                let y = Yield::new(*dst, stmt_label(id, "end"));
                cx.labels.insert(*label, y.clone());
                cx.w.block_begin();
                self.seq(cx, *body, Some(&y.falling()));
                cx.w.end();
                cx.label(&y.end);
            }
            Stmt::Protect { .. } => self.internal(format!("protect left in {}", f.name)),
            Stmt::Dup(v) if self.boxed(f, *v) => cx.w.stmt(&format!("em_dup({})", var(*v))),
            Stmt::Drop(v) if self.boxed(f, *v) => {
                let name = self.types.box_name(f.vars[*v].ty);
                cx.w.stmt(&format!("em_drop_{name}({})", var(*v)));
            }
            Stmt::Dup(_) | Stmt::Drop(_) => {}
        }
    }

    fn assign(cx: &mut Cx<'_>, dst: Option<VarId>, expr: &str) {
        match dst {
            Some(d) => cx.w.assign(&var(d), expr),
            None => cx.w.stmt(expr),
        }
    }

    fn value(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, value: &Value) {
        let f = cx.f;
        // A destination without a C value, or that nothing uses, takes nothing; only calls and
        // the like are left.
        let dst = dst.filter(|&d| self.has_value(f, d) && !cx.unused.contains(&d));
        match value {
            Value::Const(c) => {
                if let Some(d) = dst {
                    self.set_var(cx, d, &Operand::Const(c.clone()));
                }
            }
            Value::Prim {
                op,
                ty: PrimTy::String,
                args,
            } => {
                let a = self.view(cx, &args[0]);
                let b = self.view(cx, &args[1]);
                Self::assign(
                    cx,
                    dst,
                    &format!("(em_str_cmp({a}, {b}) {} 0)", op.symbol()),
                );
            }
            Value::Prim { op, ty, args } => {
                let a: Vec<String> = args
                    .iter()
                    .map(|o| self.operand(o).unwrap_or_default())
                    .collect();
                Self::assign(cx, dst, &prim(*op, *ty, &a));
            }
            Value::Call {
                callee,
                args,
                on_fail,
                ..
            } => {
                self.call(cx, dst, callee, args);
                if let Some(OnFail::Handle { error, body }) = on_fail {
                    // The call wrote its failure (if any) to `em_f`.
                    cx.w.if_begin("em_f.tag != 0");
                    cx.w.assign(&var(*error), "em_f");
                    self.seq(cx, *body, None);
                    cx.w.end();
                }
            }
            Value::IsError { error, enum_id } => {
                let tag = self.types.failure_tag(*enum_id);
                Self::assign(cx, dst, &format!("({}.tag == {tag})", var(*error)));
            }
            Value::AsError { error, enum_id } => {
                if let Some(d) = dst {
                    let tag = self.types.failure_tag(*enum_id);
                    let e = format!("{}.u.e{tag}", var(*error));
                    self.set_from(cx, d, &e);
                }
            }
            Value::Builtin {
                op: op @ (Builtin::CellNew | Builtin::CellGet | Builtin::CellSet),
                args,
            } => self.cell(cx, dst, *op, args),
            Value::Builtin { op, args } => {
                if self.text_builtin(cx, dst, *op, args) {
                    return;
                }
                let a: Vec<String> = args
                    .iter()
                    .map(|o| self.operand(o).unwrap_or_default())
                    .collect();
                match builtin(*op, &a) {
                    Some(e) => Self::assign(cx, dst, &e),
                    None => self.internal(format!("builtin {:?} in {}", op, f.name)),
                }
            }
            Value::Make { ctor, args } if dst.is_some_and(|d| self.boxed(f, d)) => {
                let d = dst.unwrap_or_else(|| unreachable!());
                self.make_box(cx, d, ctor.variant, args);
            }
            Value::Tuple(args) if dst.is_some_and(|d| self.boxed(f, d)) => {
                let d = dst.unwrap_or_else(|| unreachable!());
                self.make_box(cx, d, 0, args);
            }
            Value::List { elems, tail } => {
                if let Some(d) = dst {
                    self.make_list(cx, d, elems, tail.as_ref());
                }
            }
            Value::ListIsEmpty(l) => Self::assign(cx, dst, &format!("({} == NULL)", var(*l))),
            Value::ListHead(l) | Value::ListTail(l) => {
                if let Some(d) = dst {
                    let member = if matches!(value, Value::ListHead(_)) {
                        "head"
                    } else {
                        "tail"
                    };
                    self.set_from(cx, d, &format!("{}->{member}", var(*l)));
                }
            }
            Value::Field { base, ctor, index } if self.boxed(f, *base) => {
                if let Some(d) = dst {
                    let e =
                        self.types
                            .box_field(&var(*base), f.vars[*base].ty, ctor.variant, *index);
                    self.set_from(cx, d, &e);
                }
            }
            Value::TupleGet { base, index } if self.boxed(f, *base) => {
                if let Some(d) = dst {
                    let e = self
                        .types
                        .box_field(&var(*base), f.vars[*base].ty, 0, *index);
                    self.set_from(cx, d, &e);
                }
            }
            Value::Make { ctor, args } => {
                let Some(d) = dst else { return };
                let a: Vec<Option<String>> = args.iter().map(|o| self.field_value(cx, o)).collect();
                let e = self.types.make(f.vars[d].ty, ctor.variant, &a);
                cx.w.assign(&var(d), &e);
            }
            Value::Field { base, ctor, index } => {
                let Some(d) = dst else { return };
                let e = self
                    .types
                    .field(&var(*base), f.vars[*base].ty, ctor.variant, *index);
                self.set_from(cx, d, &e);
            }
            Value::Tag(v) => {
                let e = self.types.tag(&var(*v), f.vars[*v].ty);
                Self::assign(cx, dst, &e);
            }
            Value::Tuple(args) => {
                let Some(d) = dst else { return };
                let a: Vec<Option<String>> = args.iter().map(|o| self.field_value(cx, o)).collect();
                let e = self.types.make(f.vars[d].ty, 0, &a);
                cx.w.assign(&var(d), &e);
            }
            Value::TupleGet { base, index } => {
                let Some(d) = dst else { return };
                let e = self.types.field(&var(*base), f.vars[*base].ty, 0, *index);
                self.set_from(cx, d, &e);
            }
            Value::Coerce(v) => {
                if let Some(d) = dst {
                    self.set_var(cx, d, &Operand::Var(*v));
                }
            }
            Value::Runtime {
                op: RuntimeOp::ShowString,
                args,
            } => {
                if let Some(d) = dst {
                    let v = self.view(cx, &args[0]);
                    cx.w.stmt(&format!("em_str_show(&{}, {v})", var(d)));
                }
            }
            Value::Runtime {
                op: RuntimeOp::DebugPrint,
                args,
            } => {
                let v = self.view(cx, &args[0]);
                cx.w.stmt(&format!("em_debug({v})"));
            }
            Value::Runtime {
                op: RuntimeOp::Defect,
                args,
            } if matches!(args.first(), Some(Operand::Var(_))) => {
                let v = self.view(cx, &args[0]);
                cx.w.block_begin();
                cx.w.decl("EmStr", "message", Some(&v));
                cx.w.stmt("emela_board_defect(EMELA_DEFECT_PANIC, message.ptr, message.len)");
                cx.w.end();
            }
            Value::Runtime {
                op: RuntimeOp::Defect,
                args,
            } => {
                let (kind, message) = match args.first() {
                    Some(Operand::Const(Const::String(s))) => (defect_kind(s), s.clone()),
                    _ => ("EMELA_DEFECT_PANIC", String::new()),
                };
                let len = message.len().min(u16::MAX as usize);
                cx.w.stmt(&call(
                    "emela_board_defect",
                    [
                        kind.to_owned(),
                        names::string_literal(message.as_bytes()),
                        len.to_string(),
                    ],
                ));
            }
            other => self.internal(format!("{other:?} in {}", f.name)),
        }
    }

    /// `em_drain`: frees the boxes waiting to be freed, and the boxes they hold whose counts reach
    /// zero, in a loop (no recursion, however deep a list or tree is). `box_types[k]` is type index k.
    fn drain_function(&mut self, box_types: &[TypeId]) -> String {
        let index = |t: TypeId| box_types.iter().position(|&b| b == t);
        let release = |s: &Self, field: &str, t: TypeId| -> Option<String> {
            if s.m.repr(t) != Repr::Ref {
                return None;
            }
            let k = index(t)?;
            Some(format!("if (em_release({field})) em_defer({field}, {k});"))
        };
        let mut w = Writer::new();
        w.line_raw("static void em_drain(void) {");
        w.line_raw("  while (em_pending != 0) {");
        w.line_raw("    uint32_t e = em_pending;");
        w.line_raw("    void *b = em_heap + (size_t)((e & EM_PENDING_OFFSET) - 1) * 8;");
        w.line_raw("    em_pending = *(uint32_t *)b;");
        w.line_raw("    switch (e >> EM_PENDING_TYPE_SHIFT) {");
        for (k, &ty) in box_types.iter().enumerate() {
            let name = self.types.box_name(ty);
            w.line_raw(&format!("    case {k}: {{"));
            w.line_raw(&format!("      {name} *p = ({name} *)b;"));
            match self.types.boxes.get(&ty).cloned() {
                Some(Boxed::List(elem)) => {
                    if let Some(r) = release(self, "p->head", elem) {
                        w.line_raw(&format!("      {r}"));
                    }
                    if let Some(r) = release(self, "p->tail", ty) {
                        w.line_raw(&format!("      {r}"));
                    }
                }
                _ => {
                    let members = self.types.box_members(ty);
                    if self.types.box_tagged(ty) {
                        w.line_raw("      switch (p->tag) {");
                        for (v, ms) in members.iter().enumerate() {
                            let releases: Vec<String> = ms
                                .iter()
                                .filter_map(|(i, _, t)| {
                                    release(self, &self.types.box_field("p", ty, v, *i), *t)
                                })
                                .collect();
                            if !releases.is_empty() {
                                w.line_raw(&format!(
                                    "      case {v}: {} break;",
                                    releases.join(" ")
                                ));
                            }
                        }
                        w.line_raw("      default: break;");
                        w.line_raw("      }");
                    } else {
                        for (i, _, t) in &members[0] {
                            if let Some(r) =
                                release(self, &self.types.box_field("p", ty, 0, *i), *t)
                            {
                                w.line_raw(&format!("      {r}"));
                            }
                        }
                    }
                }
            }
            w.line_raw("      em_free(p, sizeof *p);");
            w.line_raw("      break;");
            w.line_raw("    }");
        }
        w.line_raw("    default: break;");
        w.line_raw("    }");
        w.line_raw("  }");
        w.line_raw("}");
        w.finish()
    }

    /// Whether the variable is a heap value (a pointer to a box).
    fn boxed(&self, f: &Function, v: VarId) -> bool {
        f.vars[v].repr == Repr::Ref && self.types.boxes.contains_key(&f.vars[v].ty)
    }

    /// A box of constructor `variant` of `dst`'s type with the fields `args`: a new box, or the
    /// static one of a constructor without fields.
    fn make_box(&mut self, cx: &mut Cx<'_>, dst: VarId, variant: usize, args: &[Operand]) {
        let ty = cx.f.vars[dst].ty;
        let members = self.types.box_members(ty)[variant].clone();
        let d = var(dst);
        if members.is_empty() {
            let s = self.types.static_box(ty, variant);
            cx.w.assign(&d, &s);
            return;
        }
        let name = self.types.box_name(ty);
        cx.w.assign(&d, &format!("({name} *)em_alloc(sizeof({name}))"));
        cx.w.assign(&format!("{d}->rc"), "1");
        if self.types.box_tagged(ty) {
            cx.w.assign(&format!("{d}->tag"), &variant.to_string());
        }
        for (i, _, t) in members {
            let field = self.types.box_field(&d, ty, variant, i);
            self.set_field(cx, &field, t, &args[i]);
        }
    }

    /// `field = o` for a field of type `t` (a String is copied as `em_str_set`).
    fn set_field(&mut self, cx: &mut Cx<'_>, field: &str, t: TypeId, o: &Operand) {
        if self.is_string(t) {
            let v = self.view(cx, o);
            cx.w.stmt(&format!("em_str_set(&{field}, {v})"));
        } else if let Some(v) = self.field_value(cx, o) {
            cx.w.assign(field, &v);
        }
    }

    /// `[a, b, ..tail]`: cells from the last element on.
    fn make_list(
        &mut self,
        cx: &mut Cx<'_>,
        dst: VarId,
        elems: &[Operand],
        tail: Option<&Operand>,
    ) {
        let ty = cx.f.vars[dst].ty;
        let name = self.types.box_name(ty);
        let elem = match self.types.boxes.get(&ty) {
            Some(Boxed::List(t)) => *t,
            _ => {
                self.internal(format!("a list without a box in {}", cx.f.name));
                return;
            }
        };
        let rest = tail
            .and_then(|t| self.operand(t))
            .unwrap_or_else(|| "NULL".into());
        cx.w.block_begin();
        cx.w.decl(&format!("{name} *"), "list", Some(&rest));
        for e in elems.iter().rev() {
            cx.w.block_begin();
            cx.w.decl(
                &format!("{name} *"),
                "cell",
                Some(&format!("({name} *)em_alloc(sizeof({name}))")),
            );
            cx.w.assign("cell->rc", "1");
            if !self.types.is_void(elem) {
                self.set_field(cx, "cell->head", elem, e);
            }
            cx.w.assign("cell->tail", "list");
            cx.w.assign("list", "cell");
            cx.w.end();
        }
        cx.w.assign(&var(dst), "list");
        cx.w.end();
    }

    /// `dst = expr`, where `expr` is an lvalue of `dst`'s type (a String is copied as
    /// `em_str_set`).
    fn set_from(&mut self, cx: &mut Cx<'_>, dst: VarId, expr: &str) {
        if self.is_string(cx.f.vars[dst].ty) {
            self.strings = true;
            cx.w.stmt(&format!("em_str_set(&{}, em_view(&{expr}))", var(dst)));
        } else {
            cx.w.assign(&var(dst), expr);
        }
    }

    /// A call of a board operation that may fail or takes or gives a `List[Int]` (see
    /// [`Types::op_signature`]): lists go through arrays of `EMELA_TRANSFER_CAPACITY` elements,
    /// and a nonzero code is the failure `PeripheralError(code)`. A list too long or with an
    /// element out of 0..=65535 fails with code -1 without calling the board.
    fn board_call(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, g: FnId, args: &[Operand]) {
        let target = &self.m.functions[g];
        let name = self.fn_names[ix(g)].clone();
        let fails = target.fails;
        let param_types: Vec<TypeId> = target.params.iter().map(|&p| target.vars[p].ty).collect();
        let ret = target.ret;
        cx.w.block_begin();
        let mut call_args: Vec<String> = Vec::new();
        let mut checks: Vec<String> = Vec::new();
        for (k, (o, &t)) in args.iter().zip(&param_types).enumerate() {
            if self.types.is_int_list(t) {
                let list = self.operand(o).unwrap_or_else(|| "NULL".into());
                let box_name = self.types.box_name(t);
                let (buf, len, ok) = (
                    format!("data{k}"),
                    format!("data{k}_length"),
                    format!("data{k}_ok"),
                );
                cx.w.decl("uint16_t", &format!("{buf}[EMELA_TRANSFER_CAPACITY]"), None);
                cx.w.decl("uint16_t", &len, Some("0"));
                cx.w.decl("bool", &ok, Some("true"));
                cx.w.line_raw(&format!(
                    "for (const {box_name} *c = {list}; c != NULL; c = c->tail) {{"
                ));
                cx.w.line_raw(&format!(
                    "  if ({len} == EMELA_TRANSFER_CAPACITY || c->head < 0 || c->head > 65535) {{ {ok} = false; break; }}"
                ));
                cx.w.line_raw(&format!("  {buf}[{len}++] = (uint16_t)c->head;"));
                cx.w.line_raw("}");
                call_args.push(buf);
                call_args.push(len);
                checks.push(ok);
            } else if let Some(v) = self.arg(cx, o) {
                call_args.push(v);
            }
        }
        let list_result = self.types.is_int_list(ret);
        if list_result {
            cx.w.decl("uint16_t", "out[EMELA_TRANSFER_CAPACITY]", None);
            cx.w.decl("uint16_t", "out_length", Some("0"));
            call_args.extend([
                "out".into(),
                "EMELA_TRANSFER_CAPACITY".into(),
                "&out_length".into(),
            ]);
        } else if fails && !self.types.is_void(ret) {
            let ty = self.types.ctype(ret);
            let init = self.types.initial(ret).unwrap_or("{0}");
            cx.w.decl(&ty, "result", Some(init));
            call_args.push("&result".into());
        }
        let c = call(&name, call_args);
        // The result, into `dst`.
        let finish = |e: &mut Self, cx: &mut Cx<'_>| {
            let Some(d) = dst else { return };
            if list_result {
                let box_name = e.types.box_name(ret);
                cx.w.decl(&format!("{box_name} *"), "list", Some("NULL"));
                cx.w.line_raw("for (uint16_t i = out_length; i-- > 0;) {");
                cx.w.line_raw(&format!(
                    "  {box_name} *cell = ({box_name} *)em_alloc(sizeof({box_name}));"
                ));
                cx.w.line_raw("  cell->rc = 1;");
                cx.w.line_raw("  cell->head = (int32_t)out[i];");
                cx.w.line_raw("  cell->tail = list;");
                cx.w.line_raw("  list = cell;");
                cx.w.line_raw("}");
                cx.w.assign(&var(d), "list");
            } else if fails {
                cx.w.assign(&var(d), "result");
            }
        };
        if fails {
            let e = self.types.define_failure();
            if !cx.slots.iter().any(|(_, n)| n == "em_f") {
                cx.slots.push((e.clone(), "em_f".into()));
            }
            let code = if checks.is_empty() {
                c
            } else {
                format!("({}) ? {c} : -1", checks.join(" && "))
            };
            cx.w.decl("int32_t", "code", Some(&code));
            cx.w.assign("em_f.tag", "0");
            let tag = self
                .types
                .errors
                .iter()
                .position(|(id, _)| {
                    self.m.core.enums[*id].name == "Std.Embedded.Peripheral.PeripheralError"
                })
                .map_or(0, |i| i + 1);
            cx.w.if_begin("code != 0");
            cx.w.assign("em_f", &format!("({e}){{.tag = {tag}, .u.e{tag} = code}}"));
            cx.w.else_begin();
            finish(self, cx);
            cx.w.end();
        } else {
            if list_result {
                cx.w.stmt(&c);
            } else {
                Self::assign(cx, dst, &c);
            }
            finish(self, cx);
        }
        cx.w.end();
    }

    /// Writes the pieces of a string to a Console hook: a streamed string's parts in order, any
    /// other string as it is.
    fn pieces(&mut self, cx: &mut Cx<'_>, hook: &str, o: &Operand) {
        if let Operand::Var(v) = o
            && cx.streamed.contains(v)
            && let Some(value) = definition(cx.f, *v)
        {
            match value {
                Value::Builtin {
                    op: Builtin::StringAppend,
                    args,
                } => {
                    self.pieces(cx, hook, &args[0]);
                    self.pieces(cx, hook, &args[1]);
                }
                Value::Builtin { op, args } => {
                    // Int.to_string / Int64.to_string: a number's digits.
                    let n = self.operand(&args[0]).unwrap_or_default();
                    let cast = if *op == Builtin::IntToString {
                        "(int64_t)"
                    } else {
                        ""
                    };
                    self.strings = true;
                    cx.w.stmt(&format!("em_write_int64({hook}, {cast}{n})"));
                }
                _ => {}
            }
            return;
        }
        let v = self.view(cx, o);
        cx.w.block_begin();
        cx.w.decl("EmStr", "piece", Some(&v));
        cx.w.stmt(&format!("{hook}(piece.ptr, piece.len)"));
        cx.w.end();
    }

    /// The builtins on strings, and those giving an Option. Whether `op` is one.
    fn text_builtin(
        &mut self,
        cx: &mut Cx<'_>,
        dst: Option<VarId>,
        op: Builtin,
        args: &[Operand],
    ) -> bool {
        let d = dst.map(var);
        let out = |cx: &mut Cx<'_>, e: String| {
            if d.is_some() {
                cx.w.stmt(&e);
            }
        };
        match op {
            Builtin::StringByteSize => {
                let v = self.view(cx, &args[0]);
                Self::assign(cx, dst, &format!("(int32_t){v}.len"));
            }
            Builtin::StringContains | Builtin::StringStartsWith | Builtin::StringEndsWith => {
                let name = match op {
                    Builtin::StringContains => "em_str_contains",
                    Builtin::StringStartsWith => "em_str_starts_with",
                    _ => "em_str_ends_with",
                };
                let a = self.view(cx, &args[0]);
                let b = self.view(cx, &args[1]);
                Self::assign(cx, dst, &format!("{name}({a}, {b})"));
            }
            Builtin::StringAppend => {
                let a = self.view(cx, &args[0]);
                let b = self.view(cx, &args[1]);
                let e = format!(
                    "em_str_append(&{}, {a}, {b})",
                    d.clone().unwrap_or_default()
                );
                out(cx, e);
            }
            Builtin::StringTrim => {
                let a = self.view(cx, &args[0]);
                let e = format!("em_str_trim(&{}, {a})", d.clone().unwrap_or_default());
                out(cx, e);
            }
            Builtin::StringLength => {
                let Some(dv) = dst else { return true };
                let s = self.view(cx, &args[0]);
                cx.w.block_begin();
                cx.w.decl("EmStr", "text", Some(&s));
                cx.w.decl("int32_t", "n", Some("0"));
                cx.w.line_raw(
                    r#"/* Grapheme clusters (spec 16.2), as `String.split` with an empty separator. */
for (size_t i = 0; i < text.len; i++, n++) {
  if ((unsigned char)text.ptr[i] >= 0x80) {
    emela_board_defect(EMELA_DEFECT_PANIC, "String.length needs ASCII text", 30);
  }
  if (text.ptr[i] == '\r' && i + 1 < text.len && text.ptr[i + 1] == '\n') i++;
}"#,
                );
                cx.w.assign(&var(dv), "n");
                cx.w.end();
            }
            Builtin::StringSplit | Builtin::StringChars => {
                let Some(dv) = dst else { return true };
                let list = self.types.box_name(cx.f.vars[dv].ty);
                let s = self.view(cx, &args[0]);
                let (sep, defect) = match op {
                    Builtin::StringChars => (
                        "em_lit(\"\", 0)".to_owned(),
                        "\"String.chars needs ASCII text\", 29",
                    ),
                    _ => (
                        self.view(cx, &args[1]),
                        "\"String.split with an empty separator needs ASCII text\", 53",
                    ),
                };
                cx.w.block_begin();
                cx.w.decl("EmStr", "text", Some(&s));
                cx.w.decl("EmStr", "sep", Some(&sep));
                cx.w.decl(&format!("{list} *"), "head", Some("NULL"));
                cx.w.decl(&format!("{list} **"), "tail", Some("&head"));
                cx.w.line_raw(&format!(
                    "#define EM_PUSH(p, n) do {{ {list} *c = ({list} *)em_alloc(sizeof({list})); c->rc = 1; \\\n  em_str_bytes(&c->head, (p), (n)); c->tail = NULL; *tail = c; tail = &c->tail; }} while (0)"
                ));
                cx.w.line_raw(&format!(
                    r#"if (sep.len == 0) {{
  /* Grapheme clusters (spec 16.2): without the Unicode tables only ASCII text, where every
   * character is a cluster except CR LF (as the host C target). */
  for (size_t i = 0; i < text.len; i++) {{
    if ((unsigned char)text.ptr[i] >= 0x80) {{
      emela_board_defect(EMELA_DEFECT_PANIC, {defect});
    }}
  }}
  for (size_t i = 0; i < text.len;) {{
    size_t n = text.ptr[i] == '\r' && i + 1 < text.len && text.ptr[i + 1] == '\n' ? 2 : 1;
    EM_PUSH(text.ptr + i, n);
    i += n;
  }}
}} else {{
  size_t start = 0;
  for (size_t i = 0; i + sep.len <= text.len;) {{
    if (memcmp(text.ptr + i, sep.ptr, sep.len) == 0) {{
      EM_PUSH(text.ptr + start, i - start);
      i += sep.len;
      start = i;
    }} else {{
      i++;
    }}
  }}
  EM_PUSH(text.ptr + start, text.len - start);
}}
#undef EM_PUSH"#
                ));
                cx.w.assign(&var(dv), "head");
                cx.w.end();
            }
            Builtin::StringJoin => {
                let Some(dv) = dst else { return true };
                let Operand::Var(parts) = &args[0] else {
                    return true;
                };
                let list = self.types.box_name(cx.f.vars[*parts].ty);
                let sep = self.view(cx, &args[1]);
                let d = var(dv);
                cx.w.block_begin();
                cx.w.decl("EmStr", "sep", Some(&sep));
                cx.w.stmt(&format!("em_str_start(&{d})"));
                cx.w.line_raw(&format!(
                    "for (const {list} *c = {}; c != NULL; c = c->tail) {{",
                    var(*parts)
                ));
                cx.w.line_raw(&format!(
                    "  if (c != {}) em_str_push(&{d}, sep);",
                    var(*parts)
                ));
                cx.w.line_raw(&format!("  em_str_push(&{d}, em_view(&c->head));"));
                cx.w.line_raw("}");
                cx.w.end();
            }
            Builtin::StringCodePoints => {
                let Some(dv) = dst else { return true };
                let list = self.types.box_name(cx.f.vars[dv].ty);
                let s = self.view(cx, &args[0]);
                cx.w.block_begin();
                cx.w.decl("EmStr", "text", Some(&s));
                cx.w.decl(&format!("{list} *"), "head", Some("NULL"));
                cx.w.decl(&format!("{list} **"), "tail", Some("&head"));
                cx.w.line_raw(&format!(
                    r#"for (size_t i = 0; i < text.len;) {{
  uint32_t cp;
  i += em_utf8_next((const unsigned char *)text.ptr, i, &cp);
  {list} *c = ({list} *)em_alloc(sizeof({list}));
  c->rc = 1;
  c->head = (int32_t)cp;
  c->tail = NULL;
  *tail = c;
  tail = &c->tail;
}}"#
                ));
                cx.w.assign(&var(dv), "head");
                cx.w.end();
            }
            Builtin::StringFromCodePoint => {
                let Some(dv) = dst else { return true };
                self.strings = true;
                let ty = cx.f.vars[dv].ty;
                let (some, none) = (self.variant(ty, "Some"), self.variant(ty, "None"));
                let n = self.operand(&args[0]).unwrap_or_default();
                let found =
                    self.types
                        .make(ty, some, &[Some("em_string_of(em_view(&text))".into())]);
                let missing = self.types.make(ty, none, &[]);
                cx.w.block_begin();
                cx.w.decl("int32_t", "cp", Some(&n));
                cx.w.if_begin("cp < 0 || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)");
                cx.w.assign(&var(dv), &missing);
                cx.w.else_begin();
                cx.w.decl("char", "bytes[4]", None);
                cx.w.decl("EmString", "text", None);
                cx.w.stmt("em_str_bytes(&text, bytes, em_utf8_encode((uint32_t)cp, bytes))");
                cx.w.assign(&var(dv), &found);
                cx.w.end();
                cx.w.end();
            }
            Builtin::IntToString | Builtin::Int64ToString | Builtin::FloatToString => {
                self.strings = true;
                let name = match op {
                    Builtin::IntToString => "em_str_int",
                    Builtin::Int64ToString => "em_str_int64",
                    _ => {
                        self.float_strings = true;
                        "em_str_float"
                    }
                };
                let n = self.operand(&args[0]).unwrap_or_default();
                let e = format!("{name}(&{}, {n})", d.clone().unwrap_or_default());
                out(cx, e);
            }
            Builtin::IntParse | Builtin::FloatParse | Builtin::IntCheckedAdd => {
                let Some(dv) = dst else { return true };
                let ty = cx.f.vars[dv].ty;
                let (some, none) = (self.variant(ty, "Some"), self.variant(ty, "None"));
                let (cty, test) = match op {
                    Builtin::IntParse => {
                        let v = self.view(cx, &args[0]);
                        ("int32_t", format!("em_str_parse_int({v}, &r)"))
                    }
                    Builtin::FloatParse => {
                        self.float_strings = true;
                        let v = self.view(cx, &args[0]);
                        ("double", format!("em_str_parse_float({v}, &r)"))
                    }
                    _ => {
                        let a = self.operand(&args[0]).unwrap_or_default();
                        let b = self.operand(&args[1]).unwrap_or_default();
                        ("int32_t", format!("!__builtin_add_overflow({a}, {b}, &r)"))
                    }
                };
                let found = self.types.make(ty, some, &[Some("r".into())]);
                let missing = self.types.make(ty, none, &[]);
                cx.w.block_begin();
                cx.w.decl(cty, "r", Some("0"));
                cx.w.if_begin(&test);
                cx.w.assign(&var(dv), &found);
                cx.w.else_begin();
                cx.w.assign(&var(dv), &missing);
                cx.w.end();
                cx.w.end();
            }
            _ => return false,
        }
        true
    }

    /// The index of the constructor named `name` of the enum type `ty`.
    fn variant(&self, ty: TypeId, name: &str) -> usize {
        match self.m.core.ty(ty) {
            Type::Enum(id, _) => self.m.core.enums[*id]
                .variants
                .iter()
                .position(|v| v.name == name)
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// A cell's operation. A new cell is a local (declared at the top of the function) and the
    /// cell is its address.
    fn cell(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, op: Builtin, args: &[Operand]) {
        let a: Vec<Option<String>> = args.iter().map(|o| self.operand(o)).collect();
        // A cell of a heap value is a box `{rc, value}` holding a reference of its own.
        let cell_var = match (op, dst, args.first()) {
            (Builtin::CellNew, Some(d), _) => Some(d),
            (_, _, Some(Operand::Var(c))) => Some(*c),
            _ => None,
        };
        if let Some(c) = cell_var
            && self.boxed(cx.f, c)
        {
            let ty = cx.f.vars[c].ty;
            let Some(Boxed::Cell(content)) = self.types.boxes.get(&ty).cloned() else {
                return;
            };
            let name = self.types.box_name(ty);
            let counted =
                self.types.boxes.contains_key(&content) && self.m.repr(content) == Repr::Ref;
            match op {
                Builtin::CellNew => {
                    let d = var(c);
                    cx.w.assign(&d, &format!("({name} *)em_alloc(sizeof({name}))"));
                    cx.w.assign(&format!("{d}->rc"), "1");
                    if let Some(init) = args.first() {
                        self.set_field(cx, &format!("{d}->value"), content, init);
                        if counted {
                            cx.w.stmt(&format!("em_dup({d}->value)"));
                        }
                    }
                }
                Builtin::CellGet => {
                    if let Some(d) = dst {
                        self.set_from(cx, d, &format!("{}->value", var(c)));
                        if counted {
                            cx.w.stmt(&format!("em_dup({})", var(d)));
                        }
                    }
                }
                _ => {
                    if let Some(v) = args.get(1) {
                        let field = format!("{}->value", var(c));
                        if counted {
                            let drop = format!("em_drop_{}", self.types.box_name(content));
                            if let Some(x) = self.operand(v) {
                                cx.w.stmt(&format!("em_dup({x})"));
                            }
                            cx.w.stmt(&format!("{drop}({field})"));
                        }
                        self.set_field(cx, &field, content, v);
                    }
                }
            }
            return;
        }
        match op {
            Builtin::CellNew => {
                let Some(d) = dst else { return };
                let Some(Layout::Cell(t)) = self.types.layouts.get(&cx.f.vars[d].ty).cloned()
                else {
                    self.internal(format!("a cell without a layout in {}", cx.f.name));
                    return;
                };
                if self.types.is_void(t) {
                    cx.w.assign(&var(d), "NULL");
                    return;
                }
                let slot = format!("{}_state", var(d));
                let ty = self.types.ctype(t);
                cx.slots.push((ty, slot.clone()));
                if self.is_string(t) {
                    let v = self.view(cx, &args[0]);
                    cx.w.stmt(&format!("em_str_set(&{slot}, {v})"));
                } else if let Some(Some(init)) = a.first() {
                    cx.w.assign(&slot, init);
                }
                cx.w.assign(&var(d), &format!("&{slot}"));
            }
            Builtin::CellGet => {
                if let (Some(d), Some(Some(c))) = (dst, a.first()) {
                    self.set_from(cx, d, &format!("(*{c})"));
                }
            }
            _ => {
                if let (Some(Some(c)), Some(v)) = (a.first(), args.get(1)) {
                    if self.is_string_operand(cx, v) {
                        let v = self.view(cx, v);
                        cx.w.stmt(&format!("em_str_set({c}, {v})"));
                    } else if let Some(v) = self.operand(v) {
                        cx.w.assign(&format!("*{c}"), &v);
                    }
                }
            }
        }
    }

    fn call(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, callee: &Callee, args: &[Operand]) {
        let Callee::Direct(g) = callee else {
            self.internal(format!("an indirect call in {}", cx.f.name));
            return;
        };
        let target = &self.m.functions[*g];
        let external = target.external.is_some();
        if is_board_op(target)
            && (target.fails
                || target
                    .params
                    .iter()
                    .any(|&p| self.types.is_int_list(target.vars[p].ty))
                || self.types.is_int_list(target.ret))
        {
            self.board_call(cx, dst, *g, args);
            return;
        }
        // A Console line: its pieces, then the newline.
        if let Some(&hook) = self.sinks.get(g)
            && let Some(text) = args.iter().find(|o| self.is_string_operand(cx, o))
        {
            cx.w.block_begin();
            self.pieces(cx, hook, text);
            cx.w.stmt(&format!("{hook}(\"\\n\", 1)"));
            cx.w.end();
            return;
        }
        let mut a: Vec<String> = Vec::new();
        for o in args {
            if self.is_string_operand(cx, o) {
                let v = self.view(cx, o);
                if external {
                    // `const char *` and the length.
                    a.push(format!("{v}.ptr"));
                    a.push(format!("{v}.len"));
                } else {
                    a.push(v);
                }
            } else if let Some(v) = self.arg(cx, o) {
                a.push(v);
            }
        }
        let name = self.fn_names[ix(*g)].clone();
        if target.fails {
            let e = self.types.define_failure();
            if !cx.slots.iter().any(|(_, n)| n == "em_f") {
                cx.slots.push((e, "em_f".into()));
            }
            cx.w.assign("em_f.tag", "0");
        }
        let fail_arg = target.fails.then(|| "&em_f".to_owned());
        if self.is_string(target.ret) {
            let out = match dst {
                Some(d) => var(d),
                None => {
                    if !cx.slots.iter().any(|(_, n)| n == "em_discard") {
                        cx.slots.push(("EmString".into(), "em_discard".into()));
                    }
                    "em_discard".into()
                }
            };
            if external {
                a.push(format!("{out}.buf"));
                a.push("EMELA_STRING_CAPACITY".into());
                cx.w.block_begin();
                cx.w.decl("uint16_t", "n", Some(&call(&name, a)));
                cx.w.stmt("em_str_capacity(n)");
                cx.w.assign(&format!("{out}.len"), "n");
                cx.w.assign(&format!("{out}.lit"), "NULL");
                cx.w.assign(&format!("{out}.buf[n]"), "'\\0'");
                cx.w.end();
            } else {
                a.push(format!("&{out}"));
                a.extend(fail_arg);
                cx.w.stmt(&call(&name, a));
            }
            return;
        }
        a.extend(fail_arg);
        Self::assign(cx, dst, &call(&name, a));
    }
}

fn prim(op: PrimOp, ty: PrimTy, a: &[String]) -> String {
    let bin = |sym: &str| format!("({} {sym} {})", a[0], a[1]);
    match ty {
        PrimTy::I32 | PrimTy::I64 => {
            let p = if ty == PrimTy::I32 { "em_i" } else { "em_l" };
            match op {
                PrimOp::Add => format!("{p}add({}, {})", a[0], a[1]),
                PrimOp::Sub => format!("{p}sub({}, {})", a[0], a[1]),
                PrimOp::Mul => format!("{p}mul({}, {})", a[0], a[1]),
                PrimOp::Div => format!("{p}div({}, {})", a[0], a[1]),
                PrimOp::Rem => format!("{p}rem({}, {})", a[0], a[1]),
                PrimOp::Neg => format!("{p}neg({})", a[0]),
                PrimOp::Not => format!("(!{})", a[0]),
                _ => bin(op.symbol()),
            }
        }
        PrimTy::F64 => match op {
            PrimOp::Rem => format!("fmod({}, {})", a[0], a[1]),
            PrimOp::Neg => format!("(-{})", a[0]),
            PrimOp::Not => format!("(!{})", a[0]),
            _ => bin(op.symbol()),
        },
        // Rejected by the checker (a String is a heap value).
        PrimTy::String => "0".into(),
    }
}

fn builtin(op: Builtin, a: &[String]) -> Option<String> {
    let named = |n: &str| call(n, a.iter());
    Some(match op {
        Builtin::IntBitAnd | Builtin::Int64BitAnd => format!("({} & {})", a[0], a[1]),
        Builtin::IntBitOr | Builtin::Int64BitOr => format!("({} | {})", a[0], a[1]),
        Builtin::IntBitXor | Builtin::Int64BitXor => format!("({} ^ {})", a[0], a[1]),
        Builtin::IntBitNot | Builtin::Int64BitNot => format!("(~{})", a[0]),
        Builtin::IntShiftLeft => named("em_ishl"),
        Builtin::IntShiftRight => named("em_ishr"),
        Builtin::IntShiftRightUnsigned => named("em_iushr"),
        Builtin::Int64ShiftLeft => named("em_lshl"),
        Builtin::Int64ShiftRight => named("em_lshr"),
        Builtin::Int64ShiftRightUnsigned => named("em_lushr"),
        Builtin::Int64FromInt => format!("(int64_t){}", a[0]),
        Builtin::Int64ToInt => format!("(int32_t)(uint32_t)(uint64_t){}", a[0]),
        Builtin::FloatFromInt => format!("(double){}", a[0]),
        Builtin::FloatFloor => format!("em_float_to_int(floor({}))", a[0]),
        Builtin::FloatCeil => format!("em_float_to_int(ceil({}))", a[0]),
        // 0.5 rounds away from zero (spec 16.1).
        Builtin::FloatRound => format!("em_float_to_int(round({}))", a[0]),
        Builtin::FloatTruncate => format!("em_float_to_int(trunc({}))", a[0]),
        _ => return None,
    })
}
