//! Type checking of traits and impls (spec 10.1–10.3).

use std::path::Path;

use emela_driver::{Analysis, Input, Location, MemoryFiles, Typecheck, check, code, render};

fn checked(files: &[(&str, &str)]) -> Analysis {
    let mut all = vec![("/app/Pome.toml", "")];
    all.extend_from_slice(files);
    let fs = MemoryFiles::new(all);
    let input = Input::resolve(&fs, Path::new("/app")).unwrap();
    check(&fs, &input, &mut Typecheck::default()).0
}

/// The errors and warnings of `main.emel` as `code message @ source text`.
fn diagnostics(main: &str) -> Vec<String> {
    let analysis = checked(&[("/app/src/main.emel", main)]);
    analysis
        .diagnostics
        .iter()
        .map(|d| {
            let at = match d.location {
                Some(Location::Span(span)) => &main[span.range],
                _ => "",
            };
            format!("{} {} @ {at}", d.code.unwrap_or("-"), d.message)
        })
        .collect()
}

/// [`diagnostics`] without warnings (the unused derives of test programs, W0303).
fn errors(main: &str) -> Vec<String> {
    diagnostics(main)
        .into_iter()
        .filter(|d| !d.starts_with('W'))
        .collect()
}

fn assert_clean(files: &[(&str, &str)]) {
    let analysis = checked(files);
    assert!(
        !analysis.has_errors(),
        "{}",
        render(&analysis.diagnostics, &analysis.sources, false)
    );
}

const POINT: &str = "type Point(x: Int, y: Int)\n";

#[test]
fn impl_functions_take_their_types_from_the_trait() {
    let main = format!(
        "{POINT}trait Merge {{\n  fn merge(self, other: Self) -> Self\n  fn size(self, scale: Int) -> Int {{ scale }}\n}}\nimpl Point: Merge {{\n  fn merge(self, other) {{ Point(x: self.x + other.x, y: self.y) }}\n  fn size(self, scale) {{ self.x * scale }}\n}}\n"
    );
    assert_clean(&[("/app/src/main.emel", &main)]);
}

#[test]
fn impl_and_default_bodies_are_type_checked() {
    let main = format!(
        "{POINT}trait Label {{\n  fn label(self) -> String\n  fn twice(self) -> Int {{ Label.label(self) }}\n}}\nimpl Point: Label {{\n  fn label(self) {{ self.x }}\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        [
            "E0301 type mismatch: expected `Int`, found `String` @ Label.label(self)",
            "E0301 type mismatch: expected `String`, found `Int` @ self.x",
        ]
    );
}

#[test]
fn written_parameter_types_must_be_the_traits() {
    let main = format!("{POINT}impl Point: Eq {{\n  fn eq(self, other: Int) {{ True }}\n}}\n");
    assert_eq!(
        diagnostics(&main),
        ["E0301 type mismatch: expected `Point`, found `Int` @ Int"]
    );
}

#[test]
fn impl_functions_take_as_many_parameters_as_the_trait() {
    let main = format!("{POINT}impl Point: Show {{\n  fn show(self, extra) {{ \"p\" }}\n}}\n");
    assert_eq!(
        diagnostics(&main),
        ["E0332 wrong number of parameters: `show` of `Show` takes 1, found 2 @ show"]
    );
}

#[test]
fn impl_errors_and_effects_stay_within_the_traits_declaration() {
    let main = format!(
        "{POINT}error Oops\neffect Log {{\n  fn write(line: String) -> ()\n}}\ntrait Check {{\n  fn check(self) -> Int fails Oops\n  fn quiet(self) -> Int\n}}\nimpl Point: Check {{\n  fn check(self) {{ fail Oops }}\n  fn quiet(self) {{\n    log = use Log\n    log.write(\"x\")\n    fail Oops\n  }}\n}}\n"
    );
    let found = diagnostics(&main);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(found[0].starts_with("E0402 "), "{found:#?}");
    assert!(found[1].starts_with("E0318 "), "{found:#?}");
}

#[test]
fn heads_are_a_type_with_distinct_type_parameters() {
    let main = "type Pair[A, B](first: A, second: B)\ntrait Label { fn label(self) -> String }\nimpl List[Int]: Label {\n  fn label(self) { \"ints\" }\n}\nimpl Pair[A, A]: Label {\n  fn label(self) { \"pair\" }\n}\n";
    assert_eq!(
        diagnostics(main),
        [
            "E0328 impl head must be a type with distinct type parameters: `List[Int]` @ List[Int]",
            "E0328 impl head must be a type with distinct type parameters: `Pair[A, A]` @ Pair[A, A]",
        ]
    );
}

