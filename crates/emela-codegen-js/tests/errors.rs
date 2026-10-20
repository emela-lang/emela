//! Errors (spec chapter 7). Builds IR by hand, emits JS and runs it on node. Failures come back as
//! return values (`$Fail`, spec 16.5). The emitted JS is also snapshotted with insta.

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// Checks that the hand-built IR is well typed (`emela_core::verify`).
fn assert_well_typed(m: &Module) {
    if let Err(errors) = verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("ill-typed IR:\n{}", lines.join("\n"));
    }
}

/// Calls `main` and prints its return value; `error: <shown>` for an unhandled error, and
/// `defect: <message>` for a defect.
const HARNESS: &str = r#"import { main } from "./main.mjs";
try {
  const value = main();
  const show = value?.[Symbol.for("emela.fail")];
  if (typeof show === "function") {
    process.stdout.write("error: " + show.call(value) + "\n");
  } else {
    process.stdout.write(String(value) + "\n");
  }
} catch (e) {
  if (e?.name !== "Defect") throw e;
  process.stdout.write("defect: " + e.message + "\n");
}
"#;

/// Loopifies, emits, runs on node and returns stdout. The JS is checked by snapshot.
fn run_mode(name: &str, mut m: Module, runtime: RuntimeMode) -> String {
    tail::loopify(&mut m);
    assert_well_typed(&m);
    let js = emit_module(&m, &Options { runtime });
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-js-errors")
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

fn run(name: &str, m: Module) -> String {
    run_mode(name, m, RuntimeMode::Import(None))
}

/// Adds an error with fields, or without (`fields` empty). Unless a field is a function, it is
/// shown by its structure when it reaches the entry, as for `derive Show` (monomorphization
/// records that in `Module::error_display`, spec 14.3).
fn error(m: &mut Module, name: &str, fields: Vec<(&str, TypeId)>) -> CtorRef {
    let showable = !fields
        .iter()
        .any(|&(_, t)| matches!(m.ty(t), emela_core::Type::Fn(_)));
    let variant = if fields.is_empty() {
        VariantDef::unit(name)
    } else {
        VariantDef::named(name, fields)
    };
    let enum_id = m.add_enum(EnumDef {
        name: name.into(),
        params: 0,
        variants: vec![variant],
        error: true,
        external: false,
    });
    if showable {
        m.error_display
            .insert(enum_id, emela_core::ErrorDisplay::Structure);
    }
    CtorRef {
        enum_id,
        variant: 0,
    }
}

fn int_op(op: BinOp, a: Expr, b: Expr) -> Expr {
    bin(op, OpTy::Int, a, b)
}

fn main_fn(m: &mut Module, ret: TypeId, body: Expr) {
    let main = m.declare("main", true);
    m.define(main, vec![], ret, body);
}

/// `fn find(id) = if id > 0 then id else if id == 0 then fail NotFound(id) else fail Denied`．
fn find_module() -> (Module, FnId, CtorRef, CtorRef) {
    let mut m = Module::new();
    let not_found = error(&mut m, "NotFound", vec![("id", TypeId::INT)]);
    let denied = error(&mut m, "Denied", vec![]);
    let find = m.declare("find", false);
    let id = m.local("id", TypeId::INT);
    m.define(
        find,
        vec![id],
        TypeId::INT,
        if_(
            int_op(BinOp::Gt, var(id), int(0)),
            var(id),
            if_(
                int_op(BinOp::Eq, var(id), int(0)),
                fail(ctor(not_found, vec![var(id)])),
                fail(ctor(denied, vec![])),
            ),
        ),
    );
    (m, find, not_found, denied)
}

#[test]
fn unhandled_error_returns_to_caller() {
    let (mut m, find, _, _) = find_module();
    // `find(2) + find(0)`: the second failure propagates to main's caller.
    main_fn(
        &mut m,
        TypeId::INT,
        int_op(
            BinOp::Add,
            call_fails(find, vec![int(2)]),
            call_fails(find, vec![int(0)]),
        ),
    );
    assert_eq!(run("unhandled_error", m), "error: NotFound(id: 0)\n");
}

#[test]
fn escape_catches_listed_errors_and_passes_others() {
    let (mut m, find, not_found, denied) = find_module();
    // The inner escape catches only NotFound; Denied propagates to the outer escape. A failure
    // inside an outer arm propagates to main's caller.
    let n = m.local("n", TypeId::INT);
    let inner = |m: &mut Module, arg: i32| {
        let id = m.local("id", TypeId::INT);
        escape(
            call_fails(find, vec![int(arg)]),
            vec![arm(
                p_ctor(not_found, vec![Pat::Bind(id)]),
                int_op(BinOp::Sub, int(100), var(id)),
            )],
        )
    };
    let a = inner(&mut m, 0);
    let b = inner(&mut m, -1);
    let c = inner(&mut m, -1);
    main_fn(
        &mut m,
        TypeId::INT,
        let_(
            n,
            int_op(
                BinOp::Add,
                a,
                escape(b, vec![arm(p_ctor(denied, vec![]), int(1000))]),
            ),
            int_op(
                BinOp::Add,
                var(n),
                escape(
                    c,
                    vec![arm(
                        p_ctor(denied, vec![]),
                        fail(ctor(not_found, vec![var(n)])),
                    )],
                ),
            ),
        ),
    );
    assert_eq!(run("nested_escape", m), "error: NotFound(id: 1100)\n");
}

#[test]
fn escape_arm_without_match_propagates() {
    let (mut m, find, not_found, _) = find_module();
    // Even for an error named in an arm, a value that misses the pattern and guard propagates
    // outward as an error (spec 7.3).
    let id = m.local("id", TypeId::INT);
    main_fn(
        &mut m,
        TypeId::INT,
        escape(
            call_fails(find, vec![int(0)]),
            vec![
                arm(p_ctor(not_found, vec![Pat::Lit(Lit::Int(5))]), int(1)),
                arm_if(
                    p_ctor(not_found, vec![Pat::Bind(id)]),
                    int_op(BinOp::Gt, var(id), int(0)),
                    int(2),
                ),
            ],
        ),
    );
    assert_eq!(run("partial_escape", m), "error: NotFound(id: 0)\n");
}

#[test]
fn failures_in_lambdas_return_from_the_lambda() {
    let (mut m, find, not_found, _) = find_module();
    // `apply(f, x) = f(x)` returns f's failure as is (tail position). A failure inside a lambda
    // returns to the lambda's caller, and an enclosing escape can catch it.
    let apply = m.declare("apply", false);
    let f_ty = m.func(FnType {
        params: vec![TypeId::INT],
        ret: TypeId::INT,
        fails: true,
        suspends: false,
        ev: false,
        sets: None,
    });
    let (f, x) = (m.local("f", f_ty), m.local("x", TypeId::INT));
    m.define(
        apply,
        vec![f, x],
        TypeId::INT,
        call_value_fails(var(f), vec![var(x)]),
    );
    let y = m.local("y", TypeId::INT);
    let lam = lambda(
        vec![y],
        f_ty,
        int_op(BinOp::Mul, call_fails(find, vec![var(y)]), int(10)),
    );
    let id = m.local("id", TypeId::INT);
    main_fn(
        &mut m,
        TypeId::INT,
        int_op(
            BinOp::Add,
            call_fails(apply, vec![lam.clone(), int(3)]),
            escape(
                call_fails(apply, vec![lam, int(0)]),
                vec![arm(p_ctor(not_found, vec![Pat::Bind(id)]), int(7))],
            ),
        ),
    );
    assert_eq!(run("lambda_failure", m), "37\n");
}

#[test]
fn failing_self_tail_calls_loop() {
    let (mut m, _, not_found, _) = find_module();
    // `count(n) = if n == 0 then fail NotFound(0) else count(n - 1)` uses no stack however deep.
    let count = m.declare("count", false);
    let n = m.local("n", TypeId::INT);
    m.define(
        count,
        vec![n],
        TypeId::INT,
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            fail(ctor(not_found, vec![var(n)])),
            call_fails(count, vec![int_op(BinOp::Sub, var(n), int(1))]),
        ),
    );
    let id = m.local("id", TypeId::INT);
    main_fn(
        &mut m,
        TypeId::INT,
        escape(
            call_fails(count, vec![int(1_000_000)]),
            vec![arm(
                p_ctor(not_found, vec![Pat::Bind(id)]),
                int_op(BinOp::Sub, var(id), int(1)),
            )],
        ),
    );
    assert_eq!(run("failing_loop", m), "-1\n");
}

