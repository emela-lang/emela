//! IR を手で組み立てて JS を出し，node で実行して標準出力を比べる．
//! 出力した JS は insta のスナップショットでも見る．

use std::path::PathBuf;
use std::process::Command;

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};
use emela_core::build::*;
use emela_core::*;

/// `main` を呼んで返り値を出力し，defect なら `defect: <メッセージ>` と出す．
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

fn run_with(name: &str, m: &Module, runtime: RuntimeMode) -> Run {
    let js = emit_module(m, &Options { runtime });
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-js")
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

/// ループ化をかけてから出力して実行する．JS はスナップショットで見る．
fn run(name: &str, mut m: Module) -> String {
    tail::loopify(&mut m);
    let r = run_with(name, &m, RuntimeMode::Import(None));
    assert!(
        r.stderr.is_empty(),
        "node が失敗した:\n{}\n{}",
        r.stderr,
        r.js
    );
    insta::assert_snapshot!(name, r.js);
    r.stdout
}

/// `run` と同じだが，標準エラー（`dbg` の出力）も返す．
fn run_stderr(name: &str, mut m: Module) -> (String, String) {
    tail::loopify(&mut m);
    let r = run_with(name, &m, RuntimeMode::Import(None));
    insta::assert_snapshot!(name, r.js);
    (r.stdout, r.stderr)
}

/// `main` が defect で終わるときのメッセージ．JS はスナップショットに取らない．
fn defect_of(name: &str, body: Expr) -> String {
    let mut m = Module::new();
    main_fn(&mut m, body);
    let r = run_with(name, &m, RuntimeMode::Import(None));
    assert!(r.stderr.is_empty(), "{}\n{}", r.stderr, r.js);
    r.stdout
}

/// 文字列の部品を改行でつなぐ．
fn lines(parts: Vec<(Expr, Type)>) -> Expr {
    let mut ps = Vec::new();
    for (i, (e, k)) in parts.into_iter().enumerate() {
        if i > 0 {
            ps.push(lit_part("\n"));
        }
        ps.push(value_part(e, k));
    }
    Expr::Concat(ps)
}

fn int_op(op: BinOp, a: Expr, b: Expr) -> Expr {
    bin(op, OpTy::Int, a, b)
}

fn main_fn(m: &mut Module, body: Expr) {
    let main = m.declare("main", true);
    m.define(main, vec![], body);
}

fn panic_(msg: &str) -> Expr {
    builtin(Builtin::Panic, vec![string(msg)])
}

/// fn fact(n: Int, acc: Int64) = if n == 0 then acc else fact(n - 1, acc * Int64(n))
/// Int から Int64 への変換はまだないので，Int64 の数も引数で持ち回る．
fn fact_module() -> (Module, FnId) {
    let mut m = Module::new();
    let fact = m.declare("fact", true);
    let n = m.local("n");
    let k = m.local("k");
    let acc = m.local("acc");
    m.define(
        fact,
        vec![n, k, acc],
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            var(acc),
            call(
                fact,
                vec![
                    int_op(BinOp::Sub, var(n), int(1)),
                    bin(BinOp::Sub, OpTy::Int64, var(k), int64(1)),
                    bin(BinOp::Mul, OpTy::Int64, var(acc), var(k)),
                ],
            ),
        ),
    );
    (m, fact)
}

#[test]
fn factorial_tail_recursive() {
    let (mut m, fact) = fact_module();
    let body = lines(vec![
        (call(fact, vec![int(5), int64(5), int64(1)]), Type::Int64),
        (call(fact, vec![int(20), int64(20), int64(1)]), Type::Int64),
        // 21! は 64bit を超えて巻き戻る．
        (call(fact, vec![int(21), int64(21), int64(1)]), Type::Int64),
        // 10 万回回してもスタックが溢れない．
        (
            call(fact, vec![int(100000), int64(100000), int64(1)]),
            Type::Int64,
        ),
    ]);
    main_fn(&mut m, body);
    assert_eq!(
        run("factorial_tail_recursive", m),
        "120\n2432902008176640000\n-4249290049419214848\n0\n"
    );
}

#[test]
fn factorial_without_loopify_overflows_the_stack() {
    // ループ化しなければ同じ IR でスタックが溢れることを確かめ，テストが意味を持つことを示す．
    let (mut m, fact) = fact_module();
    main_fn(
        &mut m,
        call(fact, vec![int(100000), int64(100000), int64(1)]),
    );
    let r = run_with("factorial_without_loopify", &m, RuntimeMode::Import(None));
    assert!(
        r.stderr.contains("Maximum call stack size exceeded"),
        "{}",
        r.stderr
    );
}

