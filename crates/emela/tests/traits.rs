//! End-to-end tests for traits (spec 0020) and the Core Prelude (spec 0021):
//! trait/impl declarations, dispatch from argument types (bare and receiver
//! syntax), operator/`to_string` resolution through the embedded prelude,
//! bounded-generic monomorphization, coherence, and the diagnostic cases.
//!
//! Most cases use enum and built-in types, plus a few `record` targets (spec
//! 0006/0028) for the orphan rule; none need packages: the Core Prelude is
//! embedded, so a single file resolves the operator traits and `Show` with no
//! import.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-traits-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], source: &str) -> std::process::Output {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
    for arg in args {
        command.arg(arg);
    }
    let output = command.arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn check_ok(source: &str) {
    let output = run(&["check"], source);
    assert!(
        output.status.success(),
        "expected check to pass, but it failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn check_err(source: &str) -> String {
    let output = run(&["check"], source);
    assert!(
        !output.status.success(),
        "expected check to fail, but it passed"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn ir(source: &str) -> String {
    let output = run(&["ir"], source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn js(source: &str) -> String {
    let output = run(&["build", "--backend", "js-node"], source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn build_wasm_ok(source: &str) {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    let output_path = dir.join("out.wasm");
    fs::write(&input, source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output_path)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output_path).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(&bytes[0..4], b"\0asm");
}

// `Show` is a Core Prelude trait (spec 0021); user code implements it for its
// own types rather than redeclaring it.
const SHOW_COLOR: &str = "\
enum Color {
    Red
    Green
    Blue
}
impl Show for Color {
    fn to_string(c: Color) -> String uses {} {
        match c {
            Red -> \"red\"
            Green -> \"green\"
            Blue -> \"blue\"
        }
    }
}
";

#[test]
fn dispatches_enum_impl_by_argument_type() {
    let dump = ir(&format!(
        "{SHOW_COLOR}fn main() -> Int uses {{}} {{\n  let s: String = to_string(Color::Red)\n  0\n}}\n"
    ));
    assert!(
        dump.contains("Show__Color__to_string"),
        "missing the dispatched impl method:\n{dump}"
    );
    // Traits are erased: no type variable or `Self` leaks into the IR (spec 0012).
    assert!(!dump.contains("Var("), "a type variable leaked:\n{dump}");
    assert!(!dump.contains("Self"), "`Self` leaked into the IR:\n{dump}");
}

// `Add` is a Core Prelude trait (spec 0021); user code implements it, it does
// not redeclare it. Implementing it for `Money` makes `+` work on `Money`.
const ADD_MONEY: &str = "\
enum Money {
    Cents(Int)
}
impl Add for Money {
    fn add(a: Money, b: Money) -> Money uses {} {
        match a {
            Cents(x) -> match b {
                Cents(y) -> Money::Cents(x + y)
            }
        }
    }
}
";

#[test]
fn desugars_operator_on_user_type() {
    let source = format!(
        "{ADD_MONEY}fn combine(a: Money, b: Money) -> Money {{ a + b }}\nfn main() -> Int uses {{}} {{ 0 }}\n"
    );
    let dump = ir(&source);
    assert!(
        dump.contains("Add__Money__add"),
        "`+` on Money should call the impl method:\n{dump}"
    );
    let code = js(&source);
    assert!(
        code.contains("Add__Money__add"),
        "the mangled call should reach JS:\n{code}"
    );
}

#[test]
fn operator_impl_body_bottoms_out_in_intrinsic() {
    // Inside `impl Add for Money`, `x + y` on Int dispatches to the prelude's
    // `Add for Int`, whose body is the `i32_add` intrinsic — not a recursion.
    let dump = ir(&format!(
        "{ADD_MONEY}fn combine(a: Money, b: Money) -> Money {{ a + b }}\nfn main() -> Int uses {{}} {{ 0 }}\n"
    ));
    assert!(
        dump.contains("Add__Int__add"),
        "the inner Int add should dispatch to the prelude impl:\n{dump}"
    );
    assert!(
        dump.contains("i32_add"),
        "the Int add impl should call the intrinsic:\n{dump}"
    );
}

#[test]
fn monomorphizes_bounded_generic() {
    // A `T: Add` generic used at a user type specializes to that type's impl.
    let dump = ir(&format!(
        "{ADD_MONEY}fn sum2<T: Add>(a: T, b: T) -> T {{ a + b }}\n\
         fn main() -> Int uses {{}} {{\n  let m: Money = sum2(Money::Cents(1), Money::Cents(2))\n  0\n}}\n"
    ));
    assert!(
        dump.contains("sum2__Money"),
        "missing the Money specialization:\n{dump}"
    );
    assert!(
        dump.contains("Add__Money__add"),
        "the specialization should dispatch to Money's `add`:\n{dump}"
    );
    assert!(!dump.contains("Var("), "a type variable leaked:\n{dump}");
}

#[test]
fn nested_dispatch_specializes_transitively() {
    let source = "\
enum Color {
    Red
    Green
}
enum Wrap {
    W(Color)
}
impl Show for Color {
    fn to_string(c: Color) -> String uses {} {
        match c {
            Red -> \"r\"
            Green -> \"g\"
        }
    }
}
impl Show for Wrap {
    fn to_string(w: Wrap) -> String uses {} {
        match w {
            W(c) -> to_string(c)
        }
    }
}
fn describe(w: Wrap) -> String { to_string(w) }
fn main() -> Int uses {} { 0 }
";
    let dump = ir(source);
    assert!(
        dump.contains("Show__Wrap__to_string"),
        "missing the outer impl:\n{dump}"
    );
    assert!(
        dump.contains("Show__Color__to_string"),
        "the outer impl should pull in the inner one:\n{dump}"
    );
}

#[test]
fn parameterized_instance_discharges_element_bound() {
    // `impl<T: Describe> Describe for Array<T>` applies to `Array<Color>` because
    // `Color: Describe`; the element bound is discharged during dispatch. A local
    // trait is used because implementing a Core Prelude trait for the built-in
    // `Array` would be an orphan (spec 0020).
    let source = "\
trait Describe {
    fn describe(value: Self) -> String uses {}
}
enum Color {
    Red
}
impl Describe for Color {
    fn describe(c: Color) -> String uses {} { \"red\" }
}
impl<T: Describe> Describe for Array<T> {
    fn describe(xs: Array<T>) -> String uses {} { \"[..]\" }
}
fn describe_all(xs: Array<Color>) -> String { describe(xs) }
fn main() -> Int uses {} { 0 }
";
    check_ok(source);
    let dump = ir(source);
    assert!(
        dump.contains("Describe__Array_Color_"),
        "missing the Array<Color> specialization:\n{dump}"
    );
}

#[test]
fn implements_trait_for_record() {
    // A `record` is a valid impl target (spec 0020's `impl Add for Vec2`), owned
    // by its declaring module for the orphan rule just like an enum. Records and
    // enums share the nominal `Type::Enum` representation but live in separate
    // tables, so the orphan check must consult both.
    let source = "\
record Point {
    x: Int
    y: Int
}
impl Show for Point {
    fn to_string(p: Point) -> String uses {} {
        \"(\" ++ to_string(p.x) ++ \", \" ++ to_string(p.y) ++ \")\"
    }
}
fn show_it(p: Point) -> String { to_string(p) }
fn main() -> Int uses {} { 0 }
";
    check_ok(source);
    let dump = ir(source);
    assert!(
        dump.contains("Show__Point__to_string"),
        "missing the record impl method:\n{dump}"
    );
}

#[test]
fn parameterized_instance_over_generic_record() {
    // `impl<T: Show> Show for Pair<T>` on a user-defined generic record (spec
    // 0028) discharges the element bound during dispatch, mirroring the built-in
    // `Array<T>` case: `Pair<Int>` specializes because `Int: Show`.
    let source = "\
record Pair<T> {
    first: T
    second: T
}
impl<T: Show> Show for Pair<T> {
    fn to_string(p: Pair<T>) -> String uses {} {
        \"(\" ++ to_string(p.first) ++ \", \" ++ to_string(p.second) ++ \")\"
    }
}
fn show_it(p: Pair<Int>) -> String { to_string(p) }
fn main() -> Int uses {} { 0 }
";
    check_ok(source);
    let dump = ir(source);
    assert!(
        dump.contains("Show__Pair_Int_"),
        "missing the Pair<Int> specialization:\n{dump}"
    );
}

#[test]
fn builds_trait_program_to_wasm() {
    build_wasm_ok(&format!(
        "{ADD_MONEY}fn combine(a: Money, b: Money) -> Money {{ a + b }}\nfn main() -> Int uses {{}} {{ 0 }}\n"
    ));
}

#[test]
fn resolves_ambiguous_method_by_qualification() {
    // Two traits declare `name`; a bare call is ambiguous, `Trait.name` resolves.
    let ambiguous = "\
trait A { fn name(x: Self) -> Int uses {} }
trait B { fn name(x: Self) -> Int uses {} }
enum E { V }
impl A for E { fn name(x: E) -> Int uses {} { 1 } }
impl B for E { fn name(x: E) -> Int uses {} { 2 } }
fn use_it(e: E) -> Int { name(e) }
fn main() -> Int uses {} { 0 }
";
    let diagnostics = check_err(ambiguous);
    assert!(
        diagnostics.contains("Ambiguous"),
        "expected an ambiguity error:\n{diagnostics}"
    );
    let qualified = ambiguous.replace("{ name(e) }", "{ A.name(e) }");
    check_ok(&qualified);
}

#[test]
fn rejects_duplicate_impl() {
    // `Show` is the Core Prelude trait; two impls for the same type still clash.
    let diagnostics = check_err(
        "enum Color { Red }\n\
         impl Show for Color { fn to_string(c: Color) -> String uses {} { \"a\" } }\n\
         impl Show for Color { fn to_string(c: Color) -> String uses {} { \"b\" } }\n\
         fn main() -> Int uses {} { 0 }\n",
    );
    assert!(
        diagnostics.contains("Conflicting implementations"),
        "{diagnostics}"
    );
}

#[test]
fn rejects_incomplete_impl() {
    let diagnostics = check_err(
        "trait Two {\n  fn a(x: Self) -> Int uses {}\n  fn b(x: Self) -> Int uses {}\n}\n\
         enum E { V }\n\
         impl Two for E { fn a(x: E) -> Int uses {} { 0 } }\n\
         fn main() -> Int uses {} { 0 }\n",
    );
    assert!(diagnostics.contains("Incomplete impl"), "{diagnostics}");
}

#[test]
fn rejects_extra_impl_method() {
    let diagnostics = check_err(
        "trait One { fn a(x: Self) -> Int uses {} }\n\
         enum E { V }\n\
         impl One for E {\n  fn a(x: E) -> Int uses {} { 0 }\n  fn b(x: E) -> Int uses {} { 1 }\n}\n\
         fn main() -> Int uses {} { 0 }\n",
    );
    assert!(diagnostics.contains("not a method"), "{diagnostics}");
}

#[test]
fn allows_self_only_in_return() {
    // Return dispatch (spec 0047): a method whose `Self` is only in the return
    // type (like `empty`) is now allowed; its impl comes from the expected type.
    check_ok("trait Zeroish { fn make() -> Self uses {} }\nfn main() -> Int uses {} { 0 }\n");
}

#[test]
fn monoid_empty_resolves_from_return_type() {
    // Return dispatch (spec 0047): `empty()` in the `Nil` arm resolves its impl
    // from the function's return type `String`, and `combine` dispatches on its
    // argument. This is the `concat`-over-a-list shape the stdlib uses.
    let dump = ir("enum L { Nil\n Cons(String, L) }\n\
         fn cat(xs: L) -> String uses {} {\n\
             match xs {\n\
                 Nil -> empty()\n\
                 Cons(h, t) -> combine(h, cat(t))\n\
             }\n\
         }\n\
         fn main() -> String uses {} { cat(L::Cons(\"a\", L::Cons(\"b\", L::Nil))) }\n");
    assert!(
        dump.contains("Monoid__String__empty"),
        "expected `empty()` to resolve to the String impl:\n{dump}"
    );
    assert!(
        dump.contains("string_concat"),
        "expected `combine` to reach string_concat:\n{dump}"
    );
}

#[test]
fn monoid_empty_resolves_from_annotation() {
    // With no argument to dispatch on, an annotation supplies the expected type
    // (spec 0047): `let e: String = empty()` picks the String impl.
    check_ok(
        "fn main() -> String uses {} {\n\
             let e: String = empty()\n\
             combine(e, \"hi\")\n\
         }\n",
    );
}

#[test]
fn rejects_empty_without_expected_type() {
    // No argument and no expected type: `Self` cannot be resolved (spec 0047).
    let diagnostics = check_err(
        "fn main() -> Int uses {} {\n\
             let e = empty()\n\
             0\n\
         }\n",
    );
    assert!(diagnostics.contains("Self"), "{diagnostics}");
}

#[test]
fn rejects_self_nowhere() {
    // A method that mentions `Self` nowhere is still undispatchable (spec 0047).
    let diagnostics =
        check_err("trait Bad { fn make() -> Int uses {} }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        diagnostics.contains("Undispatchable") || diagnostics.contains("Self"),
        "{diagnostics}"
    );
}

#[test]
fn prelude_operators_need_no_import() {
    // The Core Prelude (spec 0021) is embedded, so operators resolve to their
    // built-in impls with no `--package` and no import.
    let dump = ir("fn main() -> Int uses {} { 1 + 2 * 3 }\n");
    assert!(
        dump.contains("i32_add"),
        "expected the add intrinsic:\n{dump}"
    );
    assert!(
        dump.contains("i32_mul"),
        "expected the mul intrinsic:\n{dump}"
    );
}

#[test]
fn prelude_string_concat_is_an_intrinsic() {
    // `++` desugars to `Concat for String`, whose body is `string_concat`.
    let dump = ir("fn main() -> String uses {} { \"a\" ++ \"b\" }\n");
    assert!(
        dump.contains("string_concat"),
        "`++` should reach the string_concat intrinsic:\n{dump}"
    );
}

#[test]
fn method_call_syntax_desugars_to_dispatch() {
    // `x.method(args)` on a value is sugar for `method(x, args)` (spec 0020):
    // `n.to_string()` dispatches Show, and a user method takes extra arguments.
    let dump = ir("enum V { N(Int) }\n\
         trait Bump { fn bump(v: Self, by: Int) -> Int uses {} }\n\
         impl Bump for V { fn bump(v: V, by: Int) -> Int uses {} { match v { N(x) -> x + by } } }\n\
         fn go(v: V) -> String uses {} {\n  let raised = v.bump(5)\n  raised.to_string()\n}\n\
         fn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("Bump__V__bump"),
        "`v.bump(5)` should dispatch to the V impl:\n{dump}"
    );
    assert!(
        dump.contains("Show__Int__to_string"),
        "`raised.to_string()` should dispatch Show for Int:\n{dump}"
    );
}

#[test]
fn prelude_show_gives_to_string_without_import() {
    // `to_string` is a Core Prelude method (spec 0021), so it resolves on an
    // `Int` with no import and lowers to the migrated `Show for Int` impl.
    let dump =
        ir("fn describe() -> String uses {} { to_string(42) }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("Show__Int__to_string"),
        "`to_string(42)` should dispatch to the prelude's Show impl:\n{dump}"
    );
    // A user type can implement the same Show trait and dispatch is by argument.
    check_ok(
        "enum C { A }\nimpl Show for C { fn to_string(c: C) -> String uses {} { \"a\" } }\n\
         fn go(c: C) -> String { to_string(c) }\nfn main() -> Int uses {} { 0 }\n",
    );
}

#[test]
fn rejects_add_on_string() {
    // The prelude gives `Add` only to Int/Float, so `+` on String is an error —
    // preserving the operator typing of spec 0016/0017.
    let diagnostics = check_err("fn main() -> String uses {} { \"a\" + \"b\" }\n");
    assert!(diagnostics.contains("does not satisfy"), "{diagnostics}");
}

#[test]
fn rejects_rem_on_float() {
    // `%` is Int-only (spec 0016): the prelude has no `Rem for Float`.
    let diagnostics = check_err("fn main() -> Float uses {} { 2.0 % 1.0 }\n");
    assert!(diagnostics.contains("does not satisfy"), "{diagnostics}");
}

#[test]
fn rejects_unsatisfied_bound() {
    // `Bool` has no `Add` impl (the prelude provides Add only for Int/Float), so
    // a `T: Add` generic cannot be used at `Bool`.
    let diagnostics = check_err(
        "fn sum2<T: Add>(a: T, b: T) -> T { a + b }\n\
         fn main() -> Int uses {} {\n  let x: Bool = sum2(true, false)\n  0\n}\n",
    );
    assert!(diagnostics.contains("does not satisfy"), "{diagnostics}");
}
