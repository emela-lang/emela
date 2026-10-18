//! Builds IR by hand, emits JS, runs it on node and compares stdout. The emitted JS is also
//! snapshotted with insta.

use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

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

/// Calls `main` and prints its return value, or `defect: <message>` on a defect.
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
    assert_well_typed(m);
    let js = emit_module(m, &Options { runtime });
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-js")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.mjs"), &js).unwrap();
    std::fs::write(dir.join(RUNTIME_FILE), RUNTIME).unwrap();
    std::fs::write(dir.join("harness.mjs"), HARNESS).unwrap();
    let mut child = Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("cannot start node");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().expect("cannot wait for node").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("node timed out running {name}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let out = child
        .wait_with_output()
        .expect("cannot collect node output");
    Run {
        js,
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// Loopifies, emits and runs. The JS is checked by snapshot.
fn run(name: &str, mut m: Module) -> String {
    tail::loopify(&mut m);
    let r = run_with(name, &m, RuntimeMode::Import(None));
    assert!(r.stderr.is_empty(), "node failed:\n{}\n{}", r.stderr, r.js);
    insta::assert_snapshot!(name, r.js);
    r.stdout
}

/// Like `run`, but also returns stderr (the output of `dbg`).
fn run_stderr(name: &str, mut m: Module) -> (String, String) {
    tail::loopify(&mut m);
    let r = run_with(name, &m, RuntimeMode::Import(None));
    insta::assert_snapshot!(name, r.js);
    (r.stdout, r.stderr)
}

/// The message when `main` ends in a defect. The JS is not snapshotted.
fn defect_of(name: &str, ret: TypeId, body: Expr) -> String {
    let mut m = Module::new();
    main_fn(&mut m, ret, body);
    let r = run_with(name, &m, RuntimeMode::Import(None));
    assert!(r.stderr.is_empty(), "{}\n{}", r.stderr, r.js);
    r.stdout
}

/// Joins string parts with newlines.
fn lines(parts: Vec<(Expr, TypeId)>) -> Expr {
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

fn main_fn(m: &mut Module, ret: TypeId, body: Expr) {
    let main = m.declare("main", true);
    m.define(main, vec![], ret, body);
}

/// `List[Int]`．
fn int_list() -> Type {
    Type::List(TypeId::INT)
}

/// A function type that neither fails, suspends nor takes an evidence table.
fn fn_ty(params: Vec<TypeId>, ret: TypeId) -> Type {
    Type::Fn(FnType {
        params,
        ret,
        fails: false,
        suspends: false,
        ev: false,
        sets: None,
    })
}

fn panic_(msg: &str) -> Expr {
    builtin(Builtin::Panic, vec![string(msg)])
}

/// fn fact(n: Int, acc: Int64) = if n == 0 then acc else fact(n - 1, acc * Int64(n))
fn fact_module() -> (Module, FnId) {
    let mut m = Module::new();
    let fact = m.declare("fact", true);
    let n = m.local("n", TypeId::INT);
    let k = m.local("k", TypeId::INT64);
    let acc = m.local("acc", TypeId::INT64);
    m.define(
        fact,
        vec![n, k, acc],
        TypeId::INT64,
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
        (call(fact, vec![int(5), int64(5), int64(1)]), TypeId::INT64),
        (
            call(fact, vec![int(20), int64(20), int64(1)]),
            TypeId::INT64,
        ),
        // 21! overflows 64 bits and wraps.
        (
            call(fact, vec![int(21), int64(21), int64(1)]),
            TypeId::INT64,
        ),
        // 100,000 iterations do not overflow the stack.
        (
            call(fact, vec![int(100000), int64(100000), int64(1)]),
            TypeId::INT64,
        ),
    ]);
    main_fn(&mut m, TypeId::STRING, body);
    assert_eq!(
        run("factorial_tail_recursive", m),
        "120\n2432902008176640000\n-4249290049419214848\n0\n"
    );
}

#[test]
fn factorial_without_loopify_overflows_the_stack() {
    // Without loopification, the same IR overflows the stack; this shows the test is meaningful.
    let (mut m, fact) = fact_module();
    main_fn(
        &mut m,
        TypeId::INT64,
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
            VariantDef::positional("Circle", vec![TypeId::FLOAT]),
            VariantDef::named("Rect", vec![("w", TypeId::FLOAT), ("h", TypeId::FLOAT)]),
            VariantDef::unit("Empty"),
        ],
        error: false,
        external: false,
    });
    let point = m.add_enum(EnumDef {
        name: "Point".into(),
        params: 0,
        variants: vec![VariantDef::named(
            "Point",
            vec![("x", TypeId::INT), ("y", TypeId::INT)],
        )],
        error: false,
        external: false,
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
    let s = m.local("s", Type::Enum(shape, vec![]));
    let r1 = m.local("r", TypeId::FLOAT);
    let r2 = m.local("r", TypeId::FLOAT);
    let w1 = m.local("w", TypeId::FLOAT);
    let h1 = m.local("h", TypeId::FLOAT);
    let w2 = m.local("w", TypeId::FLOAT);
    let float_op = |op, a, b| bin(op, OpTy::Float, a, b);
    m.define(
        describe,
        vec![s],
        TypeId::STRING,
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
                    Expr::Concat(vec![
                        lit_part("circle "),
                        value_part(var(r2), TypeId::FLOAT),
                    ]),
                ),
                arm_if(
                    p_ctor(rect, vec![Pat::Bind(w1), Pat::Bind(h1)]),
                    float_op(BinOp::Eq, var(w1), var(h1)),
                    Expr::Concat(vec![
                        lit_part("square "),
                        value_part(var(w1), TypeId::FLOAT),
                    ]),
                ),
                arm(
                    p_ctor(rect, vec![Pat::Bind(w2), Pat::Wild]),
                    Expr::Concat(vec![lit_part("rect "), value_part(var(w2), TypeId::FLOAT)]),
                ),
                arm(p_ctor(empty, vec![]), string("empty")),
            ],
        ),
    );

    // fn norm1(p) = p.x + p.y      (field access)
    let norm1 = m.declare("norm1", false);
    let p = m.local("p", Type::Enum(point, vec![]));
    m.define(
        norm1,
        vec![p],
        TypeId::INT,
        int_op(BinOp::Add, field(var(p), pt, 0), field(var(p), pt, 1)),
    );

    let d = |e| (call(describe, vec![e]), TypeId::STRING);
    let body = lines(vec![
        d(ctor(circle, vec![float(12.0)])),
        d(ctor(circle, vec![float(1.5)])),
        d(ctor(rect, vec![float(2.0), float(2.0)])),
        d(ctor(rect, vec![float(2.0), float(3.0)])),
        d(ctor(empty, vec![])),
        (
            call(norm1, vec![ctor(pt, vec![int(3), int(-4)])]),
            TypeId::INT,
        ),
        // Structural equality.
        (
            bin(
                BinOp::Eq,
                OpTy::Structural,
                ctor(rect, vec![float(1.0), float(2.0)]),
                ctor(rect, vec![float(1.0), float(2.0)]),
            ),
            TypeId::BOOL,
        ),
        (
            bin(
                BinOp::Ne,
                OpTy::Structural,
                ctor(empty, vec![]),
                ctor(circle, vec![float(0.0)]),
            ),
            TypeId::BOOL,
        ),
    ]);
    main_fn(&mut m, TypeId::STRING, body);
    assert_eq!(
        run("enum_match", m),
        "big circle\ncircle 1.5\nsquare 2.0\nrect 2.0\nempty\n-1\nTrue\nTrue\n"
    );
}