#[test]
fn enum_match() {
    // enum Shape { Circle(Float), Rect { w: Float, h: Float }, Empty }
    // type Point { x: Int, y: Int }
    let mut m = Module::new();
    let shape = m.add_enum(EnumDef {
        name: "Shape".into(),
        params: 0,
        variants: vec![
            VariantDef::positional("Circle", vec![Type::Float]),
            VariantDef::named("Rect", vec![("w", Type::Float), ("h", Type::Float)]),
            VariantDef::unit("Empty"),
        ],
    });
    let point = m.add_enum(EnumDef {
        name: "Point".into(),
        params: 0,
        variants: vec![VariantDef::named(
            "Point",
            vec![("x", Type::Int), ("y", Type::Int)],
        )],
    });
    let circle = CtorRef {
        enum_id: shape,
        variant: 0,
    };
    let rect = CtorRef {
        enum_id: shape,
        variant: 1,
    };
    let empty = CtorRef {
        enum_id: shape,
        variant: 2,
    };
    let pt = CtorRef {
        enum_id: point,
        variant: 0,
    };

    // fn describe(s) = match s {
    //   Circle(r) if r > 10.0 => "big circle"
    //   Circle(r) => "circle #{r}"
    //   Rect { w, h } if w == h => "square #{w}"
    //   Rect { w, h: _ } => "rect #{w}"
    //   Empty => "empty"
    // }
    let describe = m.declare("describe", false);
    let s = m.local("s");
    let r1 = m.local("r");
    let r2 = m.local("r");
    let w1 = m.local("w");
    let h1 = m.local("h");
    let w2 = m.local("w");
    let float_op = |op, a, b| bin(op, OpTy::Float, a, b);
    m.define(
        describe,
        vec![s],
        match_(
            var(s),
            vec![
                arm_if(
                    p_ctor(circle, vec![Pat::Bind(r1)]),
                    float_op(BinOp::Gt, var(r1), float(10.0)),
                    string("big circle"),
                ),
                arm(
                    p_ctor(circle, vec![Pat::Bind(r2)]),
                    Expr::Concat(vec![lit_part("circle "), value_part(var(r2), Type::Float)]),
                ),
                arm_if(
                    p_ctor(rect, vec![Pat::Bind(w1), Pat::Bind(h1)]),
                    float_op(BinOp::Eq, var(w1), var(h1)),
                    Expr::Concat(vec![lit_part("square "), value_part(var(w1), Type::Float)]),
                ),
                arm(
                    p_ctor(rect, vec![Pat::Bind(w2), Pat::Wild]),
                    Expr::Concat(vec![lit_part("rect "), value_part(var(w2), Type::Float)]),
                ),
                arm(p_ctor(empty, vec![]), string("empty")),
            ],
        ),
    );

    // fn norm1(p) = p.x + p.y      （フィールドの参照）
    let norm1 = m.declare("norm1", false);
    let p = m.local("p");
    m.define(
        norm1,
        vec![p],
        int_op(BinOp::Add, field(var(p), pt, 0), field(var(p), pt, 1)),
    );

    let d = |e| (call(describe, vec![e]), Type::String);
    let body = lines(vec![
        d(ctor(circle, vec![float(12.0)])),
        d(ctor(circle, vec![float(1.5)])),
        d(ctor(rect, vec![float(2.0), float(2.0)])),
        d(ctor(rect, vec![float(2.0), float(3.0)])),
        d(ctor(empty, vec![])),
        (
            call(norm1, vec![ctor(pt, vec![int(3), int(-4)])]),
            Type::Int,
        ),
        // 構造の等値比較．
        (
            bin(
                BinOp::Eq,
                OpTy::Structural,
                ctor(rect, vec![float(1.0), float(2.0)]),
                ctor(rect, vec![float(1.0), float(2.0)]),
            ),
            Type::Bool,
        ),
        (
            bin(
                BinOp::Ne,
                OpTy::Structural,
                ctor(empty, vec![]),
                ctor(circle, vec![float(0.0)]),
            ),
            Type::Bool,
        ),
    ]);
    main_fn(&mut m, body);
    assert_eq!(
        run("enum_match", m),
        "big circle\ncircle 1.5\nsquare 2.0\nrect 2.0\nempty\n-1\nTrue\nTrue\n"
    );
}

#[test]
fn list_recursion() {
    let mut m = Module::new();
    // fn sum(xs) = match xs { [] => 0, [x, ..rest] => x + sum(rest) }   （末尾でない再帰）
    let sum = m.declare("sum", false);
    let xs = m.local("xs");
    let x = m.local("x");
    let rest = m.local("rest");
    m.define(
        sum,
        vec![xs],
        match_(
            var(xs),
            vec![
                arm(p_list(vec![], None), int(0)),
                arm(
                    p_list(vec![Pat::Bind(x)], Some(Pat::Bind(rest))),
                    int_op(BinOp::Add, var(x), call(sum, vec![var(rest)])),
                ),
            ],
        ),
    );

    // fn map(xs, f) = match xs { [] => [], [x, ..rest] => [f(x), ..map(rest, f)] }
    let map = m.declare("map", false);
    let xs2 = m.local("xs");
    let f = m.local("f");
    let x2 = m.local("x");
    let rest2 = m.local("rest");
    m.define(
        map,
        vec![xs2, f],
        match_(
            var(xs2),
            vec![
                arm(p_list(vec![], None), list(vec![])),
                arm(
                    p_list(vec![Pat::Bind(x2)], Some(Pat::Bind(rest2))),
                    list_with_tail(
                        vec![call_value(var(f), vec![var(x2)])],
                        call(map, vec![var(rest2), var(f)]),
                    ),
                ),
            ],
        ),
    );

    // fn range(n, acc) = if n == 0 then acc else range(n - 1, [n, ..acc])   （末尾再帰）
    let range = m.declare("range", false);
    let n = m.local("n");
    let acc = m.local("acc");
    m.define(
        range,
        vec![n, acc],
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            var(acc),
            call(
                range,
                vec![
                    int_op(BinOp::Sub, var(n), int(1)),
                    list_with_tail(vec![var(n)], var(acc)),
                ],
            ),
        ),
    );

    // fn len(xs, acc) = match xs { [] => acc, [_, ..rest] => len(rest, acc + 1) }
    let len = m.declare("len", false);
    let xs3 = m.local("xs");
    let acc3 = m.local("acc");
    let rest3 = m.local("rest");
    m.define(
        len,
        vec![xs3, acc3],
        match_(
            var(xs3),
            vec![
                arm(p_list(vec![], None), var(acc3)),
                arm(
                    p_list(vec![Pat::Wild], Some(Pat::Bind(rest3))),
                    call(len, vec![var(rest3), int_op(BinOp::Add, var(acc3), int(1))]),
                ),
            ],
        ),
    );

    // fn shape(xs) = match xs { [] => "none", [a] => "one #{a}", [a, b] => "two #{a} #{b}", [a, ..] => "many from #{a}" }
    let shape = m.declare("shape", false);
    let xs4 = m.local("xs");
    let a1 = m.local("a");
    let a2 = m.local("a");
    let b2 = m.local("b");
    let a3 = m.local("a");
    m.define(
        shape,
        vec![xs4],
        match_(
            var(xs4),
            vec![
                arm(p_list(vec![], None), string("none")),
                arm(
                    p_list(vec![Pat::Bind(a1)], None),
                    Expr::Concat(vec![lit_part("one "), value_part(var(a1), Type::Int)]),
                ),
                arm(
                    p_list(vec![Pat::Bind(a2), Pat::Bind(b2)], None),
                    Expr::Concat(vec![
                        lit_part("two "),
                        value_part(var(a2), Type::Int),
                        lit_part(" "),
                        value_part(var(b2), Type::Int),
                    ]),
                ),
                arm(
                    p_list(vec![Pat::Bind(a3)], Some(Pat::Wild)),
                    Expr::Concat(vec![lit_part("many from "), value_part(var(a3), Type::Int)]),
                ),
            ],
        ),
    );

    let k = m.local("k");
    let y = m.local("y");
    let xs_main = m.local("xs");
    // let k = 10; sum(map([1, 2, 3, 4], fn(y) => y * k))   ラムダは k を捕捉する
    let body = let_(
        k,
        int(10),
        let_(
            xs_main,
            list(vec![int(1), int(2), int(3), int(4)]),
            lines(vec![
                (call(sum, vec![var(xs_main)]), Type::Int),
                (
                    call(
                        sum,
                        vec![call(
                            map,
                            vec![
                                var(xs_main),
                                lambda(vec![y], int_op(BinOp::Mul, var(y), var(k))),
                            ],
                        )],
                    ),
                    Type::Int,
                ),
                (
                    call(
                        len,
                        vec![call(range, vec![int(100000), list(vec![])]), int(0)],
                    ),
                    Type::Int,
                ),
                (call(shape, vec![list(vec![])]), Type::String),
                (call(shape, vec![list(vec![int(7)])]), Type::String),
                (call(shape, vec![list(vec![int(7), int(8)])]), Type::String),
                (call(shape, vec![var(xs_main)]), Type::String),
                // 長いリストの構造比較もスタックを使わない．
                (
                    bin(
                        BinOp::Eq,
                        OpTy::Structural,
                        call(range, vec![int(100000), list(vec![])]),
                        call(range, vec![int(100000), list(vec![])]),
                    ),
                    Type::Bool,
                ),
            ]),
        ),
    );
    main_fn(&mut m, body);
    assert_eq!(
        run("list_recursion", m),
        "10\n100\n100000\nnone\none 7\ntwo 7 8\nmany from 1\nTrue\n"
    );
}

