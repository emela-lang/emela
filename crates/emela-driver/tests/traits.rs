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

#[test]
fn default_implementations_use_the_trait_on_self() {
    let main = format!(
        "{USER}trait Pretty: Show {{\n  fn pretty(self) -> String {{ \"<#{{Show.show(self)}}>\" }}\n}}\nimpl User: Pretty {{}}\nfn main() -> () {{\n  p = Pretty.pretty(User(name: \"a\"))\n  ()\n}}\n"
    );
    assert_clean(&[("/app/src/main.emel", &main)]);
}