#[test]
fn list_recursion() {
    let mut m = Module::new();
    // fn sum(xs) = match xs { [] => 0, [x, ..rest] => x + sum(rest) }   (non-tail recursion)
    let sum = m.declare("sum", false);
    let xs = m.local("xs", int_list());
    let x = m.local("x", TypeId::INT);
    let rest = m.local("rest", int_list());
    m.define(
        sum,
        vec![xs],
        TypeId::INT,
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
    // Only used with Int here.
    let map = m.declare("map", false);
    let xs2 = m.local("xs", int_list());
    let f = m.local("f", fn_ty(vec![TypeId::INT], TypeId::INT));
    let x2 = m.local("x", TypeId::INT);
    let rest2 = m.local("rest", int_list());
    m.define(
        map,
        vec![xs2, f],
        int_list(),
        match_(
            var(xs2),
            vec![
                arm(p_list(vec![], None), list(TypeId::INT, vec![])),
                arm(
                    p_list(vec![Pat::Bind(x2)], Some(Pat::Bind(rest2))),
                    list_with_tail(
                        TypeId::INT,
                        vec![call_value(var(f), vec![var(x2)])],
                        call(map, vec![var(rest2), var(f)]),
                    ),
                ),
            ],
        ),
    );

    // fn range(n, acc) = if n == 0 then acc else range(n - 1, [n, ..acc])   (tail recursion)
    let range = m.declare("range", false);
    let n = m.local("n", TypeId::INT);
    let acc = m.local("acc", int_list());
    m.define(
        range,
        vec![n, acc],
        int_list(),
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            var(acc),
            call(
                range,
                vec![
                    int_op(BinOp::Sub, var(n), int(1)),
                    list_with_tail(TypeId::INT, vec![var(n)], var(acc)),
                ],
            ),
        ),
    );

    // fn len(xs, acc) = match xs { [] => acc, [_, ..rest] => len(rest, acc + 1) }
    let len = m.declare("len", false);
    let xs3 = m.local("xs", int_list());
    let acc3 = m.local("acc", TypeId::INT);
    let rest3 = m.local("rest", int_list());
    m.define(
        len,
        vec![xs3, acc3],
        TypeId::INT,
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
    let xs4 = m.local("xs", int_list());
    let a1 = m.local("a", TypeId::INT);
    let a2 = m.local("a", TypeId::INT);
    let b2 = m.local("b", TypeId::INT);
    let a3 = m.local("a", TypeId::INT);
    m.define(
        shape,
        vec![xs4],
        TypeId::STRING,
        match_(
            var(xs4),
            vec![
                arm(p_list(vec![], None), string("none")),
                arm(
                    p_list(vec![Pat::Bind(a1)], None),
                    Expr::Concat(vec![lit_part("one "), value_part(var(a1), TypeId::INT)]),
                ),
                arm(
                    p_list(vec![Pat::Bind(a2), Pat::Bind(b2)], None),
                    Expr::Concat(vec![
                        lit_part("two "),
                        value_part(var(a2), TypeId::INT),
                        lit_part(" "),
                        value_part(var(b2), TypeId::INT),
                    ]),
                ),
                arm(
                    p_list(vec![Pat::Bind(a3)], Some(Pat::Wild)),
                    Expr::Concat(vec![
                        lit_part("many from "),
                        value_part(var(a3), TypeId::INT),
                    ]),
                ),
            ],
        ),
    );

    let k = m.local("k", TypeId::INT);
    let y = m.local("y", TypeId::INT);
    let xs_main = m.local("xs", int_list());
    let int_fn = m.intern(fn_ty(vec![TypeId::INT], TypeId::INT));
    // let k = 10; sum(map([1, 2, 3, 4], fn(y) => y * k))   the lambda captures k
    let body = let_(
        k,
        int(10),
        let_(
            xs_main,
            list(TypeId::INT, vec![int(1), int(2), int(3), int(4)]),
            lines(vec![
                (call(sum, vec![var(xs_main)]), TypeId::INT),
                (
                    call(
                        sum,
                        vec![call(
                            map,
                            vec![
                                var(xs_main),
                                lambda(vec![y], int_fn, int_op(BinOp::Mul, var(y), var(k))),
                            ],
                        )],
                    ),
                    TypeId::INT,
                ),
                (
                    call(
                        len,
                        vec![
                            call(range, vec![int(100000), list(TypeId::INT, vec![])]),
                            int(0),
                        ],
                    ),
                    TypeId::INT,
                ),
                (call(shape, vec![list(TypeId::INT, vec![])]), TypeId::STRING),
                (
                    call(shape, vec![list(TypeId::INT, vec![int(7)])]),
                    TypeId::STRING,
                ),
                (
                    call(shape, vec![list(TypeId::INT, vec![int(7), int(8)])]),
                    TypeId::STRING,
                ),
                (call(shape, vec![var(xs_main)]), TypeId::STRING),
                // Structural comparison of long lists does not use the stack either.
                (
                    bin(
                        BinOp::Eq,
                        OpTy::Structural,
                        call(range, vec![int(100000), list(TypeId::INT, vec![])]),
                        call(range, vec![int(100000), list(TypeId::INT, vec![])]),
                    ),
                    TypeId::BOOL,
                ),
            ]),
        ),
    );
    main_fn(&mut m, TypeId::STRING, body);
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
    let t = m.local("t", Type::Tuple(vec![TypeId::STRING, TypeId::INT]));
    let a = m.local("a", TypeId::STRING);
    let b = m.local("b", TypeId::INT);
    m.define(
        swap,
        vec![t],
        Type::Tuple(vec![TypeId::INT, TypeId::STRING]),
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
    let t2 = m.local("t", Type::Tuple(vec![TypeId::INT, TypeId::STRING]));
    let n = m.local("n", TypeId::INT);
    let s = m.local("s", TypeId::STRING);
    m.define(
        classify,
        vec![t2],
        TypeId::STRING,
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
                        value_part(var(n), TypeId::INT),
                        lit_part(" "),
                        value_part(var(s), TypeId::STRING),
                    ]),
                ),
            ],
        ),
    );
    let p = m.local("p", Type::Tuple(vec![TypeId::INT, TypeId::STRING]));
    let body = let_(
        p,
        call(swap, vec![Expr::Tuple(vec![string("y"), int(5)])]),
        lines(vec![
            (call(classify, vec![var(p)]), TypeId::STRING),
            (
                call(classify, vec![Expr::Tuple(vec![int(0), string("q")])]),
                TypeId::STRING,
            ),
            (
                call(classify, vec![Expr::Tuple(vec![int(1), string("x")])]),
                TypeId::STRING,
            ),
            (
                bin(
                    BinOp::Eq,
                    OpTy::Structural,
                    var(p),
                    Expr::Tuple(vec![int(5), string("y")]),
                ),
                TypeId::BOOL,
            ),
        ]),
    );
    main_fn(&mut m, TypeId::STRING, body);
    assert_eq!(run("tuples", m), "5 y\nzero first\nx second\nTrue\n");
}