#[test]
fn tuples() {
    let mut m = Module::new();
    // fn swap(t) = match t { (a, b) => (b, a) }
    let swap = m.declare("swap", false);
    let t = m.local("t");
    let a = m.local("a");
    let b = m.local("b");
    m.define(
        swap,
        vec![t],
        match_(
            var(t),
            vec![arm(
                Pat::Tuple(vec![Pat::Bind(a), Pat::Bind(b)]),
                Expr::Tuple(vec![var(b), var(a)]),
            )],
        ),
    );
    // fn classify(t) = match t { (0, _) => "zero first", (_, "x") => "x second", (n, s) => "#{n} #{s}" }
    let classify = m.declare("classify", false);
    let t2 = m.local("t");
    let n = m.local("n");
    let s = m.local("s");
    m.define(
        classify,
        vec![t2],
        match_(
            var(t2),
            vec![
                arm(
                    Pat::Tuple(vec![Pat::Lit(Lit::Int(0)), Pat::Wild]),
                    string("zero first"),
                ),
                arm(
                    Pat::Tuple(vec![Pat::Wild, Pat::Lit(Lit::String("x".into()))]),
                    string("x second"),
                ),
                arm(
                    Pat::Tuple(vec![Pat::Bind(n), Pat::Bind(s)]),
                    Expr::Concat(vec![
                        value_part(var(n), Type::Int),
                        lit_part(" "),
                        value_part(var(s), Type::String),
                    ]),
                ),
            ],
        ),
    );
    let p = m.local("p");
    let body = let_(
        p,
        call(swap, vec![Expr::Tuple(vec![string("y"), int(5)])]),
        lines(vec![
            (call(classify, vec![var(p)]), Type::String),
            (
                call(classify, vec![Expr::Tuple(vec![int(0), string("q")])]),
                Type::String,
            ),
            (
                call(classify, vec![Expr::Tuple(vec![int(1), string("x")])]),
                Type::String,
            ),
            (
                bin(
                    BinOp::Eq,
                    OpTy::Structural,
                    var(p),
                    Expr::Tuple(vec![int(5), string("y")]),
                ),
                Type::Bool,
            ),
        ]),
    );
    main_fn(&mut m, body);
    assert_eq!(run("tuples", m), "5 y\nzero first\nx second\nTrue\n");
}

#[test]
fn string_interpolation() {
    let mut m = Module::new();
    let name = m.local("name");
    let len = |s: &str| builtin(Builtin::StringLength, vec![string(s)]);
    let body = let_(
        name,
        string("Emela"),
        Expr::Concat(vec![
            lit_part("hello, "),
            value_part(var(name), Type::String),
            lit_part("! `${not interpolated}` \\ \"q\"\n"),
            value_part(int(-42), Type::Int),
            lit_part(" "),
            value_part(int64(9_007_199_254_740_993), Type::Int64),
            lit_part(" "),
            value_part(float(1.0), Type::Float),
            lit_part(" "),
            value_part(float(0.1), Type::Float),
            lit_part(" "),
            value_part(float(-2.5e-8), Type::Float),
            lit_part(" "),
            value_part(float(1e21), Type::Float),
            lit_part(" "),
            value_part(bool(true), Type::Bool),
            lit_part(" "),
            value_part(bool(false), Type::Bool),
            lit_part("\n"),
            // 書記素クラスタの数．家族の絵文字は1つ，e + 結合アクセントも1つ．
            value_part(len("👨‍👩‍👧"), Type::Int),
            lit_part(" "),
            value_part(len("e\u{301}"), Type::Int),
            lit_part(" "),
            value_part(len("日本語"), Type::Int),
            lit_part(" "),
            value_part(len(""), Type::Int),
        ]),
    );
    main_fn(&mut m, body);
    assert_eq!(
        run("string_interpolation", m),
        "hello, Emela! `${not interpolated}` \\ \"q\"\n\
         -42 9007199254740993 1.0 0.1 -2.5e-8 1e+21 True False\n\
         1 1 3 0\n"
    );
}

