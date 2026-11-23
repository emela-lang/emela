//! Handlers with type parameters (`[effect.handler]`, `[effect.with]`, `[effect.layer]`,
//! `[effect.label]`, `[type.rules]`). Checks where the type arguments of an item are inferred
//! from, that they are written where they cannot be inferred (E0459), and the errors of the
//! declaration (E0424, E0460, E0462, E0301) and of the item (E0304, E0326).
//!
//! The output shows declared types, local variable types, handler dependencies, the type
//! arguments of each item of a `with` and of a layer, diagnostics of `main.emel`, and expressions
//! with a non-empty R.

use crate::common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule, ExprKind};
use emela_types::{CheckOptions, check_program_with};

use common::{describe, resolve_main};

fn render(src: &str) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let in_main =
        |def: emela_resolve::hir::DefId| program.defs[def].module == DefModule::Source(module);
    let types = check_program_with(&program, &CheckOptions::default());
    let cons = &types.ty_table.cons;

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if !in_main(def) || data.kind != DefKind::Fn {
            continue;
        }
        match types.sigs.get(def) {
            Some(sig) => writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap(),
            None => writeln!(out, "{}: <no type>", data.name).unwrap(),
        }
        for (local, ty) in types.locals.iter() {
            if types.local_owner.get(local) == Some(&def) {
                let name = &program.locals[local].name;
                writeln!(out, "  {name}: {}", ty.display(cons)).unwrap();
            }
        }
    }
    for (def, row) in types.handler_uses.iter() {
        if in_main(def) {
            let name = &program.defs[def].name;
            writeln!(
                out,
                "handler {name}: use {}",
                row.display_effects(cons, &[])
            )
            .unwrap();
        }
    }
    let show = |args: &[emela_types::Ty]| -> String {
        let args: Vec<String> = args.iter().map(|t| t.display(cons).to_string()).collect();
        args.join(", ")
    };
    let mut withs: Vec<_> = types
        .with_ty_args
        .iter()
        .filter(|(expr, _)| program.exprs[*expr].span.module == module)
        .collect();
    withs.sort_by_key(|(expr, _)| program.exprs[*expr].span.range.start());
    for (expr, items) in withs {
        let ExprKind::With { handlers, .. } = &program.exprs[expr].kind else {
            continue;
        };
        for (item, args) in handlers.iter().zip(items) {
            if args.is_empty() {
                continue;
            }
            let range = item.span.range;
            let text = &src[usize::from(range.start())..usize::from(range.end())];
            writeln!(out, "item {text}: [{}]", show(args)).unwrap();
        }
    }
    for (layer, members) in types.layer_ty_args.iter() {
        if !in_main(layer) {
            continue;
        }
        for (i, args) in members.iter().enumerate() {
            if !args.is_empty() {
                let name = &program.defs[layer].name;
                writeln!(out, "layer {name} #{i}: [{}]", show(args)).unwrap();
            }
        }
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    let mut diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| d.span.is_some_and(|s| s.module == module))
        .collect();
    diags.sort_by_key(|d| d.span.map(|s| s.range.start()));
    for d in diags {
        writeln!(out, "{}", describe(src, cons, d)).unwrap();
    }

    let mut rows: Vec<_> = types
        .effects
        .iter()
        .filter(|(_, row)| !row.is_empty())
        .filter_map(|(expr, row)| {
            let span = program.exprs[expr].span;
            (span.module == module).then_some((span.range, row))
        })
        .collect();
    rows.sort_by_key(|(range, _)| (range.start(), std::cmp::Reverse(range.end())));
    out.push_str("--- R\n");
    for (range, row) in rows {
        let text = &src[usize::from(range.start())..usize::from(range.end())];
        let text = text.replace('\n', " ");
        writeln!(out, "{text}: {}", row.display_effects(cons, &[])).unwrap();
    }
    out
}

const REPO: &str = r#"
import Std.State.{State}

type UserId(n: Int) derive Eq
type User(name: String)