#[test]
fn string_interpolation() {
    let mut m = Module::new();
    let name = m.local("name", TypeId::STRING);
    let len = |s: &str| builtin(Builtin::StringLength, vec![string(s)]);
    let body = let_(
        name,
        string("Emela"),
        Expr::Concat(vec![
            lit_part("hello, "),
            value_part(var(name), TypeId::STRING),
            lit_part("! `${not interpolated}` \\ \"q\"\n"),
            value_part(int(-42), TypeId::INT),
            lit_part(" "),
            value_part(int64(9_007_199_254_740_993), TypeId::INT64),
            lit_part(" "),
            value_part(float(1.0), TypeId::FLOAT),
            lit_part(" "),
            value_part(float(0.1), TypeId::FLOAT),
            lit_part(" "),
            value_part(float(-2.5e-8), TypeId::FLOAT),
            lit_part(" "),
            value_part(float(1e21), TypeId::FLOAT),
            lit_part(" "),
            value_part(bool(true), TypeId::BOOL),
            lit_part(" "),
            value_part(bool(false), TypeId::BOOL),
            lit_part("\n"),
            // Grapheme cluster counts: a family emoji is one, and e plus a combining accent is one.
            value_part(len("👨‍👩‍👧"), TypeId::INT),
            lit_part(" "),
            value_part(len("e\u{301}"), TypeId::INT),
            lit_part(" "),
            value_part(len("日本語"), TypeId::INT),
            lit_part(" "),
            value_part(len(""), TypeId::INT),
        ]),
    );
    main_fn(&mut m, TypeId::STRING, body);
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
    let x = m.local("x", TypeId::INT);
    let b = |op, l, r| bin(op, OpTy::Bool, l, r);
    // Short-circuits even when the right side needs statements (match).
    let complex_rhs = |m: &mut Module| {
        let v = m.local("v", TypeId::NEVER);
        match_(panic_("must not run"), vec![arm(Pat::Bind(v), var(v))])
    };
    let c1 = complex_rhs(&mut m);
    let c2 = complex_rhs(&mut m);
    let body = lines(vec![
        (b(BinOp::And, bool(false), panic_("and")), TypeId::BOOL),
        (b(BinOp::Or, bool(true), panic_("or")), TypeId::BOOL),
        (b(BinOp::And, bool(false), c1), TypeId::BOOL),
        (b(BinOp::Or, bool(true), c2), TypeId::BOOL),
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
            TypeId::BOOL,
        ),
        (
            unary(
                UnOp::Not,
                OpTy::Bool,
                b(BinOp::Or, bool(false), bool(false)),
            ),
            TypeId::BOOL,
        ),
    ]);
    main_fn(&mut m, TypeId::STRING, body);
    assert_eq!(
        run("short_circuit", m),
        "False\nTrue\nFalse\nTrue\nTrue\nTrue\n"
    );
}