#[test]
fn one_impl_per_type_and_trait() {
    let main = "type Box[A](value: A)\ntrait Label { fn label(self) -> String }\nimpl Box[A]: Label {\n  fn label(self) { \"a\" }\n}\nimpl Box[B: Label]: Label {\n  fn label(self) { \"b\" }\n}\n";
    assert_eq!(
        diagnostics(main),
        ["E0330 `Label` is already implemented for `Box[B]` @ Box[B: Label]"]
    );
}

#[test]
fn impls_belong_with_their_type_or_trait() {
    let label = "pub trait Label { fn label(self) -> String }\nimpl Int: Label {\n  fn label(self) { \"int\" }\n}\n";
    let point = "pub type Point(x: Int)\n";
    let main = "import Labels.{Label}\nimport Points.{Point}\nimpl Point: Label {\n  fn label(self) { \"point\" }\n}\nimpl Int: Show {\n  fn show(self) { \"int\" }\n}\n";
    let analysis = checked(&[
        ("/app/src/main.emel", main),
        ("/app/src/labels.emel", label),
        ("/app/src/points.emel", point),
    ]);
    let found: Vec<(&str, &str)> = analysis
        .diagnostics
        .iter()
        .map(|d| (d.code.unwrap_or("-"), d.message.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (
                code::ORPHAN_IMPL,
                "`Label` for `Point` can only be implemented in the module of `Label` or of `Point`"
            ),
            (
                code::ORPHAN_IMPL,
                "`Show` for `Int` can only be implemented in the module of `Show` or of `Int`"
            ),
        ]
    );
}

#[test]
fn supertraits_cannot_form_a_cycle() {
    let main = "trait Alpha: Beta { fn a(self) -> Int }\ntrait Beta: Gamma { fn b(self) -> Int }\ntrait Gamma: Alpha { fn c(self) -> Int }\ntrait Own: Own { fn o(self) -> Int }\n";
    assert_eq!(
        diagnostics(main),
        [
            "E0329 trait `Alpha` is its own supertrait: `Alpha` -> `Beta` -> `Gamma` -> `Alpha` @ Alpha",
            "E0329 trait `Own` is its own supertrait: `Own` -> `Own` @ Own",
        ]
    );
}

#[test]
fn an_impl_needs_the_supertraits_impls() {
    let main = format!(
        "{POINT}trait Named: Show {{ fn name(self) -> String }}\nimpl Point: Named {{\n  fn name(self) {{ \"p\" }}\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        ["E0326 type `Point` does not implement `Show` @ Point"]
    );
    // A derive counts as the impl it generates.
    let main = "type Point(x: Int) derive Show\ntrait Named: Show { fn name(self) -> String }\nimpl Point: Named {\n  fn name(self) { \"p\" }\n}\n";
    assert_clean(&[("/app/src/main.emel", main)]);
}

#[test]
fn supertrait_constraints_follow_from_the_impls_constraints() {
    // `A: Ord` gives `A: Eq` (spec 10.1).
    let main = "type Pair[A, B](first: A, second: B)\nimpl Pair[A: Eq, B: Eq]: Eq {\n  fn eq(self, other) { True }\n}\nimpl Pair[A: Ord, B: Ord]: Ord {\n  fn compare(self, other) { Equal }\n}\n";
    assert_clean(&[("/app/src/main.emel", main)]);
    let main = "type Pair[A, B](first: A, second: B)\nimpl Pair[A: Eq, B: Eq]: Eq {\n  fn eq(self, other) { True }\n}\nimpl Pair[A: Ord, B]: Ord {\n  fn compare(self, other) { Equal }\n}\n";
    assert_eq!(
        diagnostics(main),
        ["E0326 type `B` does not implement `Eq` @ Pair[A: Ord, B]"]
    );
}

#[test]
fn self_in_trait_signatures_is_the_implementing_type() {
    let main = "type Box[A](value: A)\ntrait Swap {\n  fn swap(self, other: Self) -> (Self, Self)\n}\nimpl Box[A]: Swap {\n  fn swap(self, other) { (other, self) }\n}\nfn use_it(b: Box[Int]) -> (Box[Int], Box[Int]) { Swap.swap(b, b) }\n";
    assert_clean(&[("/app/src/main.emel", main)]);
}

