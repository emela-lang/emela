//! JavaScript backend (Tier 2).
//!
//! Emits a `"use strict"` module of plain functions, relying on JavaScript's
//! lexical scoping for closures, and logs `main()`'s result unless it is Unit.
//!
//! Platform functions (spec 0013) are resolved by a bundled default runtime
//! object `__rt`; only the platform functions the program uses are emitted.

use emela_codegen::{
    Artifact, ArtifactKind, Backend, BackendError, BackendOptions, BinaryOp, IrArm, IrExpr,
    IrPattern, IrProgram, QuestionMode, Result, Tier, Type, contains_tail_self_call, is_intrinsic,
    used_intrinsics, used_platform_fns,
};

/// The Node.js-flavored JavaScript backend.
pub struct JsBackend;

impl Backend for JsBackend {
    fn name(&self) -> &str {
        "js-node"
    }

    fn tier(&self) -> Tier {
        Tier::Tier2
    }

    fn compile(&self, ir: &IrProgram, _options: &BackendOptions) -> Result<Artifact> {
        let used = used_platform_fns(ir);
        for name in &used {
            if runtime_impl(name).is_none() {
                return Err(BackendError::new(format!(
                    "backend `js-node` does not provide platform function `{name}`"
                )));
            }
        }
        // Intrinsic coverage (spec 0021): reject a program that uses an intrinsic
        // this backend does not inline. The js backend inlines every intrinsic in
        // the normative interface, so coverage is exactly `is_intrinsic`.
        for name in used_intrinsics(ir) {
            if !is_intrinsic(&name) {
                return Err(BackendError::new(format!(
                    "backend `js-node` does not provide intrinsic `{name}`"
                )));
            }
        }
        Ok(Artifact::text(ArtifactKind::JsSource, emit(ir, &used)))
    }
}

/// The JS expression an intrinsic (spec 0021) inlines to. Assumes the intrinsic
/// is provided (checked in `compile`).
fn intrinsic_js(name: &str, args: &[IrExpr]) -> String {
    let a = emit_expr(&args[0]);
    let b = args.get(1).map(emit_expr).unwrap_or_default();
    let c = args.get(2).map(emit_expr).unwrap_or_default();
    match name {
        "i32_add" | "f64_add" => format!("({a} + {b})"),
        "i32_sub" | "f64_sub" => format!("({a} - {b})"),
        "i32_mul" | "f64_mul" => format!("({a} * {b})"),
        // Integer division truncates toward zero (spec 0016).
        "i32_div_s" => format!("(({a} / {b}) | 0)"),
        "f64_div" => format!("({a} / {b})"),
        "i32_rem_s" => format!("({a} % {b})"),
        "i32_eq" | "f64_eq" => format!("({a} === {b})"),
        "i32_lt_s" | "f64_lt" => format!("({a} < {b})"),
        "f64_sqrt" => format!("Math.sqrt({a})"),
        // String concatenation (spec 0017): the same `+` the old `Concat` node
        // emitted, now reached through the `Concat` trait's impl (spec 0020/0021).
        "string_concat" => format!("({a} + {b})"),
        // `Eq`/`Ord for String`. `===` is exact; `<` is JavaScript's UTF-16
        // lexicographic order, which agrees with the wasm backend's byte order
        // for ASCII/BMP text (they can differ only for supplementary characters).
        "string_eq" => format!("({a} === {b})"),
        "string_lt" => format!("({a} < {b})"),
        // Scalar string operations (spec 0030). JS strings are UTF-16, so use
        // `[...s]` (iterates by code point) and `codePointAt` for scalar units,
        // never `s.length` / `s[i]` which are UTF-16 code-unit based. A `Char`
        // is its code point as a number here (see `IrExpr::Char`).
        "string_length" => format!("([...{a}].length)"),
        "string_char_at" => format!("([...{a}][{b}].codePointAt(0))"),
        "string_slice" => format!("([...{a}].slice({b}, {c}).join(\"\"))"),
        // A `Char` is already its code point number, so `char_code` is identity.
        "char_code" => format!("({a})"),
        // Char/String conversions (spec 0017), formerly the `CharFromCode` /
        // `StringFromChar` nodes. A `Char` is already its code point number, so
        // `char_from_code` is identity; `string_from_char` builds the string.
        "char_from_code" => format!("({a})"),
        "string_from_char" => format!("String.fromCodePoint({a})"),
        // Array operations (spec 0007), formerly the `ArrayLength` / `ArrayGet` /
        // `ArrayPush` nodes. Arrays are native JS arrays. `array_get_unchecked`
        // is the raw accessor (the safe `array_get` is a stdlib wrapper).
        // `array_push` returns a fresh array (pure copy), so spread not mutate.
        "array_length" => format!("({a}.length)"),
        "array_get_unchecked" => format!("{a}[{b}]"),
        "array_push" => format!("[...{a}, {b}]"),
        _ => unreachable!("intrinsic `{name}` not provided by js-node backend"),
    }
}