#[test]
fn short_circuit() {
    let mut m = Module::new();
    let x = m.local("x");
    let b = |op, l, r| bin(op, OpTy::Bool, l, r);
    // 右辺が文を要する形（match）でも短絡する．
    let complex_rhs = |m: &mut Module| {
        let v = m.local("v");
        match_(panic_("must not run"), vec![arm(Pat::Bind(v), var(v))])
    };
    let c1 = complex_rhs(&mut m);
    let c2 = complex_rhs(&mut m);
    let body = lines(vec![
        (b(BinOp::And, bool(false), panic_("and")), Type::Bool),
        (b(BinOp::Or, bool(true), panic_("or")), Type::Bool),
        (b(BinOp::And, bool(false), c1), Type::Bool),
        (b(BinOp::Or, bool(true), c2), Type::Bool),
        (
            let_(
                x,
                int(3),
                b(
                    BinOp::And,
                    int_op(BinOp::Gt, var(x), int(1)),
                    match_(
                        var(x),
                        vec![
                            arm(Pat::Lit(Lit::Int(3)), bool(true)),
                            arm(Pat::Wild, bool(false)),
                        ],
                    ),
                ),
            ),
            Type::Bool,
        ),
        (
            unary(
                UnOp::Not,
                OpTy::Bool,
                b(BinOp::Or, bool(false), bool(false)),
            ),
            Type::Bool,
        ),
    ]);
    main_fn(&mut m, body);
    assert_eq!(
        run("short_circuit", m),
        "False\nTrue\nFalse\nTrue\nTrue\nTrue\n"
    );
}

#[test]
fn int_arithmetic() {
    let mut m = Module::new();
    let i = |op, a, b| (int_op(op, int(a), int(b)), Type::Int);
    let l = |op, a, b| (bin(op, OpTy::Int64, int64(a), int64(b)), Type::Int64);
    let fl = |op, a, b| (bin(op, OpTy::Float, float(a), float(b)), Type::Float);
    let body = lines(vec![
        // Int のあふれは2の補数で巻き戻る．
        i(BinOp::Add, i32::MAX, 1),
        i(BinOp::Sub, i32::MIN, 1),
        i(BinOp::Mul, 65536, 65536),
        i(BinOp::Mul, 123456789, 987654321),
        (unary(UnOp::Neg, OpTy::Int, int(i32::MIN)), Type::Int),
        i(BinOp::Div, i32::MIN, -1),
        // 除算は0方向へ切り捨て，剰余は被除数の符号に従う．
        i(BinOp::Div, -7, 2),
        i(BinOp::Rem, -7, 2),
        i(BinOp::Rem, 7, -2),
        i(BinOp::Rem, -6, 3),
        // Int64
        l(BinOp::Add, i64::MAX, 1),
        l(BinOp::Mul, i64::MAX, 2),
        (unary(UnOp::Neg, OpTy::Int64, int64(i64::MIN)), Type::Int64),
        l(BinOp::Div, i64::MIN, -1),
        l(BinOp::Div, -7, 2),
        l(BinOp::Rem, -7, 2),
        // Float のゼロ除算は IEEE のとおり．
        fl(BinOp::Div, 1.0, 0.0),
        fl(BinOp::Div, -1.0, 0.0),
        fl(BinOp::Div, 0.0, 0.0),
        fl(BinOp::Rem, 5.5, 2.0),
        (bin(BinOp::Lt, OpTy::Int, int(-1), int(0)), Type::Bool),
        (
            bin(BinOp::Lt, OpTy::String, string("b"), string("ab")),
            Type::Bool,
        ),
        // コードポイントの順．UTF-16 の順なら U+FF61 が U+1F600 より後になる．
        (
            bin(BinOp::Lt, OpTy::String, string("\u{FF61}"), string("😀")),
            Type::Bool,
        ),
    ]);
    main_fn(&mut m, body);
    assert_eq!(
        run("int_arithmetic", m),
        "-2147483648\n2147483647\n0\n-67153019\n-2147483648\n-2147483648\n\
         -3\n-1\n1\n0\n\
         -9223372036854775808\n-2\n-9223372036854775808\n-9223372036854775808\n-3\n-1\n\
         Infinity\n-Infinity\nNaN\n1.5\nTrue\nFalse\nTrue\n"
    );
}

#[test]
fn division_by_zero_is_defect() {
    let cases = [
        ("int_div_zero", OpTy::Int, BinOp::Div),
        ("int_rem_zero", OpTy::Int, BinOp::Rem),
        ("int64_div_zero", OpTy::Int64, BinOp::Div),
        ("int64_rem_zero", OpTy::Int64, BinOp::Rem),
    ];
    for (name, ty, op) in cases {
        let mut m = Module::new();
        let (a, z) = match ty {
            OpTy::Int => (int(1), int(0)),
            _ => (int64(1), int64(0)),
        };
        main_fn(&mut m, bin(op, ty, a, z));
        let r = run_with(name, &m, RuntimeMode::Import(None));
        assert_eq!(
            r.stdout, "defect: division by zero\n",
            "{name}: {}",
            r.stderr
        );
    }
}

