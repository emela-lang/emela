//! Effects (spec chapter 8) and external functions (spec chapter 12). Builds IR by hand, emits JS
//! and runs it on node. Handlers are passed in the evidence table and called directly (spec 16.5).
//! The emitted JS is also snapshotted with insta.

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// Calls `main` and prints its return value, or `defect: <message>` on a defect.
const HARNESS: &str = r#"import { main } from "./main.mjs";
try {
  process.stdout.write(String(main()) + "\n");
} catch (e) {
  if (e?.name !== "Defect") throw e;
  process.stdout.write("defect: " + e.message + "\n");
}
"#;

fn run(name: &str, m: Module) -> String {
    let js = emit_module(
        &m,
        &Options {
            runtime: RuntimeMode::Import(None),
        },
    );
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-js-effects")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.mjs"), &js).unwrap();
    std::fs::write(dir.join(RUNTIME_FILE), RUNTIME).unwrap();
    std::fs::write(dir.join("harness.mjs"), HARNESS).unwrap();
    let out = Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("cannot start node");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "node failed:\n{stderr}\n{js}");
    insta::assert_snapshot!(name, js);
    String::from_utf8(out.stdout).unwrap()
}

fn int_op(op: BinOp, a: Expr, b: Expr) -> Expr {
    bin(op, OpTy::Int, a, b)
}

/// An external function calling the host function `path` (a global reference).
fn external(m: &mut Module, name: &str, path: &str, params: Vec<Boundary>, ret: Boundary) -> FnId {
    let f = m.declare(name, false);
    let ps = (0..params.len()).map(|_| m.local("x")).collect();
    m.define_external(
        f,
        ps,
        External {
            bindings: vec![ExternalBinding {
                target: "js".into(),
                host: HostRef::Global(path.into()),
            }],
            params,
            ret,
        },
    );
    f
}

fn main_fn(m: &mut Module, body: Expr) {
    let main = m.declare("main", true);
    m.define(main, vec![], body);
}

#[test]
fn handlers_are_passed_as_evidence() {
    let mut m = Module::new();
    let oops = m.add_enum(EnumDef {
        name: "Oops".into(),
        params: 0,
        variants: vec![VariantDef::unit("Oops")],
        error: true,
    });
    let oops = CtorRef {
        enum_id: oops,
        variant: 0,
    };
    let counter = m.add_effect(EffectDef {
        name: "Counter".into(),
        ops: vec![OpDef::new("add"), OpDef::new("boom")],
    });
    let max = external(
        &mut m,
        "js_max",
        "Math.max",
        vec![Boundary::Plain, Boundary::Plain],
        Boundary::Plain,
    );

    // handler Doubler { add(n) = n * 2, boom() = fail Oops }
    let doubler = m.declare("Doubler", false);
    let n = m.local("n");
    let handler = Expr::Handler {
        name: "Doubler".into(),
        effect: counter,
        ops: vec![
            lambda(vec![n], int_op(BinOp::Mul, var(n), int(2))),
            lambda(vec![], fail(ctor(oops, vec![]))),
        ],
    };
    m.define(doubler, vec![], handler);

    // fn use_it(ev) = (use Counter).add(21)
    let use_it = m.declare("use_it", false);
    let ev = m.local("ev");
    let op = |h: Expr, op: usize, args: Vec<Expr>, fails: bool| Expr::Call {
        callee: Callee::Op {
            handler: Box::new(h),
            effect: counter,
            op,
        },
        args,
        fails,
        suspends: false,
    };
    m.define(
        use_it,
        vec![ev],
        op(
            Expr::Use {
                ctx: Box::new(var(ev)),
                effect: counter,
            },
            0,
            vec![int(21)],
            false,
        ),
    );

    // with Doubler { use_it() + (h.boom() escape { Oops -> 1 }) + js_max(1, 2) }
    let h = m.local("h");
    let ctx = m.local("ev");
    let body = int_op(
        BinOp::Add,
        int_op(
            BinOp::Add,
            call(use_it, vec![var(ctx)]),
            escape(
                op(var(h), 1, vec![], true),
                vec![arm(p_ctor(oops, vec![]), int(1))],
            ),
        ),
        call(max, vec![int(1), int(2)]),
    );
    main_fn(
        &mut m,
        Expr::With {
            binds: vec![
                WithBind::new(h, call(doubler, vec![])),
                WithBind::new(
                    ctx,
                    Expr::Provide {
                        base: None,
                        bindings: vec![(counter, var(h))],
                    },
                ),
            ],
            body: Box::new(body),
        },
    );
    assert_eq!(run("evidence", m), "45\n");
}

#[test]
fn host_strings_are_well_formed() {
    // `String.fromCharCode(0xD800)` returns a lone surrogate (spec 12.5).
    let mut m = Module::new();
    let lone = external(
        &mut m,
        "lone",
        "String.fromCharCode",
        vec![Boundary::Plain],
        Boundary::String,
    );
    main_fn(&mut m, call(lone, vec![int(0xD800)]));
    assert_eq!(run("well_formed", m), "\u{FFFD}\n");
}

#[test]
fn host_exceptions_are_defects() {
    // `JSON.parse("{")` throws a SyntaxError (spec 12.5).
    let mut m = Module::new();
    let parse = external(
        &mut m,
        "parse",
        "JSON.parse",
        vec![Boundary::String],
        Boundary::Plain,
    );
    main_fn(&mut m, call(parse, vec![string("{")]));
    let out = run("foreign_defect", m);
    assert!(out.starts_with("defect: SyntaxError: "), "{out}");
}