effect Repo[K, V] {
  fn find(key: K) -> Option[V]
  fn save(key: K, value: V) -> ()
}

handler InMemoryRepo[K: Eq, V]: Repo[K, V] {
  fn find(key) {
    st <- State[List[(K, V)]]
    List.find(st.get(), |(k, _)| k == key) |> Option.map(|(_, v)| v)
  }

  fn save(key, value) {
    st <- State[List[(K, V)]]
    st.update(|xs| [(key, value), ..xs])
  }
}

handler Constant[S](value: S): State[S] {
  init(value: S) { Constant(value:) }

  fn get() { self.value }
  fn set(value) { () }
  fn update(f) { () }
}

handler Pass[S]: State[S] {
  fn get() { (use State[S]).get() }
  fn set(value) { (use State[S]).set(value) }
  fn update(f) { (use State[S]).update(f) }
}
"#;

fn repo(rest: &str) -> String {
    render(&format!("{REPO}{rest}"))
}

/// The declaration (`[effect.handler]`): the type parameters are visible in the fields, the
/// effects, init and the operation bodies; a bound's functions can be called; `self` has the type
/// of the handler applied to them; the requirements keep them.
#[test]
fn declaration() {
    insta::assert_snapshot!(repo(""));
}

/// The type arguments of an item are inferred from the arguments of init, from the R of the
/// body, from the requirements of the other items of the same `with`, from how `x` is used, and
/// from what the outside provides; or written (`[effect.with]`). `provides(H)` and `requires(H)`
/// are instantiated (`[type.rules]`).
#[test]
fn items_of_with() {
    insta::assert_snapshot!(repo(
        r#"
fn count() -> Int uses { State[Int] } { (use State[Int]).get() }

fn names() -> List[String] uses { State[List[String]] } { (use State[List[String]]).get() }

fn repo_test() -> Option[User] uses { Repo[UserId, User] } {
  (use Repo[UserId, User]).find(UserId(n: 1))
}

handler UserService: Logger {
  fn info(msg) {
    _ = (use Repo[UserId, User]).find(UserId(n: 1))
    ()
  }
}

effect Logger {
  fn info(msg: String) -> ()
}

fn log() -> () uses { Logger } { (use Logger).info("a") }

fn from_init() { with Constant(value: 0) { count() } }

fn from_body() { State.run([], || with InMemoryRepo { repo_test() }) }

fn written() { State.run([], || with repo = InMemoryRepo[UserId, User] { repo.find(UserId(n: 1)) }) }

fn from_other_item() { State.run([], || with InMemoryRepo, UserService { log() }) }

fn from_binding() -> String { with x = Constant(value: "a") { x.get() } }

fn from_use() -> Int { with x = Pass { x.get() + 1 } }

fn from_outside() -> (Int, List[String]) { State.run(["a"], || with Pass { names() }) |> |(_, s)| (0, s) }

fn written_with_args() { with Constant[Int](value: 0) { count() } }
"#
    ));
}

/// A type argument left as a variable that appears in the type of the enclosing declaration is
/// generalized with it (`[effect.with]`).
#[test]
fn generalized() {
    insta::assert_snapshot!(repo(
        r#"
fn peek() { with x = Pass { x.get() } }

fn use_peek() -> Int uses { State[Int] } { peek() }
"#
    ));
}

/// Type arguments that are not determined are E0459: nothing determines them, two effects of
/// the body fit, or the item may override another item of the same `with`.
#[test]
fn not_determined() {
    insta::assert_snapshot!(repo(
        r#"
handler Fixed: State[Int] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

handler Empty[A]: State[List[A]] {
  fn get() { [] }
  fn set(value) { () }
  fn update(f) { () }
}

fn ints() -> List[Int] uses { State[List[Int]] } { (use State[List[Int]]).get() }

fn strings() -> List[String] uses { State[List[String]] } { (use State[List[String]]).get() }

handler Blank[A](xs: List[A]): Logger {
  init() { Blank(xs: []) }
  fn info(msg) { () }
}

effect Logger {
  fn info(msg: String) -> ()
}

fn nothing() { with Pass { () } }

fn unknown() { with Blank { () } }

fn declared() -> () uses { State[Int] } { with Pass { () } }

fn two_fit() { with Empty { (ints(), strings()) } }

fn may_override() { with Fixed, Pass { (use State[Int]).get() } }

fn written_override() { with Fixed, Pass[Int] { (use State[Int]).get() } }

fn init_narrows() { with Constant(value: [1]) { (ints(), strings()) } }
"#
    ));
}