#[test]
fn panic_and_unmatched_are_defects() {
    let mut m = Module::new();
    main_fn(&mut m, panic_("boom"));
    assert_eq!(run("panic", m), "defect: boom\n");

    let mut m = Module::new();
    main_fn(
        &mut m,
        match_(int(3), vec![arm(Pat::Lit(Lit::Int(1)), string("one"))]),
    );
    assert_eq!(
        run("unmatched", m),
        "defect: unreachable: no match arm matched\n"
    );
}

#[test]
fn evaluation_order_is_left_to_right() {
    // f(panic("a"), match 1 { _ => panic("b") })   左の引数が先に評価される
    let mut m = Module::new();
    let f = m.declare("f", false);
    let p = m.local("p");
    let q = m.local("q");
    m.define(f, vec![p, q], string("unreachable"));
    main_fn(
        &mut m,
        call(
            f,
            vec![
                panic_("a"),
                match_(int(1), vec![arm(Pat::Wild, panic_("b"))]),
            ],
        ),
    );
    assert_eq!(run("evaluation_order", m), "defect: a\n");

    // 名前付き引数 f(q: panic("first"), p: panic("second")) は書かれた順に評価する．
    let mut m = Module::new();
    let f = m.declare("f", false);
    let p = m.local("p");
    let q = m.local("q");
    m.define(f, vec![p, q], string("unreachable"));
    let main = m.declare("main", true);
    let body = named::in_written_order(
        &mut m,
        vec![(1, panic_("first")), (0, panic_("second"))],
        |args| call(f, args),
    );
    m.define(main, vec![], body);
    assert_eq!(run("named_args_order", m), "defect: first\n");
}

#[test]
fn closures_in_loop_capture_each_iteration() {
    // fn build(n, acc) = if n == 0 then acc else build(n - 1, [fn() => n, ..acc])
    // fn total(fs, acc) = match fs { [] => acc, [f, ..rest] => total(rest, acc * 10 + f()) }
    let mut m = Module::new();
    let build = m.declare("build", false);
    let n = m.local("n");
    let acc = m.local("acc");
    m.define(
        build,
        vec![n, acc],
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            var(acc),
            call(
                build,
                vec![
                    int_op(BinOp::Sub, var(n), int(1)),
                    list_with_tail(vec![lambda(vec![], var(n))], var(acc)),
                ],
            ),
        ),
    );
    let total = m.declare("total", false);
    let fs = m.local("fs");
    let acc2 = m.local("acc");
    let f = m.local("f");
    let rest = m.local("rest");
    m.define(
        total,
        vec![fs, acc2],
        match_(
            var(fs),
            vec![
                arm(p_list(vec![], None), var(acc2)),
                arm(
                    p_list(vec![Pat::Bind(f)], Some(Pat::Bind(rest))),
                    call(
                        total,
                        vec![
                            var(rest),
                            int_op(
                                BinOp::Add,
                                int_op(BinOp::Mul, var(acc2), int(10)),
                                call_value(var(f), vec![]),
                            ),
                        ],
                    ),
                ),
            ],
        ),
    );
    main_fn(
        &mut m,
        call(total, vec![call(build, vec![int(4), list(vec![])]), int(0)]),
    );
    assert_eq!(run("closures_in_loop", m), "1234\n");
}

#[test]
fn inline_runtime_runs_standalone() {
    // ランタイムと同じスコープに入っても，利用者の関数名と衝突しない．
    let mut m = Module::new();
    let segmenter = m.declare("segmenter", false);
    m.define(segmenter, vec![], int(7));
    main_fn(
        &mut m,
        Expr::Concat(vec![
            value_part(
                builtin(Builtin::StringLength, vec![string("ok")]),
                Type::Int,
            ),
            lit_part(" "),
            value_part(
                int_op(BinOp::Div, call(segmenter, vec![]), int(2)),
                Type::Int,
            ),
        ]),
    );
    let r = run_with("inline_runtime", &m, RuntimeMode::Inline);
    assert!(!r.js.contains("import "), "{}", r.js);
    assert_eq!(r.stdout, "2 3\n", "{}", r.stderr);
}

#[test]
fn short_circuit_tail_call_does_not_grow_stack() {
    // fn any_zero(xs) = match xs { [] => false, [x, ..rest] => x == 0 || any_zero(rest) }
    // fn range(n, acc) = if n == 0 then acc else range(n - 1, [n, ..acc])
    let mut m = Module::new();
    let any_zero = m.declare("any_zero", false);
    let xs = m.local("xs");
    let x = m.local("x");
    let rest = m.local("rest");
    m.define(
        any_zero,
        vec![xs],
        match_(
            var(xs),
            vec![
                arm(p_list(vec![], None), bool(false)),
                arm(
                    p_list(vec![Pat::Bind(x)], Some(Pat::Bind(rest))),
                    bin(
                        BinOp::Or,
                        OpTy::Bool,
                        int_op(BinOp::Eq, var(x), int(0)),
                        call(any_zero, vec![var(rest)]),
                    ),
                ),
            ],
        ),
    );
    let range = m.declare("range", false);
    let n = m.local("n");
    let acc = m.local("acc");
    m.define(
        range,
        vec![n, acc],
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            var(acc),
            call(
                range,
                vec![
                    int_op(BinOp::Sub, var(n), int(1)),
                    list_with_tail(vec![var(n)], var(acc)),
                ],
            ),
        ),
    );
    let big = || call(range, vec![int(100000), list(vec![])]);
    main_fn(
        &mut m,
        lines(vec![
            (call(any_zero, vec![big()]), Type::Bool),
            (
                call(any_zero, vec![list_with_tail(vec![int(0)], big())]),
                Type::Bool,
            ),
        ]),
    );
    assert_eq!(run("short_circuit_tail_call", m), "False\nTrue\n");
}