#[test]
fn int_arithmetic() {
    let mut m = Module::new();
    let i = |op, a, b| (int_op(op, int(a), int(b)), TypeId::INT);
    let l = |op, a, b| (bin(op, OpTy::Int64, int64(a), int64(b)), TypeId::INT64);
    let fl = |op, a, b| (bin(op, OpTy::Float, float(a), float(b)), TypeId::FLOAT);
    let body = lines(vec![
        // Int overflow wraps in two's complement.
        i(BinOp::Add, i32::MAX, 1),
        i(BinOp::Sub, i32::MIN, 1),
        i(BinOp::Mul, 65536, 65536),
        i(BinOp::Mul, 123456789, 987654321),
        (unary(UnOp::Neg, OpTy::Int, int(i32::MIN)), TypeId::INT),
        i(BinOp::Div, i32::MIN, -1),
        // Division truncates toward zero; the remainder takes the dividend's sign.
        i(BinOp::Div, -7, 2),
        i(BinOp::Rem, -7, 2),
        i(BinOp::Rem, 7, -2),
        i(BinOp::Rem, -6, 3),
        // Int64
        l(BinOp::Add, i64::MAX, 1),
        l(BinOp::Mul, i64::MAX, 2),
        (
            unary(UnOp::Neg, OpTy::Int64, int64(i64::MIN)),
            TypeId::INT64,
        ),
        l(BinOp::Div, i64::MIN, -1),
        l(BinOp::Div, -7, 2),
        l(BinOp::Rem, -7, 2),
        // Float division by zero follows IEEE.
        fl(BinOp::Div, 1.0, 0.0),
        fl(BinOp::Div, -1.0, 0.0),
        fl(BinOp::Div, 0.0, 0.0),
        fl(BinOp::Rem, 5.5, 2.0),
        (bin(BinOp::Lt, OpTy::Int, int(-1), int(0)), TypeId::BOOL),
        (
            bin(BinOp::Lt, OpTy::String, string("b"), string("ab")),
            TypeId::BOOL,
        ),
        // Code point order. In UTF-16 order, U+FF61 would come after U+1F600.
        (
            bin(BinOp::Lt, OpTy::String, string("\u{FF61}"), string("😀")),
            TypeId::BOOL,
        ),
    ]);
    main_fn(&mut m, TypeId::STRING, body);
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
        let ret = match ty {
            OpTy::Int => TypeId::INT,
            _ => TypeId::INT64,
        };
        main_fn(&mut m, ret, bin(op, ty, a, z));
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
    main_fn(&mut m, TypeId::NEVER, panic_("boom"));
    assert_eq!(run("panic", m), "defect: boom\n");

    let mut m = Module::new();
    main_fn(
        &mut m,
        TypeId::STRING,
        match_(int(3), vec![arm(Pat::Lit(Lit::Int(1)), string("one"))]),
    );
    assert_eq!(
        run("unmatched", m),
        "defect: unreachable: no match arm matched\n"
    );
}

#[test]
fn evaluation_order_is_left_to_right() {
    // f(panic("a"), match 1 { _ => panic("b") })   the left argument is evaluated first
    let mut m = Module::new();
    let f = m.declare("f", false);
    let p = m.local("p", TypeId::NEVER);
    let q = m.local("q", TypeId::NEVER);
    m.define(f, vec![p, q], TypeId::STRING, string("unreachable"));
    main_fn(
        &mut m,
        TypeId::STRING,
        call(
            f,
            vec![
                panic_("a"),
                match_(int(1), vec![arm(Pat::Wild, panic_("b"))]),
            ],
        ),
    );
    assert_eq!(run("evaluation_order", m), "defect: a\n");

    // Named arguments f(q: panic("first"), p: panic("second")) are evaluated in written order.
    let mut m = Module::new();
    let f = m.declare("f", false);
    let p = m.local("p", TypeId::NEVER);
    let q = m.local("q", TypeId::NEVER);
    m.define(f, vec![p, q], TypeId::STRING, string("unreachable"));
    let main = m.declare("main", true);
    let body = named::in_written_order(
        &mut m,
        vec![
            (1, panic_("first"), TypeId::NEVER),
            (0, panic_("second"), TypeId::NEVER),
        ],
        |args| call(f, args),
    );
    m.define(main, vec![], TypeId::STRING, body);
    assert_eq!(run("named_args_order", m), "defect: first\n");
}

#[test]
fn closures_in_loop_capture_each_iteration() {
    // fn build(n, acc) = if n == 0 then acc else build(n - 1, [fn() => n, ..acc])
    // fn total(fs, acc) = match fs { [] => acc, [f, ..rest] => total(rest, acc * 10 + f()) }
    let mut m = Module::new();
    let build = m.declare("build", false);
    let thunk = m.intern(fn_ty(vec![], TypeId::INT));
    let thunks = m.list(thunk);
    let n = m.local("n", TypeId::INT);
    let acc = m.local("acc", thunks);
    m.define(
        build,
        vec![n, acc],
        thunks,
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            var(acc),
            call(
                build,
                vec![
                    int_op(BinOp::Sub, var(n), int(1)),
                    list_with_tail(thunk, vec![lambda(vec![], thunk, var(n))], var(acc)),
                ],
            ),
        ),
    );
    let total = m.declare("total", false);
    let fs = m.local("fs", thunks);
    let acc2 = m.local("acc", TypeId::INT);
    let f = m.local("f", fn_ty(vec![], TypeId::INT));
    let rest = m.local("rest", thunks);
    m.define(
        total,
        vec![fs, acc2],
        TypeId::INT,
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
        TypeId::INT,
        call(
            total,
            vec![call(build, vec![int(4), list(thunk, vec![])]), int(0)],
        ),
    );
    assert_eq!(run("closures_in_loop", m), "1234\n");
}