/// The items of a layer determine their type arguments from the arguments of init, or write
/// them (`[effect.layer]`); otherwise E0459.
#[test]
fn layers() {
    insta::assert_snapshot!(repo(
        r#"
layer AppTest {
  InMemoryRepo[UserId, User]
  Constant(value: 0)
}

layer Missing {
  InMemoryRepo
}

fn count() -> Int uses { State[Int] } { (use State[Int]).get() }

fn main() { State.run([], || with AppTest { count() }) }
"#
    ));
}

/// The right of `by` (`[effect.label]`): type arguments can be written, and are inferred from the
/// arguments of init and from the underlying effect of the label.
#[test]
fn labels() {
    insta::assert_snapshot!(repo(
        r#"
effect Hits of State[Int]

effect Names of State[List[String]]

fn hits() -> Int uses { Hits } { (use Hits).get() }

fn names() -> List[String] uses { Names } { (use Names).get() }

fn from_label() { State.run(0, || with Hits by Pass { hits() }) }

fn written() { with Hits by Constant[Int](value: 0) { hits() } }

fn from_init() { with Names by Constant(value: ["a"]) { names() } }
"#
    ));
}

/// Errors in the declaration: a type parameter used as a set (E0424), one that appears in none
/// of the effects, fields and init parameters (E0462), type parameters on an inner handler
/// (E0460), and building the value with other type arguments in init (E0301).
#[test]
fn declaration_errors() {
    insta::assert_snapshot!(render(
        r#"
import Std.State.{State}

effect Logger {
  fn info(msg: String) -> ()
}

effect ReadFile {
  fn read() -> String
}

effect Files {
  fn exists(path: String) -> Bool
  handler Open(path: String): ReadFile
}

handler AsSet[E]: State[fn() -> () fails E] {
  fn get() { || () }
  fn set(value) { () }
  fn update(f) { () }
}

handler Recording[A]: Logger {
  fn info(msg) {
    st <- State[List[A]]
    ()
  }
}

handler Nowhere[A]: Logger {
  fn info(msg) { () }
}

handler InInit[A]: Logger {
  init(x: A) { InInit }
  fn info(msg) { () }
}

handler Memory[S](value: S): State[S] {
  init(value: S) { Memory(value: 1) }
  fn get() { self.value }
  fn set(value) { () }
  fn update(f) { () }
}

handler Outer[S](value: S): Files {
  init(value: S) { Outer(value:) }
  fn exists(path) { True }
  handler Open[T](path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read() { self.path }
  }
}

handler Sees[S](value: S): Files {
  init(value: S) { Sees(value:) }
  fn exists(path) { True }
  handler Open(path: String): ReadFile {
    init(path: String) {
      none: Option[S] = None
      Self(path:)
    }
    fn read() { self.path }
  }
}

handler Generic: State {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}
"#
    ));
}

/// Errors at an item: type arguments on a handler without type parameters, or a wrong number of
/// them (E0304), and unmet bounds (E0326).
#[test]
fn item_errors() {
    insta::assert_snapshot!(repo(
        r#"
handler Fixed: State[Int] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

type Opaque(n: Int)

fn not_generic() { with Fixed[Int] { () } }

fn too_many() { with Pass[Int, String] { (use State[Int]).get() } }

fn unbound() { State.run([], || with InMemoryRepo[Opaque, User] { () }) }

fn mismatch() { with Constant[Int](value: "a") { () } }
"#
    ));
}
