//! Traits, impls and derive (spec 10.1, 10.2, 10.5, 10.6).

mod common;

use common::render_diagnostics;

/// The supertraits of an impl are checked at its head (E0326), with the bounds of the impl.
#[test]
fn an_impl_needs_its_supertraits() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
trait Named: Show {
  fn name(self) -> String
}
type User(id: Int)
type Admin(id: Int)
  derive Show
impl User: Named {
  fn name(self) { "" }
}
impl Admin: Named {
  fn name(self) { "" }
}
"##
    ));
}

/// Following supertraits back to the same trait is E0329, once per cycle, at the trait defined
/// first, listing the cycle from it.
#[test]
fn supertrait_cycles() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
trait Aa: Bb {}
trait Bb: Cc {}
trait Cc: Aa {}
trait Dd: Dd {}
trait Ee: Aa {}
"##
    ));
}

/// An impl of a trait for a type is written in the module of one of them (E0331); one impl per
/// pair (E0330), and an impl fn has the trait fn's number of parameters (E0332).
#[test]
fn where_impls_are_written() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
type User(id: Int)
trait Named {
  fn name(self) -> String
}
impl Int: Show {
  fn show(self) { "" }
}
impl Int: Named {
  fn name(self) { "" }
}
impl User: Show {
  fn show(self) { "" }
}
impl User: Show {
  fn show(self) { "" }
}
impl User: Named {
  fn name(self, extra) { "" }
}
"##
    ));
}

/// Only Eq, Ord and Show are derived; a field whose type does not satisfy the trait makes it
/// E0326, combined per type; `derive Ord` needs Eq; a derive and an impl of the same pair, or the
/// same trait twice, are E0330. Float has no Ord, and functions satisfy none of them.
#[test]
fn derive_rules() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
type Plain(id: Int)
type HasFloat(x: Float, y: Float)
  derive Eq, Ord, Show
type HasFn(f: fn() -> Int)
  derive Eq, Show
type NoEq(id: Int)
  derive Ord
type Twice(id: Int)
  derive Show, Show
type Both(id: Int)
  derive Show
impl Both: Show {
  fn show(self) { "" }
}
type Custom(id: Int)
  derive Plainly
type HasPlain(p: Plain, xs: List[Plain], t: (Int, Plain))
  derive Show
enum Shape {
  Dot
  Circle(r: Float)
  Unit(u: ())
} derive Eq, Ord
error Down(code: Int) derive Eq
pub type Uses(id: Int)
  derive Eq

trait Plainly {}

fn use_all(a: HasFloat, d: Shape, e: Down) -> Bool {
  a == a && d == d && d < d && e == e
}
"##
    ));
}

/// A derived implementation of a non-pub type that nothing requires is W0303; a pub type, a
/// supertrait of a derived trait, and the Show of an error are not warned about.
#[test]
fn unused_derives() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
type Unused(id: Int)
  derive Eq, Show
pub type Public(id: Int)
  derive Eq
type Super(id: Int)
  derive Eq, Ord
type Compared(id: Int)
  derive Eq
type Shown(id: Int)
  derive Show
error Failed(code: Int) derive Show
type Inner(id: Int)
  derive Eq
type Outer(inner: Inner)
  derive Eq

fn f(a: Super, b: Compared, c: Shown, o: Outer) -> Bool {
  s = "#{c}"
  a < a && b == b && o == o
}
"##
    ));
}

/// Whether a type satisfies a trait goes through its type arguments, the elements of lists and
/// tuples, and the bounds of type parameters, where a bound gives its supertraits (`[A: Ord]`
/// allows `==`, `[A: Show]` does not).
#[test]
fn what_satisfies_a_trait() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
type Box[A](value: A)
  derive Eq
error Bad(f: fn() -> Int) derive Eq

fn bounds[A: Ord, B: Show](a: A, b: B) -> Bool { a == a && b == b }
fn args(f: fn() -> Int) -> Bool {
  [f] == [f] && (1, f) == (1, f) && Box(value: f) == Box(value: f) && Box(value: 1) == Box(value: 1)
}
fn unit() -> Bool { () == () }
"##
    ));
}

/// A recursive type's derivation that requires itself is not a use of it (W0303), and a Prelude
/// trait that is not Eq, Ord or Show is not derivable (E0326).
#[test]
fn recursive_derives_and_non_derivable_prelude_traits() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
type Tree(kids: List[Tree])
  derive Eq
type Fast(id: Int)
  derive Immediate
"##
    ));
}

