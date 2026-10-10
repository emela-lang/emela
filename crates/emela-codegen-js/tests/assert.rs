//! `assert`（仕様 13.2）の IR を手で組み立てて JS を出し，node で実行して結果を比べる．

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// `main` を呼び，defect なら `defect: <メッセージ>` と出す．
const HARNESS: &str = r#"import { main } from "./main.mjs";
try {
  process.stdout.write(String(main()) + "\n");
} catch (e) {
  if (e?.name !== "Defect") throw e;
  process.stdout.write("defect: " + e.message + "\n");
}
"#;

struct Run {
    js: String,
    stdout: String,
    stderr: String,
}

/// `body` を本体とする `main` を出力して実行する．
fn run(name: &str, build: impl FnOnce(&mut Module) -> Expr) -> Run {
    let mut m = Module::new();
    let body = build(&mut m);
    let main = m.declare("main", true);
    m.define(main, vec![], body);
    let js = emit_module(
        &m,
        &Options {
            runtime: RuntimeMode::Import(None),
        },
    );
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-js-assert")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.mjs"), &js).unwrap();
    std::fs::write(dir.join(RUNTIME_FILE), RUNTIME).unwrap();
    std::fs::write(dir.join("harness.mjs"), HARNESS).unwrap();
    let out = Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("node を起動できない");
    Run {
        js,
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// assert の後に `"ok"` を返す本体．
fn then_ok(m: &mut Module, assert: Expr) -> Expr {
    let unit = m.local("unit");
    let_(unit, assert, string("ok"))
}

fn eq(ty: Type, op_ty: OpTy, lhs: Expr, rhs: Expr, text: &str) -> Expr {
    Assert::compare(BinOp::Eq, op_ty, ty, lhs, rhs, text)
}

#[test]
fn passing_comparison_continues() {
    let r = run("pass", |m| {
        let a = eq(Type::Int, OpTy::Int, int(3), int(3), "3 == 3");
        let b = Assert::compare(
            BinOp::Lt,
            OpTy::String,
            Type::String,
            string("a"),
            string("b"),
            "",
        );
        let c = Assert::bool(bool(true), "True");
        let (u, v) = (m.local("u"), m.local("v"));
        let body = then_ok(m, c);
        let_(u, a, let_(v, b, body))
    });
    assert_eq!(r.stderr, "");
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn failing_comparison_shows_both_sides() {
    let r = run("strings", |m| {
        let a = eq(
            Type::String,
            OpTy::String,
            string("bob"),
            string("alice"),
            "user.name == \"alice\"",
        );
        then_ok(m, a)
    });
    assert_eq!(
        r.stdout,
        "defect: assertion failed: user.name == \"alice\"\n  left: \"bob\"\n right: \"alice\"\n"
    );
    insta::assert_snapshot!("failing_comparison", r.js);
}

#[test]
fn structural_values_are_shown_in_source_syntax() {
    let r = run("lists", |m| {
        let some = m.some_ctor();
        let ty = Type::list(Type::Enum(m.option_enum(), vec![Type::Int]));
        let a = eq(
            ty,
            OpTy::Structural,
            list(vec![ctor(some, vec![int(1)]), ctor(some, vec![int(2)])]),
            list(vec![ctor(some, vec![int(1)])]),
            "xs == [Some(1)]",
        );
        then_ok(m, a)
    });
    assert_eq!(
        r.stdout,
        "defect: assertion failed: xs == [Some(1)]\n  left: [Some(1), Some(2)]\n right: [Some(1)]\n"
    );
}

#[test]
fn operands_are_evaluated_once_from_left() {
    // dbg は値を標準エラーに出して返す．比較と表示で2回使っても1回ずつしか出ない．
    let r = run("once", |m| {
        let dbg = |n| builtin_ty(Builtin::Dbg, vec![Type::Int], vec![int(n)]);
        let a = Assert::compare(BinOp::Ge, OpTy::Int, Type::Int, dbg(1), dbg(2), "");
        then_ok(m, a)
    });
    assert_eq!(r.stderr, "1\n2\n");
    assert_eq!(
        r.stdout,
        "defect: assertion failed: left >= right\n  left: 1\n right: 2\n"
    );
}

#[test]
fn non_comparison_looks_only_at_the_value() {
    let r = run("bool", |m| {
        let a = Assert::bool(
            builtin(Builtin::StringContains, vec![string("abc"), string("x")]),
            "String.contains(s, \"x\")",
        );
        then_ok(m, a)
    });
    assert_eq!(
        r.stdout,
        "defect: assertion failed: String.contains(s, \"x\")\n"
    );

    let r = run("bool_no_text", |m| {
        let a = Assert::bool(bool(false), "");
        then_ok(m, a)
    });
    assert_eq!(r.stdout, "defect: assertion failed\n");
}

#[test]
fn assert_as_an_argument_keeps_evaluation_order() {
    // (dbg(1), assert dbg(2) == 2) の組．左の要素が assert より先に評価される．
    let r = run("order", |m| {
        let dbg = |n| builtin_ty(Builtin::Dbg, vec![Type::Int], vec![int(n)]);
        let t = Expr::Tuple(vec![
            dbg(1),
            eq(Type::Int, OpTy::Int, dbg(2), int(2), "dbg(2) == 2"),
        ]);
        let pair = m.local("pair");
        let_(pair, t, string("ok"))
    });
    assert_eq!(r.stderr, "1\n2\n");
    assert_eq!(r.stdout, "ok\n");
}