#[test]
fn inline_runtime_runs_standalone() {
    // Even in the same scope as the runtime, user function names do not clash.
    let mut m = Module::new();
    let segmenter = m.declare("segmenter", false);
    m.define(segmenter, vec![], TypeId::INT, int(7));
    main_fn(
        &mut m,
        TypeId::STRING,
        Expr::Concat(vec![
            value_part(
                builtin(Builtin::StringLength, vec![string("ok")]),
                TypeId::INT,
            ),
            lit_part(" "),
            value_part(
                int_op(BinOp::Div, call(segmenter, vec![]), int(2)),
                TypeId::INT,
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
    let xs = m.local("xs", int_list());
    let x = m.local("x", TypeId::INT);
    let rest = m.local("rest", int_list());
    m.define(
        any_zero,
        vec![xs],
        TypeId::BOOL,
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
    let n = m.local("n", TypeId::INT);
    let acc = m.local("acc", int_list());
    m.define(
        range,
        vec![n, acc],
        int_list(),
        if_(
            int_op(BinOp::Eq, var(n), int(0)),
            var(acc),
            call(
                range,
                vec![
                    int_op(BinOp::Sub, var(n), int(1)),
                    list_with_tail(TypeId::INT, vec![var(n)], var(acc)),
                ],
            ),
        ),
    );
    let big = || call(range, vec![int(100000), list(TypeId::INT, vec![])]);
    main_fn(
        &mut m,
        TypeId::STRING,
        lines(vec![
            (call(any_zero, vec![big()]), TypeId::BOOL),
            (
                call(
                    any_zero,
                    vec![list_with_tail(TypeId::INT, vec![int(0)], big())],
                ),
                TypeId::BOOL,
            ),
        ]),
    );
    assert_eq!(run("short_circuit_tail_call", m), "False\nTrue\n");
}

#[test]
fn runtime_top_level_names_start_with_dollar() {
    // With Inline, runtime.mjs's top-level names share the generated code's scope. Emela
    // identifiers never contain `$`, so names starting with `$` cannot clash.
    for line in emela_codegen_js::inline_runtime().lines() {
        for kw in ["let ", "const ", "var ", "function ", "class "] {
            if let Some(rest) = line.strip_prefix(kw) {
                assert!(
                    rest.starts_with('$'),
                    "top-level name not starting with `$`: {line}"
                );
            }
        }
    }
}

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
    let strs = m.list(TypeId::STRING);
    let mut parts = Vec::new();
    for s in samples {
        // Length, byte size, and splitting into graphemes
        parts.push((b(Builtin::StringLength, vec![string(s)]), TypeId::INT));
        parts.push((b(Builtin::StringByteSize, vec![string(s)]), TypeId::INT));
        parts.push((b(Builtin::StringChars, vec![string(s)]), strs));
    }
    main_fn(&mut m, TypeId::STRING, lines(parts));
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
    let strs = m.list(TypeId::STRING);
    let mut parts = vec![
        (split("a,b,,c", ","), strs),
        (split("", ","), strs),
        (split("abc", ""), strs),
        (split("", ""), strs),
        (split("e\u{301}x", ""), strs),
        (split("a--b--", "--"), strs),
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
            TypeId::BOOL,
        ));
    }
    let two = |op, a: &str, x: &str| b(op, vec![string(a), string(x)]);
    parts.extend([
        (two(Builtin::StringContains, "hello", "ell"), TypeId::BOOL),
        (two(Builtin::StringContains, "hello", ""), TypeId::BOOL),
        (two(Builtin::StringContains, "e\u{301}", "e"), TypeId::BOOL),
        (two(Builtin::StringStartsWith, "hello", "he"), TypeId::BOOL),
        (two(Builtin::StringStartsWith, "hello", "lo"), TypeId::BOOL),
        (two(Builtin::StringEndsWith, "hello", "lo"), TypeId::BOOL),
        (two(Builtin::StringAppend, "foo", "bar"), TypeId::STRING),
        (
            b(
                Builtin::StringTrim,
                vec![string(" \t\n\u{3000}\u{85}a b\u{a0}\r ")],
            ),
            TypeId::STRING,
        ),
        // U+FEFF and U+200B are not White_Space, so they stay.
        (
            b(Builtin::StringTrim, vec![string("\u{feff}x\u{200b}")]),
            TypeId::STRING,
        ),
        (b(Builtin::IntToString, vec![int(-7)]), TypeId::STRING),
    ]);
    main_fn(&mut m, TypeId::STRING, lines(parts));
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
    let opt_id = m.option_enum();
    let opt_int = m.enum_type(opt_id, vec![TypeId::INT]);
    let add = |a, c| b(Builtin::IntCheckedAdd, vec![int(a), int(c)]);
    let parts = vec![
        (add(1, 2), opt_int),
        (add(i32::MAX, 1), opt_int),
        (add(i32::MIN, -1), opt_int),
        (add(i32::MAX, i32::MIN), opt_int),
        (b(Builtin::IntToString, vec![int(-42)]), TypeId::STRING),
        (b(Builtin::FloatFromInt, vec![int(7)]), TypeId::FLOAT),
        // Converted to Int64 first, twice Int's maximum does not overflow.
        (
            bin(
                BinOp::Mul,
                OpTy::Int64,
                b(Builtin::Int64FromInt, vec![int(i32::MAX)]),
                b(Builtin::Int64FromInt, vec![int(2)]),
            ),
            TypeId::INT64,
        ),
        (
            b(Builtin::Int64ToString, vec![int64(i64::MIN)]),
            TypeId::STRING,
        ),
        // Int64 to Int keeps the low 32 bits.
        (b(Builtin::Int64ToInt, vec![int64(1 << 31)]), TypeId::INT),
        (
            b(Builtin::Int64ToInt, vec![int64((1 << 32) + 5)]),
            TypeId::INT,
        ),
        (b(Builtin::Int64ToInt, vec![int64(-1)]), TypeId::INT),
    ];
    main_fn(&mut m, TypeId::STRING, lines(parts));
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
                [
                    lit_part(sep),
                    value_part(b(op, vec![float(x)]), TypeId::INT),
                ]
            })
            .collect();
        parts.push((Expr::Concat(row), TypeId::STRING));
    }
    for x in [1.0, -0.0, 0.1, 1e21, f64::NAN, f64::NEG_INFINITY] {
        parts.push((b(Builtin::FloatToString, vec![float(x)]), TypeId::STRING));
    }
    main_fn(&mut m, TypeId::STRING, lines(parts));
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
            defect_of(
                &format!("float_to_int_defect_{i}"),
                TypeId::INT,
                b(op, vec![float(x)])
            ),
            format!("defect: cannot convert {shown} to Int\n"),
            "{op:?}({x})"
        );
    }
    // The ends of the range fit exactly.
    let mut m = Module::new();
    main_fn(
        &mut m,
        TypeId::STRING,
        lines(vec![
            (
                b(Builtin::FloatTruncate, vec![float(-2147483648.9)]),
                TypeId::INT,
            ),
            (
                b(Builtin::FloatFloor, vec![float(2147483647.9)]),
                TypeId::INT,
            ),
        ]),
    );
    assert_eq!(
        run_with("float_to_int_edges", &m, RuntimeMode::Import(None)).stdout,
        "-2147483648\n2147483647\n"
    );
}

