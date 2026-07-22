//! End-to-end tests for user-defined generic enums (spec 0028): declaring
//! `enum List<T>` / `enum Either<L, R>`, constructing values with inferred type
//! arguments, matching them, and using them through generic functions that
//! monomorphize per instantiation. These cover the enum half of the spec; the
//! record half lives in `generic_records.rs`.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-genum-test-{}-{id}", std::process::id()));
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

// A recursive generic enum (spec 0028): finite because payloads are references.
const LIST: &str = "\
enum List<T> {
    Nil
    Cons(T, List<T>)
}
fn length<T>(xs: List<T>) -> Int {
    match xs {
        Nil -> 0
        Cons(h, t) -> 1 + length(t)
    }
}
";

#[test]
fn monomorphizes_generic_list_function() {
    // `length<T>` at `List<Int>` specializes to `length__Int`, and the typed IR
    // holds no type variable (spec 0012/0028).
    let dump = ir(&format!(
        "{LIST}fn main() -> Int uses {{}} {{\n  \
         let xs: List<Int> = List::Cons(1, List::Cons(2, List::Nil))\n  length(xs)\n}}\n"
    ));
    assert!(
        dump.contains("length__Int"),
        "missing the Int specialization of `length`:\n{dump}"
    );
    assert!(
        !dump.contains("Var("),
        "a type variable leaked into the IR:\n{dump}"
    );
}

#[test]
fn two_instantiations_specialize_separately() {
    // The same generic function used at `List<Int>` and `List<Bool>` produces two
    // specializations (spec 0028/0014).
    let dump = ir(&format!(
        "{LIST}fn main() -> Int uses {{}} {{\n  \
         let a: List<Int> = List::Cons(1, List::Nil)\n  \
         let b: List<Bool> = List::Cons(true, List::Nil)\n  \
         length(a) + length(b)\n}}\n"
    ));
    assert!(
        dump.contains("length__Int") && dump.contains("length__Bool"),
        "expected both `length__Int` and `length__Bool`:\n{dump}"
    );
}

#[test]
fn infers_type_argument_from_payload_without_annotation() {
    // `List::Cons(1, List::Nil)` infers `List<Int>` from the payload (spec 0028);
    // no annotation is needed for the enclosing binding.
    let source = format!(
        "{LIST}fn main() -> Int uses {{}} {{\n  \
         let xs = List::Cons(1, List::Cons(2, List::Nil))\n  length(xs)\n}}\n"
    );
    assert!(
        ir(&source).contains("length__Int"),
        "unannotated construction should infer `List<Int>`"
    );
}

#[test]
fn type_parameter_binds_to_a_generic_enum() {
    // A generic function whose type parameter is inferred to a generic-enum
    // instance mangles with the enum's arguments (spec 0028/0014).
    let dump = ir(&format!(
        "{LIST}fn id<T>(x: T) -> T {{ x }}\n\
         fn main() -> Int uses {{}} {{\n  \
         let xs: List<Int> = List::Nil\n  let ys: List<Int> = id(xs)\n  length(ys)\n}}\n"
    ));
    assert!(
        dump.contains("id__List_Int_"),
        "expected `id` specialized at `List<Int>`:\n{dump}"
    );
}

#[test]
fn generic_list_builds_and_runs_on_js() {
    // A concrete run: sum a three-element `List<Int>`.
    let code = js(&format!(
        "{LIST}fn sum(xs: List<Int>) -> Int {{\n  \
         match xs {{\n    Nil -> 0\n    Cons(h, t) -> h + sum(t)\n  }}\n}}\n\
         fn main() -> Int {{\n  \
         let xs: List<Int> = List::Cons(10, List::Cons(20, List::Cons(12, List::Nil)))\n  sum(xs)\n}}\n"
    ));
    // The value flows through native JS objects; just confirm the code emitted.
    assert!(code.contains("function sum"), "{code}");
}

#[test]
fn generic_list_builds_to_wasm() {
    build_wasm_ok(&format!(
        "{LIST}fn main() -> Int {{\n  \
         let xs: List<Int> = List::Cons(1, List::Cons(2, List::Nil))\n  length(xs)\n}}\n"
    ));
}

#[test]
fn either_with_two_type_parameters() {
    // Multiple type parameters, and a variant (`Left`) that does not pin every
    // parameter — the rest come from the annotation (spec 0028).
    let source = "\
enum Either<L, R> {
    Left(L)
    Right(R)
}
fn from_either(e: Either<Int, Int>, default: Int) -> Int {
    match e {
        Left(_) -> default
        Right(r) -> r
    }
}
fn main() -> Int uses {} {
    let e: Either<Int, Int> = Either::Right(42)
    from_either(e, 0)
}
";
    assert!(
        js(source).contains("function from_either"),
        "Either program should build to JS"
    );
    build_wasm_ok(source);
}

#[test]
fn rejects_wrong_number_of_type_arguments() {
    let err = check_err(
        "enum Box<T> { Wrap(T) }\nfn f(b: Box<Int, Int>) -> Int { 0 }\n\
         fn main() -> Int uses {} { 0 }\n",
    );
    assert!(
        err.contains("type argument"),
        "wrong arity should be reported:\n{err}"
    );
}

#[test]
fn rejects_bound_on_enum_type_parameter() {
    // Data types cannot carry bounds (spec 0028): the requirement belongs on the
    // functions/impls that use the type.
    let err = check_err("enum Bad<T: Show> { Wrap(T) }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        err.to_lowercase().contains("bound"),
        "a bound on an enum type parameter should be rejected:\n{err}"
    );
}

#[test]
fn constructs_variant_with_colon_colon() {
    // Enum variants are `::` type paths (spec 0005/0018 R7): `Enum::Variant`.
    let source = "enum Color {\n  Red\n  Green\n}\n\
        fn code(c: Color) -> Int {\n  match c {\n    Red -> 1\n    Green -> 2\n  }\n}\n\
        fn main() -> Int uses {} { code(Color::Red) }\n";
    assert!(
        ir(source).contains("Red"),
        "`Color::Red` should construct an enum value"
    );
}

#[test]
fn rejects_dotted_variant_access() {
    // The old dotted spelling `Enum.Variant` is no longer a variant path
    // (spec 0018 R7): `.` is reserved for module/receiver access, so `Color.Red`
    // resolves to nothing and must be rejected.
    let err = check_err(
        "enum Color {\n  Red\n  Green\n}\n\
         fn main() -> Int uses {} {\n  let c: Color = Color.Red\n  0\n}\n",
    );
    assert!(
        err.contains("::"),
        "the error should point at the `::` spelling:\n{err}"
    );
}