/// The impls and derivations of a trait, in definition order.
#[test]
fn impls_and_derivations_of_a_trait() {
    use emela_resolve::hir::DefKind;
    let (program, main, _) = common::resolve_main(
        r##"
type Aa(id: Int)
  derive Eq
type Bb(id: Int)
  derive Show
type Cc(id: Int)
impl Cc: Eq {
  fn eq(self, other) { True }
}
impl Bb: Eq {
  fn eq(self, other) { True }
}
"##,
    );
    let types = emela_types::check_program(&program);
    let named = |name: &str, kind| {
        program
            .defs
            .iter()
            .find(|(_, d)| {
                d.name == name
                    && d.kind == kind
                    && d.module == emela_resolve::hir::DefModule::Source(main)
            })
            .map(|(id, _)| id)
            .unwrap()
    };
    let eq = program.prelude_def("Eq").unwrap();
    let show = program.prelude_def("Show").unwrap();
    let impl_tys: Vec<String> = types
        .traits
        .impls_of(eq)
        .iter()
        .filter(|&&i| program.defs[i].module == emela_resolve::hir::DefModule::Source(main))
        .map(|&i| {
            program.defs[types.traits.head(i).unwrap().ty_def]
                .name
                .to_string()
        })
        .collect();
    assert_eq!(impl_tys, ["Cc", "Bb"]);
    // The core's impls of Show follow; none is in `main.emel`.
    assert!(
        types
            .traits
            .impls_of(show)
            .iter()
            .all(|&i| program.defs[i].module != emela_resolve::hir::DefModule::Source(main))
    );
    let own = |defs: Vec<emela_resolve::hir::DefId>| -> Vec<_> {
        defs.into_iter()
            .filter(|&d| program.defs[d].module == emela_resolve::hir::DefModule::Source(main))
            .collect()
    };
    assert_eq!(
        own(types.traits.derived_of(eq)),
        [named("Aa", DefKind::Type)]
    );
    assert_eq!(
        own(types.traits.derived_of(show)),
        [named("Bb", DefKind::Type)]
    );
}

/// A trait fn's own type parameters come after the impl's in the impl fn's type.
#[test]
fn impl_fns_with_type_parameters_of_their_own() {
    insta::assert_snapshot!(common::render_bodies(
        r##"
trait Pairs {
  fn pair[B, C](self, b: B, c: C) -> (Self, B, C)
}
type Box[A](value: A)
impl Box[A]: Pairs {
  fn pair(self, b, c) { (self, b, c) }
}

fn use_it() {
  p = Box(value: 1).pair("x", True)
}
"##
    ));
}

/// An impl fn is checked against the trait fn with the impl's type parameters first and the trait
/// fn's own after them, each kept apart.
#[test]
fn an_impl_fn_keeps_the_trait_fns_type_parameters_apart() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
trait Pairs {
  fn pair[B, C](self, b: B, c: C) -> (Self, B, C)
}
type Box[A](value: A)
impl Box[A]: Pairs {
  fn pair(self, b, c) { (self, c, b) }
}
"##
    ));
}

/// The default implementation of a trait fn is checked like any body.
#[test]
fn default_implementations_are_checked() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
trait Sized {
  fn size(self) -> Int { "big" }
}
"##
    ));
}

/// `Self` in a trait is bounded by the trait, so a default implementation can call the trait's
/// other functions on `self`; a trait fn's own set parameters stay apart in an impl.
#[test]
fn self_is_bounded_by_its_trait_and_own_sets_stay_apart() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
trait Named {
  fn name(self) -> String
  fn greet(self) -> String { "hi #{self.name()}" }
  fn run[E, R](self, f: fn() -> Int fails E uses { R }) -> Int fails E uses { R }
  fn run_pure[E, R](self, f: fn() -> Int fails E uses { R }) -> Int fails E uses {}
}
type Box[A](value: A)
impl Box[A]: Named {
  fn name(self) { "box" }
  fn run(self, f) { f() }
  fn run_pure(self, f) { f() }
}
"##
    ));
}

/// A call instantiates the bounds of the callee's type parameters, which the arguments satisfy
/// (E0326); a trait fn whose parameters do not mention `Self` needs `Self` known from the result
/// (E0333), one whose parameters do (even inside a list) does not.
#[test]
fn bounds_at_calls_and_selves_known_from_results() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
trait Parse {
  fn parse(s: String) -> Self
}
trait Merge {
  fn merge(xs: List[Self]) -> Self
}
impl Int: Parse {
  fn parse(s) { 1 }
}
impl Int: Merge {
  fn merge(xs) { 1 }
}
type Plain(id: Int)

fn needs[A: Show](a: A) -> A { a }

fn calls(f: fn() -> Int, p: Plain) {
  a = needs(f)
  b = needs(p)
  c = needs(1)
  d = Parse.parse("1")
  e: Int = Parse.parse("1")
  g: Int = Merge.merge([])
  h = Merge.merge([])
  ()
}
"##
    ));
}

/// An impl fn can name the impl's type parameters, and a default implementation can name `Self`;
/// both are types of their own, apart from Int.
#[test]
fn impl_type_parameters_and_self_in_bodies() {
    insta::assert_snapshot!(render_diagnostics(
        r##"
trait Copy {
  fn copy(self) -> Self {
    me: Self = self
    wrong: Self = 1
    me
  }
  fn first(self) -> Int
}
type Box[A](value: A)
impl Box[A]: Copy {
  fn first(self) {
    v: A = self.value
    wrong: A = 1
    1
  }
}
"##
    ));
}

/// The type of a trait fn bounds `Self` by the trait, and shows it.
#[test]
fn trait_fns_show_self_bounded_by_their_trait() {
    use emela_resolve::hir::{DefKind, DefModule};
    let (program, main, _) = common::resolve_main(
        r##"
trait Named {
  fn name(self) -> String
}
"##,
    );
    let types = emela_types::check_program(&program);
    let name = program
        .defs
        .iter()
        .find(|(_, d)| {
            d.name == "name" && d.kind == DefKind::TraitFn && d.module == DefModule::Source(main)
        })
        .map(|(id, _)| id)
        .unwrap();
    let scheme = &types.sigs.get(name).unwrap().scheme;
    assert_eq!(
        scheme.display(&types.ty_table.cons).to_string(),
        "∀ Self: Named. fn(Self) -> String"
    );
}