#[test]
fn runtime_top_level_names_start_with_dollar() {
    // Inline では runtime.mjs の最上位の名前が生成コードと同じスコープに入る．
    // Emela の識別子は `$` を含まないので，`$` で始まる名前なら衝突しない．
    for line in emela_codegen_js::inline_runtime().lines() {
        for kw in ["let ", "const ", "var ", "function ", "class "] {
            if let Some(rest) = line.strip_prefix(kw) {
                assert!(
                    rest.starts_with('$'),
                    "`$` で始まらない最上位の名前: {line}"
                );
            }
        }
    }
}

// ---- 組み込み関数と表示 ----

fn b(op: Builtin, args: Vec<Expr>) -> Expr {
    builtin(op, args)
}

#[test]
fn string_graphemes_and_bytes() {
    let mut m = Module::new();
    let samples = [
        "👍",
        "e\u{301}",
        "👨\u{200d}👩\u{200d}👧",
        "🇯🇵",
        "日本語",
        "",
    ];
    let mut parts = Vec::new();
    for s in samples {
        // 長さ，バイト数，書記素ごとの分割
        parts.push((b(Builtin::StringLength, vec![string(s)]), Type::Int));
        parts.push((b(Builtin::StringByteSize, vec![string(s)]), Type::Int));
        parts.push((
            b(Builtin::StringChars, vec![string(s)]),
            Type::list(Type::String),
        ));
    }
    main_fn(&mut m, lines(parts));
    let shown = [
        "1\n4\n[\"👍\"]",
        "1\n3\n[\"e\u{301}\"]",
        "1\n18\n[\"👨\u{200d}👩\u{200d}👧\"]",
        "1\n8\n[\"🇯🇵\"]",
        "3\n9\n[\"日\", \"本\", \"語\"]",
        "0\n0\n[]",
    ];
    assert_eq!(
        run("string_graphemes_and_bytes", m),
        shown.join("\n") + "\n"
    );
}

#[test]
fn string_split_join_and_search() {
    let mut m = Module::new();
    let split = |s: &str, sep: &str| b(Builtin::StringSplit, vec![string(s), string(sep)]);
    let strs = || Type::list(Type::String);
    let mut parts = vec![
        (split("a,b,,c", ","), strs()),
        (split("", ","), strs()),
        (split("abc", ""), strs()),
        (split("", ""), strs()),
        (split("e\u{301}x", ""), strs()),
        (split("a--b--", "--"), strs()),
    ];
    // join(split(s, sep), sep) == s
    for (s, sep) in [
        ("a,b,,c", ","),
        ("", ","),
        (",", ","),
        ("abc", ""),
        ("x--y", "--"),
    ] {
        parts.push((
            bin(
                BinOp::Eq,
                OpTy::String,
                b(Builtin::StringJoin, vec![split(s, sep), string(sep)]),
                string(s),
            ),
            Type::Bool,
        ));
    }
    let two = |op, a: &str, x: &str| b(op, vec![string(a), string(x)]);
    parts.extend([
        (two(Builtin::StringContains, "hello", "ell"), Type::Bool),
        (two(Builtin::StringContains, "hello", ""), Type::Bool),
        (two(Builtin::StringContains, "e\u{301}", "e"), Type::Bool),
        (two(Builtin::StringStartsWith, "hello", "he"), Type::Bool),
        (two(Builtin::StringStartsWith, "hello", "lo"), Type::Bool),
        (two(Builtin::StringEndsWith, "hello", "lo"), Type::Bool),
        (two(Builtin::StringConcat, "foo", "bar"), Type::String),
        (
            b(
                Builtin::StringTrim,
                vec![string(" \t\n\u{3000}\u{85}a b\u{a0}\r ")],
            ),
            Type::String,
        ),
        // U+FEFF と U+200B は White_Space ではないので残る．
        (
            b(Builtin::StringTrim, vec![string("\u{feff}x\u{200b}")]),
            Type::String,
        ),
        (b(Builtin::StringFromInt, vec![int(-7)]), Type::String),
    ]);
    main_fn(&mut m, lines(parts));
    assert_eq!(
        run("string_split_join_and_search", m),
        [
            r#"["a", "b", "", "c"]"#,
            r#"[""]"#,
            r#"["a", "b", "c"]"#,
            "[]",
            "[\"e\u{301}\", \"x\"]",
            r#"["a", "b", ""]"#,
            "True\nTrue\nTrue\nTrue\nTrue",
            "True\nTrue\nTrue\nTrue\nFalse\nTrue",
            "foobar",
            "a b",
            "\u{feff}x\u{200b}",
            "-7",
        ]
        .join("\n")
            + "\n"
    );
}

#[test]
fn int_conversions_and_checked_add() {
    let mut m = Module::new();
    let opt_int = Type::Enum(m.option_enum(), vec![Type::Int]);
    let add = |a, c| b(Builtin::IntCheckedAdd, vec![int(a), int(c)]);
    let parts = vec![
        (add(1, 2), opt_int.clone()),
        (add(i32::MAX, 1), opt_int.clone()),
        (add(i32::MIN, -1), opt_int.clone()),
        (add(i32::MAX, i32::MIN), opt_int.clone()),
        (b(Builtin::IntToString, vec![int(-42)]), Type::String),
        (b(Builtin::IntToFloat, vec![int(7)]), Type::Float),
        // Int64 にしてからなら Int の最大値の2倍もあふれない．
        (
            bin(
                BinOp::Mul,
                OpTy::Int64,
                b(Builtin::IntToInt64, vec![int(i32::MAX)]),
                b(Builtin::Int64FromInt, vec![int(2)]),
            ),
            Type::Int64,
        ),
        (
            b(Builtin::Int64ToString, vec![int64(i64::MIN)]),
            Type::String,
        ),
        // Int64 から Int へは下位 32bit に巻き戻す．
        (b(Builtin::Int64ToInt, vec![int64(1 << 31)]), Type::Int),
        (
            b(Builtin::Int64ToInt, vec![int64((1 << 32) + 5)]),
            Type::Int,
        ),
        (b(Builtin::Int64ToInt, vec![int64(-1)]), Type::Int),
    ];
    main_fn(&mut m, lines(parts));
    assert_eq!(
        run("int_conversions_and_checked_add", m),
        "Some(3)\nNone\nNone\nSome(-1)\n-42\n7.0\n4294967294\n-9223372036854775808\n\
         -2147483648\n5\n-1\n"
    );
}