const USER: &str = "type User(name: String)\nimpl User: Show {\n  fn show(self) { self.name }\n}\nimpl User: Eq {\n  fn eq(self, other) { True }\n}\nimpl User: Ord {\n  fn compare(self, other) { Equal }\n}\n";

#[test]
fn trait_functions_are_called_through_the_trait() {
    let main = format!(
        "{USER}fn main() -> () {{\n  u = User(name: \"a\")\n  s: String = Show.show(u)\n  o: Ordering = Ord.compare(u, u)\n  t = u |> Show.show\n  ()\n}}\n"
    );
    assert_clean(&[("/app/src/main.emel", &main)]);
}

#[test]
fn calls_need_an_impl() {
    let main = format!(
        "{POINT}fn main() -> () {{\n  s = Show.show(Point(x: 1, y: 2))\n  e = Eq.eq(Point(x: 1, y: 2), Point(x: 1, y: 2))\n  ()\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        [
            "E0326 type `Point` does not implement `Show` @ Show.show",
            "E0326 type `Point` does not implement `Eq` @ Eq.eq",
        ]
    );
}

#[test]
fn constraints_are_checked_where_the_function_is_used() {
    let main = format!(
        "{USER}{POINT}type Tagged(id: Int) derive Show\nfn label[A: Show](x: A) -> String {{ Show.show(x) }}\nfn main() -> () {{\n  a = label(User(name: \"a\"))\n  b = label(Tagged(id: 1))\n  c = label(1)\n  d = label(Point(x: 1, y: 2))\n  f = label\n  ()\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        ["E0326 type `Point` does not implement `Show` @ label"]
    );
}

#[test]
fn constraints_include_supertraits() {
    let main = format!(
        "{USER}fn same[A: Ord](a: A, b: A) -> Bool {{ Eq.eq(a, b) }}\nfn show_it[A: Ord](a: A) -> String {{ Show.show(a) }}\nfn main() -> () {{\n  b = same(User(name: \"a\"), User(name: \"b\"))\n  ()\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        ["E0326 type `A` does not implement `Show` @ Show.show"]
    );
}

#[test]
fn impl_constraints_are_followed_into_type_arguments() {
    let main = format!(
        "{USER}{POINT}type Box[A](value: A)\nimpl Box[A: Show]: Show {{\n  fn show(self) {{ Show.show(self.value) }}\n}}\ntype Loose[A](value: A)\nimpl Loose[A]: Show {{\n  fn show(self) {{ Show.show(self.value) }}\n}}\nfn main() -> () {{\n  a = Show.show(Box(value: User(name: \"a\")))\n  b = Show.show(Box(value: Box(value: Point(x: 1, y: 2))))\n  ()\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        [
            "E0326 type `A` does not implement `Show` @ Show.show",
            "E0326 type `Point` does not implement `Show` @ Show.show",
        ]
    );
}

#[test]
fn functions_without_self_are_chosen_by_the_expected_type() {
    let main = format!(
        "{USER}trait Parse {{\n  fn parse(text: String) -> Self\n}}\nimpl User: Parse {{\n  fn parse(text) {{ User(name: text) }}\n}}\nfn read(text: String) -> User {{ Parse.parse(text) }}\nfn main() -> () {{\n  u: User = Parse.parse(\"a\")\n  n: Int = Parse.parse(\"1\")\n  ()\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        ["E0326 type `Int` does not implement `Parse` @ Parse.parse"]
    );
}

const PARSE: &str = "trait Parse {\n  fn parse(text: String) -> Self\n}\nimpl User: Parse {\n  fn parse(text) { User(name: text) }\n}\nimpl Int: Parse {\n  fn parse(text) { 1 }\n}\nfn greet(u: User) -> String { u.name }\n";

#[test]
fn self_can_be_inferred_from_later_use() {
    // Spec 10.3: `Parse.parse(s) + 1` is Int; passing the result on decides it too.
    let main = format!(
        "{USER}{PARSE}fn main() -> () {{\n  n = Parse.parse(\"1\")\n  m = n + 1\n  g = greet(Parse.parse(\"a\"))\n  u = Parse.parse(\"b\")\n  h = greet(u)\n  ()\n}}\n"
    );
    assert_clean(&[("/app/src/main.emel", &main)]);
}