#[test]
fn errors_are_values_with_their_own_tag() {
    let (mut m, _, not_found, denied) = find_module();
    // Error values are ordinary values: comparable and showable in interpolation (spec 7.1, 10.6).
    let not_found_ty = m.enum_type(not_found.enum_id, vec![]);
    let denied_ty = m.enum_type(denied.enum_id, vec![]);
    let e = m.local("e", not_found_ty);
    main_fn(
        &mut m,
        TypeId::STRING,
        let_(
            e,
            ctor(not_found, vec![int(1)]),
            Expr::Concat(vec![
                value_part(var(e), not_found_ty),
                lit_part(" "),
                value_part(ctor(denied, vec![]), denied_ty),
                lit_part(" "),
                value_part(
                    bin(
                        BinOp::Eq,
                        OpTy::Structural,
                        var(e),
                        ctor(not_found, vec![int(1)]),
                    ),
                    TypeId::BOOL,
                ),
            ]),
        ),
    );
    assert_eq!(run("error_values", m), "NotFound(id: 1) Denied True\n");
}

#[test]
fn unshowable_error_shows_its_name() {
    // An unhandled error with a function field prints only its name (spec 14.3). Even with the
    // runtime inlined, the entry tells failures apart by the `Symbol.for` key.
    let mut m = Module::new();
    let f = m.func(FnType {
        params: vec![],
        ret: TypeId::INT,
        fails: false,
        suspends: false,
        ev: false,
        sets: None,
    });
    let callback = error(&mut m, "Callback", vec![("f", f)]);
    main_fn(
        &mut m,
        TypeId::NEVER,
        fail(ctor(callback, vec![lambda(vec![], f, int(1))])),
    );
    assert_eq!(
        run_mode("unshowable_error", m, RuntimeMode::Inline),
        "error: Callback\n"
    );
}