/// The platform functions this backend provides, with their JS implementations.
fn runtime_impl(name: &str) -> Option<&'static str> {
    match name {
        "io.write_stdout" => Some("(s) => process.stdout.write(s)"),
        "io.write_stderr" => Some("(s) => process.stderr.write(s)"),
        "clock.monotonic_seconds" => Some("() => Math.floor(Date.now() / 1000)"),
        _ => None,
    }
}

fn emit(program: &IrProgram, used_platform: &[String]) -> String {
    let mut out = String::new();
    out.push_str("\"use strict\";\n\n");
    // Error-handling runtime (spec 0011): a thrown error carries its value, a
    // propagated `None` is its own signal, and a panic is distinct so `catch`
    // never swallows it.
    out.push_str("class EmelaError { constructor(value) { this.value = value; } }\n");
    out.push_str("class EmelaNone {}\n");
    out.push_str("class EmelaPanic { constructor(message) { this.message = message; } }\n");
    // Trampoline marker for self-tail-calls (spec 0045): a tail-recursive
    // function returns it to its own driver loop instead of calling itself.
    out.push_str("class EmelaTail { constructor(args) { this.args = args; } }\n\n");
    if !used_platform.is_empty() {
        // Bundled default runtime: the backend supplies the platform bodies.
        out.push_str("const __rt = {\n");
        for name in used_platform {
            if let Some(body) = runtime_impl(name) {
                out.push_str(&format!("  {name:?}: {body},\n"));
            }
        }
        out.push_str("};\n\n");
    }
    for function in &program.functions {
        if !function.effects.effects.is_empty() {
            out.push_str(&format!(
                "// uses {{{}}}\n",
                function.effects.effects.join(", ")
            ));
        }
        out.push_str(&format!(
            "function {}({}) {{\n",
            js_name(&function.name),
            function
                .params
                .iter()
                .map(|param| js_name(&param.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let none_guard = matches!(function.ret, Type::Option(_));
        if contains_tail_self_call(&function.body) {
            // Self-tail-calls (spec 0045) trampoline through `EmelaTail`: the
            // body returns the marker, and this driver loop reassigns the
            // parameters and continues — constant stack depth.
            out.push_str("  while (true) {\n");
            out.push_str("    let __r;\n");
            if none_guard {
                out.push_str("    try { __r = ");
                out.push_str(&emit_expr(&function.body));
                out.push_str("; } catch (__e) { if (__e instanceof EmelaNone) return { tag: 1, values: [] }; throw __e; }\n");
            } else {
                out.push_str("    __r = ");
                out.push_str(&emit_expr(&function.body));
                out.push_str(";\n");
            }
            out.push_str("    if (__r instanceof EmelaTail) {\n");
            for (index, param) in function.params.iter().enumerate() {
                out.push_str(&format!(
                    "      {} = __r.args[{index}];\n",
                    js_name(&param.name)
                ));
            }
            out.push_str("      continue;\n");
            out.push_str("    }\n");
            out.push_str("    return __r;\n");
            out.push_str("  }\n}\n\n");
        } else if none_guard {
            // A function returning Option catches a propagated `None` (`?`).
            out.push_str("  try { return ");
            out.push_str(&emit_expr(&function.body));
            out.push_str("; } catch (__e) { if (__e instanceof EmelaNone) return { tag: 1, values: [] }; throw __e; }\n}\n\n");
        } else {
            out.push_str("  return ");
            out.push_str(&emit_expr(&function.body));
            out.push_str(";\n}\n\n");
        }
    }
    let main_ret = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .map(|function| &function.ret);
    out.push_str("try {\n");
    out.push_str("  const __emela_result = main();\n");
    if !matches!(main_ret, Some(Type::Unit)) {
        out.push_str("  if (__emela_result !== undefined) console.log(__emela_result);\n");
    }
    out.push_str("} catch (__e) {\n");
    out.push_str(
        "  if (__e instanceof EmelaPanic) { console.error(\"panic: \" + __e.message); process.exit(1); }\n",
    );
    out.push_str("  throw __e;\n");
    out.push_str("}\n");
    out
}

fn emit_expr(expr: &IrExpr) -> String {
    match expr {
        IrExpr::Int(value) => value.to_string(),
        IrExpr::Float(value) => value.to_string(),
        IrExpr::Bool(value) => value.to_string(),
        IrExpr::String(value) => format!("{value:?}"),
        // A `Char` is its Unicode scalar value as a number (spec 0017).
        IrExpr::Char(value) => value.to_string(),
        IrExpr::Concat { left, right } => format!("({} + {})", emit_expr(left), emit_expr(right)),
        IrExpr::Array { elems, .. } => format!(
            "[{}]",
            elems.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Unit => "undefined".to_string(),
        // RC ops (spec 0048) are transparent under the host GC (A9). The wasm
        // backend inserts them for itself only, but hand-built IR may carry them.
        IrExpr::Retain { value } => emit_expr(value),
        IrExpr::Release { next, .. } => emit_expr(next),
        IrExpr::Var { name, .. } => js_name(name),
        IrExpr::FunctionRef { name, .. } => js_name(name),
        IrExpr::Let {
            name, value, next, ..
        } => format!(
            "(() => {{ const {} = {}; return {}; }})()",
            js_name(name),
            emit_expr(value),
            emit_expr(next)
        ),
        IrExpr::Call { callee, args, .. } => format!(
            "{}({})",
            emit_expr(callee),
            args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Platform { name, args, .. } => format!(
            "__rt[{name:?}]({})",
            args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        // An intrinsic (spec 0021) inlines to a native JS expression.
        IrExpr::Intrinsic { name, args, .. } => intrinsic_js(name, args),
        IrExpr::Fn {
            params, body, ret, ..
        } => {
            let params = params
                .iter()
                .map(|param| js_name(&param.name))
                .collect::<Vec<_>>()
                .join(", ");
            if matches!(ret, Type::Option(_)) {
                format!(
                    "function({params}) {{ try {{ return {}; }} catch (__e) {{ if (__e instanceof EmelaNone) return {{ tag: 1, values: [] }}; throw __e; }} }}",
                    emit_expr(body)
                )
            } else {
                format!("function({params}) {{ return {}; }}", emit_expr(body))
            }
        }
        IrExpr::Binary {
            op,
            ty,
            left,
            right,
        } => {
            let a = emit_expr(left);
            let b = emit_expr(right);
            match op {
                BinaryOp::Add => format!("({a} + {b})"),
                BinaryOp::Sub => format!("({a} - {b})"),
                BinaryOp::Mul => format!("({a} * {b})"),
                // Integer division truncates toward zero (spec 0016).
                BinaryOp::Div if *ty == Type::Int => format!("(({a} / {b}) | 0)"),
                BinaryOp::Div => format!("({a} / {b})"),
                BinaryOp::Rem => format!("({a} % {b})"),
                // `++` lowers to `IrExpr::Concat`, never to a Binary.
                BinaryOp::Concat => unreachable!("concat lowers to IrExpr::Concat"),
                BinaryOp::Eq => format!("({a} === {b})"),
                BinaryOp::Lt => format!("({a} < {b})"),
                // `!= > <= >=` desugar to `eq`/`lt` calls in lowering (spec 0027).
                BinaryOp::Ne | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => {
                    unreachable!("derived comparison desugared before lowering")
                }
            }
        }
        IrExpr::EnumValue { tag, payload, .. } => format!(
            "{{ tag: {tag}, values: [{}] }}",
            payload.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        // A record (spec 0006) is a positional array of its fields in
        // declaration order; access indexes it.
        IrExpr::RecordValue { fields, .. } => format!(
            "[{}]",
            fields.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::FieldAccess { target, index, .. } => {
            format!("{}[{index}]", emit_expr(target))
        }
        IrExpr::Match {
            scrutinee, arms, ..
        } => emit_match(scrutinee, arms),
        IrExpr::If {
            cond, then, els, ..
        } => format!(
            "({} ? ({}) : ({}))",
            emit_expr(cond),
            emit_expr(then),
            emit_expr(els)
        ),
        IrExpr::Throw { value } => {
            format!(
                "(() => {{ throw new EmelaError({}); }})()",
                emit_expr(value)
            )
        }
        IrExpr::Try { body, arms, .. } => emit_try(body, arms),
        IrExpr::Question { value, mode, .. } => match mode {
            // A thrown error propagates as a native exception, so `?` is a
            // no-op on the value channel.
            QuestionMode::Throws => emit_expr(value),
            QuestionMode::Option => format!(
                "(() => {{ const __o = {}; if (__o.tag === 1) throw new EmelaNone(); return __o.values[0]; }})()",
                emit_expr(value)
            ),
        },
        IrExpr::Panic { message } => {
            format!(
                "(() => {{ throw new EmelaPanic({}); }})()",
                emit_expr(message)
            )
        }
        // The marker propagates out through the nested IIFEs to the function's
        // trampoline loop (spec 0045), unwinding the stack before it re-enters.
        IrExpr::TailSelfCall { args, .. } => format!(
            "new EmelaTail([{}])",
            args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Lowers a `match` to an IIFE that dispatches on the enum/Option tag.
fn emit_match(scrutinee: &IrExpr, arms: &[IrArm]) -> String {
    let mut out = format!("(() => {{ const __m = {}; ", emit_expr(scrutinee));
    for arm in arms {
        out.push_str(&emit_arm("__m", arm));
    }
    out.push_str("throw new Error(\"non-exhaustive match\"); })()");
    out
}

/// Lowers a `try`/`catch` to an IIFE; thrown `EmelaError`s route to the arms
/// while panics (and other exceptions) propagate.
fn emit_try(body: &IrExpr, arms: &[IrArm]) -> String {
    let mut out = format!(
        "(() => {{ try {{ return {}; }} catch (__e) {{ if (!(__e instanceof EmelaError)) throw __e; const __err = __e.value; ",
        emit_expr(body)
    );
    for arm in arms {
        out.push_str(&emit_arm("__err", arm));
    }
    out.push_str("throw __e; } })()");
    out
}

/// Emits one `match`/`catch` arm testing `subject`.
fn emit_arm(subject: &str, arm: &IrArm) -> String {
    let body = emit_expr(&arm.body);
    let guard = arm
        .guard
        .as_ref()
        .map(|guard| format!("if ({}) ", emit_expr(guard)))
        .unwrap_or_default();
    match &arm.pattern {
        IrPattern::Variant { tag, bindings, .. } => {
            let mut binds = String::new();
            for (index, binding) in bindings.iter().enumerate() {
                if let Some((name, _)) = binding {
                    binds.push_str(&format!(
                        "const {} = {subject}.values[{index}]; ",
                        js_name(name)
                    ));
                }
            }
            format!("if ({subject}.tag === {tag}) {{ {binds}{guard}return {body}; }} ")
        }
        IrPattern::Wildcard { binding } => {
            let bind = binding
                .as_ref()
                .map(|(name, _)| format!("const {} = {subject}; ", js_name(name)))
                .unwrap_or_default();
            format!("{{ {bind}{guard}return {body}; }} ")
        }
    }
}

fn js_name(name: &str) -> String {
    if name == "main" {
        return "main".to_string();
    }
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    // An Emela identifier that happens to be a JavaScript reserved word (e.g. a
    // parameter named `default`) is not a valid JS binding, so prefix it with
    // `$`. Deterministic, so declarations and uses stay in agreement.
    if is_js_reserved(&sanitized) {
        format!("${sanitized}")
    } else {
        sanitized
    }
}

/// JavaScript reserved words (and a few contextually-reserved / literal names)
/// that cannot be used as a plain binding name.
fn is_js_reserved(name: &str) -> bool {
    matches!(
        name,
        "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_codegen::{EffectRow, IrFunction};

    fn main_with(body: IrExpr) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret: Type::Unit,
                throws: None,
                effects: EffectRow::sorted(vec!["io".into()]),
                body,
            }],
        }
    }

    fn platform_call(name: &str) -> IrExpr {
        IrExpr::Platform {
            name: name.into(),
            args: vec![IrExpr::String("hi".into())],
            ret: Type::Unit,
            throws: None,
        }
    }

    #[test]
    fn bundles_runtime_for_used_platform_fns() {
        let artifact = JsBackend
            .compile(
                &main_with(platform_call("io.write_stdout")),
                &BackendOptions::default(),
            )
            .expect("compile");
        let js = String::from_utf8(artifact.bytes).unwrap();
        assert!(js.contains("const __rt = {"), "{js}");
        assert!(
            js.contains("\"io.write_stdout\": (s) => process.stdout.write(s)"),
            "{js}"
        );
        assert!(js.contains("__rt[\"io.write_stdout\"](\"hi\")"), "{js}");
        // An unused platform function is not bundled.
        assert!(!js.contains("write_stderr"), "{js}");
    }

    #[test]
    fn rejects_unprovided_platform_fn() {
        let err = JsBackend
            .compile(
                &main_with(platform_call("fs.read")),
                &BackendOptions::default(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("does not provide"), "{err}");
        assert!(err.to_string().contains("fs.read"), "{err}");
    }

    #[test]
    fn preserves_generated_dollar_names() {
        assert_eq!(js_name("$cmp0"), "$cmp0");
        assert_eq!(js_name("$stmt0"), "$stmt0");
        assert_eq!(js_name("a-b"), "a_b");
    }

    /// A rewritten self-tail-call (spec 0045) trampolines through `EmelaTail`
    /// in a `while (true)` driver loop instead of recursing.
    #[test]
    fn self_tail_call_trampolines() {
        use emela_codegen::IrParam;
        let spin = IrFunction {
            name: "spin".into(),
            params: vec![IrParam {
                name: "n".into(),
                ty: Type::Int,
            }],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::TailSelfCall {
                args: vec![IrExpr::Var {
                    name: "n".into(),
                    ty: Type::Int,
                }],
                ty: Type::Int,
            },
        };
        let main = IrFunction {
            name: "main".into(),
            params: vec![],
            ret: Type::Unit,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Unit,
        };
        let artifact = JsBackend
            .compile(
                &IrProgram {
                    functions: vec![spin, main],
                },
                &BackendOptions::default(),
            )
            .expect("compile");
        let js = String::from_utf8(artifact.bytes).unwrap();
        assert!(js.contains("while (true)"), "{js}");
        assert!(js.contains("new EmelaTail([n])"), "{js}");
        assert!(js.contains("n = __r.args[0];"), "{js}");
    }
}