#[test]
fn undetermined_self_is_an_error() {
    // No type is chosen by default (spec 10.3), not even through the function's inferred type.
    let main = format!(
        "{USER}{PARSE}fn read(text: String) {{ Parse.parse(text) }}\nfn main() -> () {{\n  x = Parse.parse(\"1\")\n  f = Parse.parse\n  n: Int = Parse.parse(\"2\")\n  ()\n}}\n"
    );
    let message = "E0333 cannot infer which impl of `Parse` to use: add a type annotation";
    assert_eq!(
        diagnostics(&main),
        [
            format!("{message} @ Parse.parse"),
            format!("{message} @ Parse.parse"),
            format!("{message} @ Parse.parse"),
        ]
    );
}

#[test]
fn default_implementations_use_the_trait_on_self() {
    let main = format!(
        "{USER}trait Pretty: Show {{\n  fn pretty(self) -> String {{ \"<#{{Show.show(self)}}>\" }}\n}}\nimpl User: Pretty {{}}\nfn main() -> () {{\n  p = Pretty.pretty(User(name: \"a\"))\n  ()\n}}\n"
    );
    assert_clean(&[("/app/src/main.emel", &main)]);
}

#[test]
fn only_eq_ord_and_show_can_be_derived() {
    let main = "trait Label { fn label(self) -> String }\ntype Point(x: Int) derive Eq, Label\n";
    assert_eq!(
        errors(main),
        ["E0326 cannot derive `Label`: only `Eq`, `Ord` and `Show` can be derived @ Label"]
    );
}

#[test]
fn derived_fields_need_the_trait() {
    // A Float field rules out Ord (spec 10.6), a function rules out everything, and a type
    // without the trait rules it out.
    let main = "type Plain(x: Int)\ntype Celsius(value: Float) derive Eq, Ord\ntype Callback(f: fn(Int) -> Int) derive Show\ntype Holder(p: Plain) derive Eq\n";
    assert_eq!(
        errors(main),
        [
            "E0326 type `Float` does not implement `Ord` @ Ord",
            "E0326 type `fn(Int) -> Int` does not implement `Show` @ Show",
            "E0326 type `Plain` does not implement `Eq` @ Eq",
        ]
    );
    // A field's own impl, written or derived, counts.
    let main = "type Email(value: String)\nimpl Email: Show {\n  fn show(self) { self.value }\n}\ntype Id(n: Int) derive Show\ntype User(id: Id, email: Email, tags: List[String]) derive Show\n";
    assert_clean(&[("/app/src/main.emel", main)]);
}

#[test]
fn deriving_ord_needs_eq() {
    // A derive does not derive the supertraits.
    let main = "type Point(x: Int) derive Ord\n";
    assert_eq!(
        errors(main),
        ["E0326 type `Point` does not implement `Eq` @ Ord"]
    );
    let main =
        "type Point(x: Int) derive Ord\nimpl Point: Eq {\n  fn eq(self, other) { True }\n}\n";
    assert_clean(&[("/app/src/main.emel", main)]);
}

#[test]
fn a_derive_and_an_impl_of_the_same_trait_conflict() {
    let main = "type Point(x: Int) derive Show, Eq, Eq\nimpl Point: Show {\n  fn show(self) { \"p\" }\n}\n";
    assert_eq!(
        errors(main),
        [
            "E0330 `Show` is already implemented for `Point` @ Show",
            "E0330 `Eq` is already implemented for `Point` @ Eq",
        ]
    );
}

#[test]
fn a_derive_constrains_every_type_parameter() {
    // `enum Tree[A] { ... } derive Eq` is `impl Tree[A: Eq]: Eq`, even for a parameter that no
    // field mentions (spec 10.5).
    let main = "type Plain(x: Int)\nenum Tree[A] {\n  Leaf\n  Node(Tree[A], A, Tree[A])\n}\n  derive Eq\ntype Tag[A](name: String) derive Eq\nfn same[A: Eq](a: A, b: A) -> Bool { Eq.eq(a, b) }\nfn main() -> () {\n  a = same(Leaf, Node(Leaf, 1, Leaf))\n  b = same(Node(Leaf, Plain(x: 1), Leaf), Leaf)\n  t: Tag[Plain] = Tag(name: \"a\")\n  c = same(t, t)\n  ()\n}\n";
    assert_eq!(
        diagnostics(main),
        [
            "E0326 type `Plain` does not implement `Eq` @ same",
            "E0326 type `Plain` does not implement `Eq` @ same",
        ]
    );
}

#[test]
fn opaque_types_can_derive() {
    let types = "pub opaque type Secret(value: String) derive Eq, Show\n";
    let main = "import Types.{Secret}\nfn label(s: Secret) -> String { Show.show(s) }\n";
    assert_clean(&[("/app/src/types.emel", types), ("/app/src/main.emel", main)]);
}