#[test]
fn int_bitwise_and_shifts() {
    let mut m = Module::new();
    let two = |op, a, c| b(op, vec![int(a), int(c)]);
    let mut parts = vec![
        (two(Builtin::IntBitAnd, 0b1100, 0b1010), TypeId::INT),
        (two(Builtin::IntBitOr, 0b1100, 0b1010), TypeId::INT),
        (two(Builtin::IntBitXor, 0b1100, 0b1010), TypeId::INT),
        (b(Builtin::IntBitNot, vec![int(0)]), TypeId::INT),
        (two(Builtin::IntBitAnd, -1, i32::MIN), TypeId::INT),
    ];
    // Shift amounts 0, 1, 31, 32 (exactly the width), and 100 (beyond the width).
    for op in [
        Builtin::IntShiftLeft,
        Builtin::IntShiftRight,
        Builtin::IntShiftRightUnsigned,
    ] {
        for (n, by) in [(1, 31), (-8, 1), (-1, 0), (-1, 31), (-1, 32), (5, 100)] {
            parts.push((two(op, n, by), TypeId::INT));
        }
    }
    main_fn(&mut m, TypeId::STRING, lines(parts));
    let shifts = [
        // shift_left: 1 << 31 wraps to the minimum.
        "-2147483648\n-16\n-1\n-2147483648\n0\n0",
        // shift_right: fills with the sign; 0 or -1 at or beyond the width.
        "0\n-4\n-1\n-1\n-1\n0",
        // shift_right_unsigned: the zero-filled result read as an Int.
        "0\n2147483644\n-1\n1\n0\n0",
    ];
    assert_eq!(
        run("int_bitwise_and_shifts", m),
        format!("8\n14\n6\n-1\n-2147483648\n{}\n", shifts.join("\n"))
    );
}

#[test]
fn int64_bitwise_and_shifts() {
    let mut m = Module::new();
    let two = |op, a, c| b(op, vec![int64(a), int64(c)]);
    let shift = |op, a, by| b(op, vec![int64(a), int(by)]);
    let mut parts = vec![
        (two(Builtin::Int64BitAnd, 0b1100, 0b1010), TypeId::INT64),
        (two(Builtin::Int64BitOr, 0b1100, 0b1010), TypeId::INT64),
        (two(Builtin::Int64BitXor, -1, i64::MAX), TypeId::INT64),
        (
            b(Builtin::Int64BitNot, vec![int64(i64::MAX)]),
            TypeId::INT64,
        ),
    ];
    for op in [
        Builtin::Int64ShiftLeft,
        Builtin::Int64ShiftRight,
        Builtin::Int64ShiftRightUnsigned,
    ] {
        for (n, by) in [(1, 63), (-8, 1), (-1, 0), (-1, 63), (-1, 64), (5, 100)] {
            parts.push((shift(op, n, by), TypeId::INT64));
        }
    }
    // Shifts beyond 32 are done correctly in 64 bits.
    parts.push((shift(Builtin::Int64ShiftLeft, 3, 40), TypeId::INT64));
    main_fn(&mut m, TypeId::STRING, lines(parts));
    let min = i64::MIN.to_string();
    let shifts = [
        format!("{min}\n-16\n-1\n{min}\n0\n0"),
        "0\n-4\n-1\n-1\n-1\n0".to_owned(),
        format!("0\n{}\n-1\n1\n0\n0", (u64::MAX - 7) >> 1),
    ];
    assert_eq!(
        run("int64_bitwise_and_shifts", m),
        format!(
            "8\n14\n{min}\n{min}\n{}\n{}\n",
            shifts.join("\n"),
            3_i64 << 40
        )
    );
}

#[test]
fn negative_shift_is_defect() {
    let cases = [
        (b(Builtin::IntShiftLeft, vec![int(1), int(-1)]), TypeId::INT),
        (
            b(Builtin::IntShiftRight, vec![int(1), int(-1)]),
            TypeId::INT,
        ),
        (
            b(Builtin::IntShiftRightUnsigned, vec![int(1), int(-1)]),
            TypeId::INT,
        ),
        (
            b(Builtin::Int64ShiftLeft, vec![int64(1), int(-1)]),
            TypeId::INT64,
        ),
        (
            b(Builtin::Int64ShiftRight, vec![int64(1), int(-1)]),
            TypeId::INT64,
        ),
        (
            b(Builtin::Int64ShiftRightUnsigned, vec![int64(1), int(-1)]),
            TypeId::INT64,
        ),
    ];
    for (i, (e, ty)) in cases.into_iter().enumerate() {
        assert_eq!(
            defect_of(&format!("negative_shift_{i}"), ty, e),
            "defect: negative shift amount: -1\n"
        );
    }
}

#[test]
fn string_code_points() {
    let mut m = Module::new();
    let opt_id = m.option_enum();
    let opt_str = m.enum_type(opt_id, vec![TypeId::STRING]);
    let ints = m.list(TypeId::INT);
    let points = |s: &str| b(Builtin::StringCodePoints, vec![string(s)]);
    let from = |n| b(Builtin::StringFromCodePoint, vec![int(n)]);
    let parts = vec![
        (points("aé"), ints),
        // A character written as a surrogate pair is still one code point.
        (points("👍"), ints),
        // One grapheme cluster, but two code points.
        (points("e\u{301}"), ints),
        (points(""), ints),
        (from(0x41), opt_str),
        (from(0x1f44d), opt_str),
        (from(0), opt_str),
        (from(0x10ffff), opt_str),
        (from(-1), opt_str),
        (from(0xd800), opt_str),
        (from(0xdfff), opt_str),
        (from(0x110000), opt_str),
    ];
    main_fn(&mut m, TypeId::STRING, lines(parts));
    assert_eq!(
        run("string_code_points", m),
        [
            "[97, 233]",
            "[128077]",
            "[101, 769]",
            "[]",
            r#"Some("A")"#,
            "Some(\"👍\")",
            r#"Some("\u{0}")"#,
            "Some(\"\u{10ffff}\")",
            "None\nNone\nNone\nNone",
        ]
        .join("\n")
            + "\n"
    );
}