#[test]
fn float_conversions() {
    let mut m = Module::new();
    let ops = [
        Builtin::FloatFloor,
        Builtin::FloatCeil,
        Builtin::FloatRound,
        Builtin::FloatTruncate,
    ];
    let xs = [
        2.5,
        -2.5,
        0.5,
        -0.5,
        -0.4,
        1.4999999999999998,
        0.49999999999999994,
        2147483646.6,
        -2147483647.6,
    ];
    let mut parts = Vec::new();
    for x in xs {
        let row = ops
            .iter()
            .enumerate()
            .flat_map(|(i, &op)| {
                let sep = if i == 0 { "" } else { " " };
                [lit_part(sep), value_part(b(op, vec![float(x)]), Type::Int)]
            })
            .collect();
        parts.push((Expr::Concat(row), Type::String));
    }
    for x in [1.0, -0.0, 0.1, 1e21, f64::NAN, f64::NEG_INFINITY] {
        parts.push((b(Builtin::FloatToString, vec![float(x)]), Type::String));
    }
    main_fn(&mut m, lines(parts));
    assert_eq!(
        run("float_conversions", m),
        [
            "2 3 3 2",
            "-3 -2 -3 -2",
            "0 1 1 0",
            "-1 0 -1 0",
            "-1 0 0 0",
            "1 2 1 1",
            "0 1 0 0",
            "2147483646 2147483647 2147483647 2147483646",
            "-2147483648 -2147483647 -2147483648 -2147483647",
        ]
        .join("\n")
            + "\n"
            + "1.0\n0.0\n0.1\n1e+21\nNaN\n-Infinity\n"
    );
}

#[test]
fn float_to_int_out_of_range_is_defect() {
    let cases = [
        (Builtin::FloatFloor, f64::NAN, "NaN"),
        (Builtin::FloatCeil, f64::INFINITY, "Infinity"),
        (Builtin::FloatTruncate, f64::NEG_INFINITY, "-Infinity"),
        (Builtin::FloatTruncate, 2147483648.0, "2147483648.0"),
        (Builtin::FloatRound, 2147483647.5, "2147483647.5"),
        (Builtin::FloatFloor, -2147483648.5, "-2147483648.5"),
        (Builtin::FloatCeil, 2147483647.1, "2147483647.1"),
    ];
    for (i, (op, x, shown)) in cases.into_iter().enumerate() {
        assert_eq!(
            defect_of(&format!("float_to_int_defect_{i}"), b(op, vec![float(x)])),
            format!("defect: cannot convert {shown} to Int\n"),
            "{op:?}({x})"
        );
    }
    // 範囲の端はちょうど収まる．
    let mut m = Module::new();
    main_fn(
        &mut m,
        lines(vec![
            (
                b(Builtin::FloatTruncate, vec![float(-2147483648.9)]),
                Type::Int,
            ),
            (b(Builtin::FloatFloor, vec![float(2147483647.9)]), Type::Int),
        ]),
    );
    assert_eq!(
        run_with("float_to_int_edges", &m, RuntimeMode::Import(None)).stdout,
        "-2147483648\n2147483647\n"
    );
}

#[test]
fn todo_is_defect() {
    assert_eq!(
        defect_of("todo", b(Builtin::Todo, vec![])),
        "defect: not yet implemented\n"
    );
}

/// `enum Shape { Circle(radius: Float), Rect(Float, Float), Empty }` と
/// `enum Tree[A] { Leaf, Node(Tree[A], A, Tree[A]) }` と `type Point(x: Int, y: Int)`．
fn show_module() -> (Module, EnumId, EnumId, EnumId) {
    let mut m = Module::new();
    let shape = m.add_enum(EnumDef {
        name: "Shape".into(),
        params: 0,
        variants: vec![
            VariantDef::named("Circle", vec![("radius", Type::Float)]),
            VariantDef::positional("Rect", vec![Type::Float, Type::Float]),
            VariantDef::unit("Empty"),
        ],
    });
    let tree = m.add_enum(EnumDef {
        name: "Tree".into(),
        params: 1,
        variants: vec![VariantDef::unit("Leaf"), VariantDef::unit("Node")],
    });
    let tree_a = Type::Enum(tree, vec![Type::Param(0)]);
    m.enums[tree].variants[1] =
        VariantDef::positional("Node", vec![tree_a.clone(), Type::Param(0), tree_a]);
    let point = m.add_enum(EnumDef {
        name: "Point".into(),
        params: 0,
        variants: vec![VariantDef::named(
            "Point",
            vec![("x", Type::Int), ("y", Type::Int)],
        )],
    });
    (m, shape, tree, point)
}

