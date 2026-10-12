//! エラー（仕様 7章）の出力．IR を手で組み立てて JS を出し，node で実行する．
//! 失敗は戻り値（`$Fail`）で返る（16.5）．出力した JS は insta のスナップショットでも見る．

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// `main` を呼んで返り値を出力する．処理されなかったエラーなら `error: <表示>`，
/// defect なら `defect: <メッセージ>` と出す．
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

/// ループ化をかけてから出力して node で実行し，標準出力を返す．JS はスナップショットで見る．
fn run_mode(name: &str, mut m: Module, runtime: RuntimeMode) -> String {
    tail::loopify(&mut m);
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
        .expect("node を起動できない");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "node が失敗した:\n{stderr}\n{js}");
    insta::assert_snapshot!(name, js);
    String::from_utf8(out.stdout).unwrap()
}

fn run(name: &str, m: Module) -> String {
    run_mode(name, m, RuntimeMode::Import(None))
}

/// フィールドを持つ error か，持たない error（`fields` が空）を足す．
fn error(m: &mut Module, name: &str, fields: Vec<(&str, Type)>) -> CtorRef {
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
    });
    CtorRef {
        enum_id,
        variant: 0,
    }
}

fn int_op(op: BinOp, a: Expr, b: Expr) -> Expr {
    bin(op, OpTy::Int, a, b)
}

fn main_fn(m: &mut Module, body: Expr) {
    let main = m.declare("main", true);
    m.define(main, vec![], body);
}

/// `fn find(id) = if id > 0 then id else if id == 0 then fail NotFound(id) else fail Denied`．
fn find_module() -> (Module, FnId, CtorRef, CtorRef) {
    let mut m = Module::new();
    let not_found = error(&mut m, "NotFound", vec![("id", Type::Int)]);
    let denied = error(&mut m, "Denied", vec![]);
    let find = m.declare("find", false);
    let id = m.local("id");
    m.define(
        find,
        vec![id],
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
    // `find(2) + find(0)`: 2つ目の失敗が main の呼び出し元まで伝わる．
    main_fn(
        &mut m,
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
    // 内側の escape は NotFound だけを受け，Denied は外側の escape へ伝わる．
    // 外側の腕の中で起きた失敗は main の呼び出し元へ伝わる．
    let n = m.local("n");
    let inner = |m: &mut Module, arg: i32| {
        let id = m.local("id");
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
    // 腕に書かれた error でも，パターンとガードに合わなければ，エラーのまま外側へ伝わる（7.3）．
    let id = m.local("id");
    main_fn(
        &mut m,
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
    // `apply(f, x) = f(x)` は f の失敗をそのまま返す（末尾位置）．ラムダの中の失敗は
    // ラムダの呼び出し元へ返り，外側の escape の中なら受けられる．
    let apply = m.declare("apply", false);
    let (f, x) = (m.local("f"), m.local("x"));
    m.define(apply, vec![f, x], call_value_fails(var(f), vec![var(x)]));
    let y = m.local("y");
    let lam = lambda(
        vec![y],
        int_op(BinOp::Mul, call_fails(find, vec![var(y)]), int(10)),
    );
    let id = m.local("id");
    main_fn(
        &mut m,
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
    // `count(n) = if n == 0 then fail NotFound(0) else count(n - 1)` は深くてもスタックを使わない．
    let count = m.declare("count", false);
    let n = m.local("n");
    m.define(
        count,
        vec![n],
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            fail(ctor(not_found, vec![var(n)])),
            call_fails(count, vec![int_op(BinOp::Sub, var(n), int(1))]),
        ),
    );
    let id = m.local("id");
    main_fn(
        &mut m,
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
    // error の値は普通の値．比べられ，補間で表示できる（7.1，10.6）．
    let e = m.local("e");
    main_fn(
        &mut m,
        let_(
            e,
            ctor(not_found, vec![int(1)]),
            Expr::Concat(vec![
                value_part(var(e), Type::Enum(not_found.enum_id, vec![])),
                lit_part(" "),
                value_part(ctor(denied, vec![]), Type::Enum(denied.enum_id, vec![])),
                lit_part(" "),
                value_part(
                    bin(
                        BinOp::Eq,
                        OpTy::Structural,
                        var(e),
                        ctor(not_found, vec![int(1)]),
                    ),
                    Type::Bool,
                ),
            ]),
        ),
    );
    assert_eq!(run("error_values", m), "NotFound(id: 1) Denied True\n");
}

#[test]
fn unshowable_error_shows_its_name() {
    // 関数のフィールドを持つ error は表示できない（10.6）ので，処理されなかったときは名前だけを出す．
    // ランタイムを埋め込んでも，起動側は `Symbol.for` の鍵で失敗を見分けられる．
    let mut m = Module::new();
    let callback = error(&mut m, "Callback", vec![("f", Type::Fn)]);
    main_fn(&mut m, fail(ctor(callback, vec![lambda(vec![], int(1))])));
    assert_eq!(
        run_mode("unshowable_error", m, RuntimeMode::Inline),
        "error: Callback\n"
    );
}