#[test]
fn todo_is_defect() {
    assert_eq!(
        defect_of("todo", TypeId::NEVER, b(Builtin::Todo, vec![])),
        "defect: not yet implemented\n"
    );
}

/// `enum Shape { Circle(radius: Float), Rect(Float, Float), Empty }`,
/// `enum Tree[A] { Leaf, Node(Tree[A], A, Tree[A]) }` and `type Point(x: Int, y: Int)`.
fn show_module() -> (Module, EnumId, EnumId, EnumId) {
    let mut m = Module::new();
    let shape = m.add_enum(EnumDef {
        name: "Shape".into(),
        params: 0,
        variants: vec![
            VariantDef::named("Circle", vec![("radius", TypeId::FLOAT)]),
            VariantDef::positional("Rect", vec![TypeId::FLOAT, TypeId::FLOAT]),
            VariantDef::unit("Empty"),
        ],
        error: false,
        external: false,
    });
    let tree = m.add_enum(EnumDef {
        name: "Tree".into(),
        params: 1,
        variants: vec![VariantDef::unit("Leaf"), VariantDef::unit("Node")],
        error: false,
        external: false,
    });
    let a = m.type_param(0);
    let tree_a = m.enum_type(tree, vec![a]);
    m.enums[tree].variants[1] = VariantDef::positional("Node", vec![tree_a, a, tree_a]);
    let point = m.add_enum(EnumDef {
        name: "Point".into(),
        params: 0,
        variants: vec![VariantDef::named(
            "Point",
            vec![("x", TypeId::INT), ("y", TypeId::INT)],
        )],
        error: false,
        external: false,
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
    let dbg = |ty: TypeId, e: Expr| builtin_ty(Builtin::Dbg, vec![ty], vec![e]);
    let shape_ty = m.enum_type(shape, vec![]);
    let tree_s = m.enum_type(tree, vec![TypeId::STRING]);
    let leaf = || ctor_ty(c(tree, 0), vec![TypeId::STRING], vec![]);
    let x = m.local("x", TypeId::STRING);
    let point_ty = m.enum_type(point, vec![]);
    let ints = m.list(TypeId::INT);
    let int_string = m.tuple(vec![TypeId::INT, TypeId::STRING]);
    let strs = m.list(TypeId::STRING);
    let opt_strs = m.enum_type(opt, vec![strs]);
    let opt_strs_list = m.list(opt_strs);
    let bools = m.list(TypeId::BOOL);
    let shape_bools = m.tuple(vec![shape_ty, bools]);
    let shape_bools_list = m.list(shape_bools);
    let typed = |ty: TypeId, e: Expr| (dbg(ty, e), ty);
    let values = vec![
        typed(shape_ty, ctor(c(shape, 0), vec![float(1.0)])),
        typed(shape_ty, ctor(c(shape, 1), vec![float(1.0), float(2.5)])),
        typed(shape_ty, ctor(c(shape, 2), vec![])),
        typed(point_ty, ctor(c(point, 0), vec![int(-1), int(2)])),
        typed(ints, list(TypeId::INT, vec![int(1), int(2), int(3)])),
        typed(ints, list(TypeId::INT, vec![])),
        typed(int_string, Expr::Tuple(vec![int(1), string("a")])),
        typed(TypeId::STRING, string("say \"hi\"\n\\ #{x} # \t")),
        typed(TypeId::STRING, string("")),
        typed(TypeId::BOOL, bool(true)),
        typed(TypeId::FLOAT, float(3.0)),
        typed(TypeId::INT64, int64(i64::MAX)),
        typed(TypeId::UNIT, unit()),
        typed(
            opt_strs_list,
            list(
                opt_strs,
                vec![
                    ctor_ty(
                        some,
                        vec![strs],
                        vec![list(TypeId::STRING, vec![string("a"), string("b")])],
                    ),
                    ctor_ty(none, vec![strs], vec![]),
                ],
            ),
        ),
        typed(
            tree_s,
            ctor_ty(
                c(tree, 1),
                vec![TypeId::STRING],
                vec![
                    ctor_ty(
                        c(tree, 1),
                        vec![TypeId::STRING],
                        vec![leaf(), string("l"), leaf()],
                    ),
                    string("root"),
                    leaf(),
                ],
            ),
        ),
        typed(
            shape_bools_list,
            list(
                shape_bools,
                vec![Expr::Tuple(vec![
                    ctor(c(shape, 2), vec![]),
                    list(TypeId::BOOL, vec![bool(false)]),
                ])],
            ),
        ),
    ];
    // dbg returns its value as is: let x = dbg("héllo") in String.length(x)
    let mut body = let_(
        x,
        dbg(TypeId::STRING, string("héllo")),
        b(Builtin::StringLength, vec![var(x)]),
    );
    for (v, ty) in values.into_iter().rev() {
        let l = m.local("_", ty);
        body = let_(l, v, body);
    }
    main_fn(&mut m, TypeId::INT, body);
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
    // Interpolation embeds only a top-level String as is; nested Strings are quoted.
    let (mut m, shape, _, _) = show_module();
    let some = m.some_ctor();
    let opt = m.option_enum();
    let shape_ty = m.enum_type(shape, vec![]);
    let strs = m.list(TypeId::STRING);
    let opt_string = m.enum_type(opt, vec![TypeId::STRING]);
    main_fn(
        &mut m,
        TypeId::STRING,
        Expr::Concat(vec![
            lit_part("s="),
            value_part(string("a\"b"), TypeId::STRING),
            lit_part(" shape="),
            value_part(
                ctor(
                    CtorRef {
                        enum_id: shape,
                        variant: 0,
                    },
                    vec![float(2.0)],
                ),
                shape_ty,
            ),
            lit_part(" xs="),
            value_part(list(TypeId::STRING, vec![string("x"), string("y")]), strs),
            lit_part(" opt="),
            value_part(
                ctor_ty(some, vec![TypeId::STRING], vec![string("z")]),
                opt_string,
            ),
            lit_part(" unit="),
            value_part(unit(), TypeId::UNIT),
        ]),
    );
    assert_eq!(
        run("interpolation_shows_any_type", m),
        "s=a\"b shape=Circle(radius: 2.0) xs=[\"x\", \"y\"] opt=Some(\"z\") unit=()\n"
    );
}

#[test]
fn option_from_builtin_matches_user_patterns() {
    // The result of checked_add can be matched with user-written Some / None patterns.
    // match Int.checked_add(a, 1) { Some(n) => n, None => 0 }
    let mut m = Module::new();
    let some = m.some_ctor();
    let none = m.none_ctor();
    let f = m.declare("inc_or_zero", false);
    let a = m.local("a", TypeId::INT);
    let n = m.local("n", TypeId::INT);
    m.define(
        f,
        vec![a],
        TypeId::INT,
        match_(
            b(Builtin::IntCheckedAdd, vec![var(a), int(1)]),
            vec![
                arm(p_ctor(some, vec![Pat::Bind(n)]), var(n)),
                arm(p_ctor(none, vec![]), int(0)),
            ],
        ),
    );
    let opt_id = m.option_enum();
    let opt_int = m.enum_type(opt_id, vec![TypeId::INT]);
    main_fn(
        &mut m,
        TypeId::STRING,
        lines(vec![
            (call(f, vec![int(41)]), TypeId::INT),
            (call(f, vec![int(i32::MAX)]), TypeId::INT),
            // None shares the runtime's value, so structural equality holds too.
            (
                bin(
                    BinOp::Eq,
                    OpTy::Structural,
                    b(Builtin::IntCheckedAdd, vec![int(i32::MAX), int(1)]),
                    ctor_ty(none, vec![TypeId::INT], vec![]),
                ),
                TypeId::BOOL,
            ),
            (ctor_ty(some, vec![TypeId::INT], vec![int(1)]), opt_int),
        ]),
    );
    assert_eq!(run("option_from_builtin", m), "42\n0\nTrue\nSome(1)\n");
}

#[test]
#[should_panic(expected = "is only valid inside an enum show function")]
fn showing_type_param_outside_enum_show_panics() {
    let mut m = Module::new();
    let a = m.type_param(0);
    main_fn(
        &mut m,
        TypeId::STRING,
        Expr::Concat(vec![value_part(int(1), a)]),
    );
    emit_module(&m, &Options::default());
}

/// A const (spec 5.6) is evaluated on the first reference only, and its value is remembered.
/// A defect while evaluating names the const and is remembered too; a reference to a const while
/// it is being evaluated is a defect.
#[test]
fn constants_are_evaluated_once() {
    // `ANSWER` traces its evaluation with `dbg`; `main` refers to it twice.
    let mut m = Module::new();
    let answer = m.declare("ANSWER", false);
    m.functions[answer].constant = true;
    let ignored = m.local("ignored", TypeId::STRING);
    m.define(
        answer,
        vec![],
        TypeId::INT,
        let_(
            ignored,
            builtin_ty(
                Builtin::Dbg,
                vec![TypeId::STRING],
                vec![string("evaluating ANSWER")],
            ),
            int(21),
        ),
    );
    main_fn(
        &mut m,
        TypeId::INT,
        bin(
            BinOp::Add,
            OpTy::Int,
            call(answer, vec![]),
            call(answer, vec![]),
        ),
    );
    let (stdout, stderr) = run_stderr("constant_evaluated_once", m);
    assert_eq!(stdout, "42\n");
    assert_eq!(stderr, "\"evaluating ANSWER\"\n");

    // `OUTER = INNER + 1` and `INNER = 1 / 0`: each const on the way adds its name.
    let mut m = Module::new();
    let inner = m.declare("INNER", false);
    let outer = m.declare("OUTER", false);
    m.functions[inner].constant = true;
    m.functions[outer].constant = true;
    m.define(
        inner,
        vec![],
        TypeId::INT,
        bin(BinOp::Div, OpTy::Int, int(1), int(0)),
    );
    m.define(
        outer,
        vec![],
        TypeId::INT,
        bin(BinOp::Add, OpTy::Int, call(inner, vec![]), int(1)),
    );
    main_fn(&mut m, TypeId::INT, call(outer, vec![]));
    let r = run_with("constant_defect", &m, RuntimeMode::Import(None));
    assert_eq!(
        r.stdout,
        "defect: while evaluating const OUTER: while evaluating const INNER: division by zero\n",
        "{}",
        r.stderr
    );

    // `SELF = f()` where `f` refers to `SELF`.
    let mut m = Module::new();
    let this = m.declare("SELF", false);
    let f = m.declare("f", false);
    m.functions[this].constant = true;
    m.define(this, vec![], TypeId::INT, call(f, vec![]));
    m.define(f, vec![], TypeId::INT, call(this, vec![]));
    main_fn(&mut m, TypeId::INT, call(this, vec![]));
    let r = run_with("constant_self_reference", &m, RuntimeMode::Import(None));
    assert_eq!(
        r.stdout, "defect: const SELF refers to itself while being evaluated\n",
        "{}",
        r.stderr
    );

    // A self-reference in a conditional arm is excluded from E0231, but must still pass through
    // the runtime const guard rather than being loopified into an infinite loop.
    let mut m = Module::new();
    let this = m.declare("A", false);
    m.functions[this].constant = true;
    m.define(
        this,
        vec![],
        TypeId::INT,
        Expr::If {
            cond: Box::new(Expr::Lit(Lit::Bool(true))),
            then: Box::new(call(this, vec![])),
            else_: Box::new(int(0)),
        },
    );
    main_fn(&mut m, TypeId::INT, call(this, vec![]));
    let r = run_with(
        "constant_self_reference_in_if",
        &m,
        RuntimeMode::Import(None),
    );
    assert_eq!(
        r.stdout, "defect: const A refers to itself while being evaluated\n",
        "{}",
        r.stderr
    );
}