#[test]
fn dbg_shows_values_in_source_syntax() {
    let (mut m, shape, tree, point) = show_module();
    let c = |enum_id, variant| CtorRef { enum_id, variant };
    let some = m.some_ctor();
    let none = m.none_ctor();
    let opt = m.option_enum();
    let dbg = |ty: Type, e: Expr| builtin_ty(Builtin::Dbg, vec![ty], vec![e]);
    let shape_ty = Type::Enum(shape, vec![]);
    let tree_s = Type::Enum(tree, vec![Type::String]);
    let leaf = || ctor(c(tree, 0), vec![]);
    let x = m.local("x");
    let values = vec![
        dbg(shape_ty.clone(), ctor(c(shape, 0), vec![float(1.0)])),
        dbg(
            shape_ty.clone(),
            ctor(c(shape, 1), vec![float(1.0), float(2.5)]),
        ),
        dbg(shape_ty.clone(), ctor(c(shape, 2), vec![])),
        dbg(
            Type::Enum(point, vec![]),
            ctor(c(point, 0), vec![int(-1), int(2)]),
        ),
        dbg(Type::list(Type::Int), list(vec![int(1), int(2), int(3)])),
        dbg(Type::list(Type::Int), list(vec![])),
        dbg(
            Type::Tuple(vec![Type::Int, Type::String]),
            Expr::Tuple(vec![int(1), string("a")]),
        ),
        dbg(Type::String, string("say \"hi\"\n\\ #{x} # \t")),
        dbg(Type::String, string("")),
        dbg(Type::Bool, bool(true)),
        dbg(Type::Float, float(3.0)),
        dbg(Type::Int64, int64(i64::MAX)),
        dbg(Type::Unit, unit()),
        dbg(
            Type::list(Type::Enum(opt, vec![Type::list(Type::String)])),
            list(vec![
                ctor(some, vec![list(vec![string("a"), string("b")])]),
                ctor(none, vec![]),
            ]),
        ),
        dbg(
            tree_s.clone(),
            ctor(
                c(tree, 1),
                vec![
                    ctor(c(tree, 1), vec![leaf(), string("l"), leaf()]),
                    string("root"),
                    leaf(),
                ],
            ),
        ),
        dbg(
            Type::list(Type::Tuple(vec![shape_ty.clone(), Type::list(Type::Bool)])),
            list(vec![Expr::Tuple(vec![
                ctor(c(shape, 2), vec![]),
                list(vec![bool(false)]),
            ])]),
        ),
    ];
    // dbg は値をそのまま返す: let x = dbg("héllo") in String.length(x)
    let mut body = let_(
        x,
        dbg(Type::String, string("héllo")),
        b(Builtin::StringLength, vec![var(x)]),
    );
    for v in values.into_iter().rev() {
        let l = m.local("_");
        body = let_(l, v, body);
    }
    main_fn(&mut m, body);
    let (stdout, stderr) = run_stderr("dbg_shows_values_in_source_syntax", m);
    assert_eq!(stdout, "5\n");
    assert_eq!(
        stderr,
        [
            "Circle(radius: 1.0)",
            "Rect(1.0, 2.5)",
            "Empty",
            "Point(x: -1, y: 2)",
            "[1, 2, 3]",
            "[]",
            r#"(1, "a")"#,
            r#""say \"hi\"\n\\ \#{x} # \t""#,
            r#""""#,
            "True",
            "3.0",
            "9223372036854775807",
            "()",
            r#"[Some(["a", "b"]), None]"#,
            r#"Node(Node(Leaf, "l", Leaf), "root", Leaf)"#,
            "[(Empty, [False])]",
            r#""héllo""#,
        ]
        .join("\n")
            + "\n"
    );
}

#[test]
fn interpolation_shows_any_type() {
    // 補間では最上位の String だけをそのまま埋め込み，入れ子の String は引用符で囲む．
    let (mut m, shape, _, _) = show_module();
    let some = m.some_ctor();
    let opt = m.option_enum();
    main_fn(
        &mut m,
        Expr::Concat(vec![
            lit_part("s="),
            value_part(string("a\"b"), Type::String),
            lit_part(" shape="),
            value_part(
                ctor(
                    CtorRef {
                        enum_id: shape,
                        variant: 0,
                    },
                    vec![float(2.0)],
                ),
                Type::Enum(shape, vec![]),
            ),
            lit_part(" xs="),
            value_part(
                list(vec![string("x"), string("y")]),
                Type::list(Type::String),
            ),
            lit_part(" opt="),
            value_part(
                ctor(some, vec![string("z")]),
                Type::Enum(opt, vec![Type::String]),
            ),
            lit_part(" unit="),
            value_part(unit(), Type::Unit),
        ]),
    );
    assert_eq!(
        run("interpolation_shows_any_type", m),
        "s=a\"b shape=Circle(radius: 2.0) xs=[\"x\", \"y\"] opt=Some(\"z\") unit=()\n"
    );
}

#[test]
fn option_from_builtin_matches_user_patterns() {
    // checked_add の結果を，利用者の書く Some / None のパターンで受けられる．
    // match Int.checked_add(a, 1) { Some(n) => n, None => 0 }
    let mut m = Module::new();
    let some = m.some_ctor();
    let none = m.none_ctor();
    let f = m.declare("inc_or_zero", false);
    let a = m.local("a");
    let n = m.local("n");
    m.define(
        f,
        vec![a],
        match_(
            b(Builtin::IntCheckedAdd, vec![var(a), int(1)]),
            vec![
                arm(p_ctor(some, vec![Pat::Bind(n)]), var(n)),
                arm(p_ctor(none, vec![]), int(0)),
            ],
        ),
    );
    let opt_int = Type::Enum(m.option_enum(), vec![Type::Int]);
    main_fn(
        &mut m,
        lines(vec![
            (call(f, vec![int(41)]), Type::Int),
            (call(f, vec![int(i32::MAX)]), Type::Int),
            // None はランタイムの値と同じものを共有するので構造の等値も成り立つ．
            (
                bin(
                    BinOp::Eq,
                    OpTy::Structural,
                    b(Builtin::IntCheckedAdd, vec![int(i32::MAX), int(1)]),
                    ctor(none, vec![]),
                ),
                Type::Bool,
            ),
            (ctor(some, vec![int(1)]), opt_int),
        ]),
    );
    assert_eq!(run("option_from_builtin", m), "42\n0\nTrue\nSome(1)\n");
}

#[test]
fn runtime_implements_every_builtin() {
    for &op in Builtin::ALL {
        let js = op.info().js;
        let decl = format!("export function {js}(");
        assert!(
            RUNTIME.contains(&decl),
            "{op:?} の実装 {js} がランタイムにない"
        );
    }
}