#[test]
fn unused_derives_of_private_types_are_reported() {
    // Uses: `==`, `<`, interpolation, a constrained function, a field of another derive, and a
    // supertrait (removing Eq would break Ord), and inspect. dbg and the sides of an assert need
    // no Show, and a type's own field is no use of it. A `pub` type is left alone, and so is an
    // error's Show, its display when it reaches `main` (spec 10.5, 14.3).
    let main = "type Unused(x: Int) derive Eq, Show\npub type Public(x: Int) derive Eq\ntype ByEq(x: Int) derive Eq\ntype ByLt(x: Int) derive Eq, Ord\ntype ByInterp(x: Int) derive Show\ntype ByCall(x: Int) derive Show\ntype Inner(x: Int) derive Eq\ntype Outer(i: Inner) derive Eq\nenum Tree {\n  Leaf\n  Node(Tree, Int)\n}\n  derive Eq\ntype ByDbg(x: Int) derive Show\ntype ByInspect(x: Int) derive Show\ntype ByAssert(x: Int) derive Eq, Show\nerror Down(x: Int) derive Eq, Show\nfn label[A: Show](x: A) -> String { Show.show(x) }\nfn main() -> () {\n  a = ByEq(x: 1) == ByEq(x: 2)\n  b = ByLt(x: 1) < ByLt(x: 2)\n  c = \"#{ByInterp(x: 1)}\"\n  d = label(ByCall(x: 1))\n  dbg(ByDbg(x: 1))\n  e = inspect(ByInspect(x: 1))\n  assert ByAssert(x: 1) == ByAssert(x: 1)\n  o = Outer(i: Inner(x: 1))\n  t = Leaf\n  ()\n}\n";
    assert_eq!(
        diagnostics(main),
        [
            "W0303 derived `Eq` for `Unused` is never used @ Eq",
            "W0303 derived `Show` for `Unused` is never used @ Show",
            "W0303 derived `Eq` for `Outer` is never used @ Eq",
            "W0303 derived `Eq` for `Tree` is never used @ Eq",
            "W0303 derived `Show` for `ByDbg` is never used @ Show",
            "W0303 derived `Show` for `ByAssert` is never used @ Show",
            "W0303 derived `Eq` for `Down` is never used @ Eq",
        ]
    );
}

#[test]
fn a_derive_used_through_a_generic_type_counts() {
    let main = "type Point(x: Int) derive Eq\ntype Box[A](value: A) derive Eq\nfn main() -> Bool { Eq.eq(Box(value: Point(x: 1)), Box(value: Point(x: 2))) }\n";
    assert_eq!(diagnostics(main), Vec::<String>::new());
}

#[test]
fn operators_and_interpolation_need_the_traits() {
    // `==` needs Eq, comparisons Ord (but Float's are built in), interpolation and inspect Show
    // (spec 10.3, 10.6). A function is none of them. dbg needs no Show.
    let main = format!(
        "{POINT}fn f(x: Int) -> Int {{ x }}\nfn main() {{\n  p = Point(x: 1, y: 2)\n  a = \"#{{p}}\"\n  b = p == p\n  c = f == f\n  d = [1.0] < [2.0]\n  e = 1.0 < 2.0\n  g = p < p\n  h = inspect([p])\n  i = inspect(f)\n  j = dbg((p, f))\n}}\n"
    );
    assert_eq!(
        diagnostics(&main),
        [
            "E0326 type `Point` does not implement `Show` @ p",
            "E0326 type `Point` does not implement `Eq` @ p == p",
            "E0326 type `fn(Int) -> Int` does not implement `Eq` @ f == f",
            "E0326 type `Float` does not implement `Ord` @ [1.0] < [2.0]",
            "E0326 type `Point` does not implement `Ord` @ p < p",
            "E0326 type `Point` does not implement `Show` @ inspect",
            "E0326 type `fn(Int) -> Int` does not implement `Show` @ inspect",
        ]
    );
}

#[test]
fn constraints_and_derives_give_the_operators() {
    let main = "type Pt(x: Int)\n  derive Eq, Ord, Show\nfn big[A: Ord](x: A, y: A) -> Bool { x < y && x == y }\nfn say[A: Show](x: A) -> String { \"#{x}\" }\nfn main() {\n  p = Pt(x: 1)\n  a = \"#{p} #{[p]} #{(1, Some(p))}\"\n  b = p == p && [p] < [p] && big(p, p)\n}\n";
    assert_clean(&[("/app/src/main.emel", main)]);
}
