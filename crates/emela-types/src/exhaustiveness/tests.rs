use super::*;

/// A test type whose shape is visible only through `CtorTable`.
#[derive(Clone, Debug)]
enum T {
    Int,
    Float,
    Str,
    Bool,
    Unit,
    /// An enum with no constructors.
    Never,
    /// `enum Shape { Circle(radius: Float), Rect(w: Float, h: Float), Empty }` (spec 6.5).
    Shape,
    Option(Box<T>),
    Tuple(Vec<T>),
    List(Box<T>),
    /// `enum Tree { Leaf, Node(Tree, Int, Tree) }`．
    Tree,
    /// `enum Json { Null, Arr(List[Option[Json]]) }`．
    Json,
}

struct Table;

fn variant(name: &str, fields: Vec<T>) -> VariantShape<T> {
    VariantShape {
        name: name.into(),
        fields,
    }
}

impl CtorTable for Table {
    type Ty = T;

    fn shape(&self, ty: &T) -> TyShape<T> {
        match ty {
            T::Int | T::Float | T::Str => TyShape::Opaque,
            T::Bool => TyShape::Enum(vec![variant("True", vec![]), variant("False", vec![])]),
            T::Unit => TyShape::Tuple(vec![]),
            T::Never => TyShape::Enum(vec![]),
            T::Shape => TyShape::Enum(vec![
                variant("Circle", vec![T::Float]),
                variant("Rect", vec![T::Float, T::Float]),
                variant("Empty", vec![]),
            ]),
            T::Option(a) => TyShape::Enum(vec![
                variant("Some", vec![(**a).clone()]),
                variant("None", vec![]),
            ]),
            T::Tuple(ts) => TyShape::Tuple(ts.clone()),
            T::List(a) => TyShape::List((**a).clone()),
            T::Tree => TyShape::Enum(vec![
                variant("Leaf", vec![]),
                variant("Node", vec![T::Tree, T::Int, T::Tree]),
            ]),
            T::Json => TyShape::Enum(vec![
                variant("Null", vec![]),
                variant("Arr", vec![T::List(Box::new(T::Option(Box::new(T::Json))))]),
            ]),
        }
    }
}

fn option(t: T) -> T {
    T::Option(Box::new(t))
}

fn list(t: T) -> T {
    T::List(Box::new(t))
}

// Constructor indices follow the order in `Table`.
const W: Pat = Pat::Wild;

fn int(n: i64) -> Pat {
    Pat::Lit(Lit::Int(n))
}

fn float(x: f64) -> Pat {
    Pat::Lit(Lit::Float(x))
}

fn string(s: &str) -> Pat {
    Pat::Lit(Lit::Str(s.into()))
}

fn tru() -> Pat {
    Pat::variant(0, vec![])
}

fn fals() -> Pat {
    Pat::variant(1, vec![])
}

fn some(p: Pat) -> Pat {
    Pat::variant(0, vec![p])
}

fn none() -> Pat {
    Pat::variant(1, vec![])
}

fn circle(r: Pat) -> Pat {
    Pat::variant(0, vec![r])
}

fn rect(w: Pat, h: Pat) -> Pat {
    Pat::variant(1, vec![w, h])
}

fn empty() -> Pat {
    Pat::variant(2, vec![])
}

fn tuple(ps: Vec<Pat>) -> Pat {
    Pat::Tuple(ps)
}

fn leaf() -> Pat {
    Pat::variant(0, vec![])
}

fn node(l: Pat, v: Pat, r: Pat) -> Pat {
    Pat::variant(1, vec![l, v, r])
}

fn null() -> Pat {
    Pat::variant(0, vec![])
}

fn arr(p: Pat) -> Pat {
    Pat::variant(1, vec![p])
}

fn arm(pat: Pat) -> Arm {
    Arm {
        pat,
        guarded: false,
    }
}

fn guarded(pat: Pat) -> Arm {
    Arm { pat, guarded: true }
}

/// The displayed missing-value examples, and the indices of redundant arms.
fn check(ty: T, arms: Vec<Arm>) -> (Vec<String>, Vec<usize>) {
    let r = check_match(&Table, &ty, &arms);
    (
        r.missing.iter().map(|w| w.to_string()).collect(),
        r.redundant,
    )
}

fn missing(ty: T, arms: Vec<Arm>) -> Vec<String> {
    let (missing, redundant) = check(ty, arms);
    assert!(redundant.is_empty(), "redundant arms: {redundant:?}");
    missing
}

fn redundant(ty: T, arms: Vec<Arm>) -> Vec<usize> {
    let (missing, redundant) = check(ty, arms);
    assert!(missing.is_empty(), "not exhaustive: {missing:?}");
    redundant
}

#[test]
fn shape_spec_example() {
    // Circle(radius:) if radius > 0.0 -> ...
    // Circle(_) -> ...
    // Rect(w, h) -> ...
    // Empty -> ...
    let arms = vec![
        guarded(circle(W)),
        arm(circle(W)),
        arm(rect(W, W)),
        arm(empty()),
    ];
    assert_eq!(check(T::Shape, arms), (vec![], vec![]));
}

#[test]
fn shape_missing_rect() {
    let arms = vec![arm(circle(W)), arm(empty())];
    assert_eq!(missing(T::Shape, arms), ["Rect(_, _)"]);
}

#[test]
fn shape_missing_several() {
    assert_eq!(
        missing(T::Shape, vec![arm(empty())]),
        ["Circle(_)", "Rect(_, _)"]
    );
}

#[test]
fn shape_named_fields_rest() {
    // `Rect(h: 0.0, ..)` treats `w` as a wildcard.
    let rect_h0 = Pat::Variant {
        variant: 1,
        fields: vec![(1, float(0.0))],
    };
    let arms = vec![arm(rect_h0), arm(circle(W)), arm(empty())];
    assert_eq!(missing(T::Shape, arms), ["Rect(_, _)"]);

    let rect_h0 = Pat::Variant {
        variant: 1,
        fields: vec![(1, float(0.0))],
    };
    let arms = vec![arm(rect(W, W)), arm(rect_h0), arm(circle(W)), arm(empty())];
    assert_eq!(redundant(T::Shape, arms), [1]);
}

#[test]
fn no_arms() {
    assert_eq!(missing(T::Shape, vec![]), ["_"]);
    assert_eq!(missing(T::Int, vec![]), ["_"]);
    // An enum with no constructors is exhaustive with no arms.
    assert_eq!(missing(T::Never, vec![]), Vec::<String>::new());
}

// Bool

#[test]
fn bool_exhaustive() {
    assert_eq!(
        check(T::Bool, vec![arm(tru()), arm(fals())]),
        (vec![], vec![])
    );
}

#[test]
fn bool_missing_false() {
    assert_eq!(missing(T::Bool, vec![arm(tru())]), ["False"]);
}

#[test]
fn nested_option_missing_some_none() {
    let ty = option(option(T::Bool));
    let arms = vec![arm(some(some(W))), arm(none())];
    assert_eq!(missing(ty, arms), ["Some(None)"]);
}

#[test]
fn nested_option_lists_every_missing_case() {
    let ty = option(option(T::Bool));
    let arms = vec![arm(some(some(tru())))];
    assert_eq!(
        missing(ty, arms),
        ["Some(Some(False))", "Some(None)", "None"]
    );
}

#[test]
fn nested_option_exhaustive() {
    let ty = option(option(T::Bool));
    let arms = vec![
        arm(some(some(tru()))),
        arm(some(some(fals()))),
        arm(some(none())),
        arm(none()),
    ];
    assert_eq!(check(ty, arms), (vec![], vec![]));
}

#[test]
fn tuple_of_bools() {
    let ty = T::Tuple(vec![T::Bool, T::Bool]);
    let arms = vec![arm(tuple(vec![tru(), W])), arm(tuple(vec![W, tru()]))];
    assert_eq!(missing(ty, arms), ["(False, False)"]);
}

#[test]
fn tuple_exhaustive_and_redundant() {
    let ty = T::Tuple(vec![T::Bool, option(T::Int)]);
    let arms = vec![
        arm(tuple(vec![tru(), W])),
        arm(tuple(vec![fals(), some(W)])),
        arm(tuple(vec![fals(), none()])),
        arm(tuple(vec![tru(), none()])),
    ];
    assert_eq!(redundant(ty, arms), [3]);
}

#[test]
fn unit() {
    assert_eq!(check(T::Unit, vec![arm(tuple(vec![]))]), (vec![], vec![]));
    assert_eq!(missing(T::Unit, vec![]), ["_"]);
}

#[test]
fn list_empty_and_cons_exhaustive() {
    let ty = list(T::Int);
    let arms = vec![arm(Pat::list(vec![])), arm(Pat::list_rest(vec![W]))];
    assert_eq!(check(ty, arms), (vec![], vec![]));
}

#[test]
fn list_missing_cons() {
    assert_eq!(
        missing(list(T::Int), vec![arm(Pat::list(vec![]))]),
        ["[_, ..]"]
    );
}

#[test]
fn list_missing_empty() {
    assert_eq!(
        missing(list(T::Int), vec![arm(Pat::list_rest(vec![W]))]),
        ["[]"]
    );
}

#[test]
fn list_fixed_lengths_only() {
    let arms = vec![
        arm(Pat::list(vec![])),
        arm(Pat::list(vec![W])),
        arm(Pat::list(vec![W, W])),
    ];
    assert_eq!(missing(list(T::Int), arms), ["[_, _, _, ..]"]);
}

#[test]
fn list_gap_between_fixed_and_rest() {
    // `[]` and `[_, _, ..]` leave out length 1.
    let arms = vec![arm(Pat::list(vec![])), arm(Pat::list_rest(vec![W, W]))];
    assert_eq!(missing(list(T::Int), arms), ["[_]"]);
}

#[test]
fn list_element_patterns() {
    let arms = vec![arm(Pat::list(vec![])), arm(Pat::list_rest(vec![tru()]))];
    assert_eq!(missing(list(T::Bool), arms), ["[False, ..]"]);
}

#[test]
fn list_redundant() {
    let arms = vec![
        arm(Pat::list(vec![])),
        arm(Pat::list_rest(vec![W])),
        arm(Pat::list(vec![W, W])),
        arm(Pat::list_rest(vec![])),
    ];
    assert_eq!(redundant(list(T::Int), arms), [2, 3]);
}

#[test]
fn literals_need_wildcard() {
    assert_eq!(missing(T::Int, vec![arm(int(0)), arm(int(1))]), ["_"]);
    assert_eq!(missing(T::Str, vec![arm(string("a"))]), ["_"]);
    assert_eq!(check(T::Int, vec![arm(int(0)), arm(W)]), (vec![], vec![]));
}

#[test]
fn literals_redundant() {
    assert_eq!(
        redundant(T::Int, vec![arm(int(0)), arm(int(0)), arm(W)]),
        [1]
    );
    assert_eq!(
        redundant(T::Str, vec![arm(string("a")), arm(W), arm(string("b"))]),
        [2]
    );
    // Floats are compared by value.
    assert_eq!(
        redundant(T::Float, vec![arm(float(0.0)), arm(float(-0.0)), arm(W)]),
        [1]
    );
    // A NaN constant matches itself, so only the second arm is redundant.
    let nan = || arm(float(f64::NAN));
    assert_eq!(redundant(T::Float, vec![nan(), nan(), arm(W)]), [1]);
}

#[test]
fn literal_in_witness() {
    let ty = T::Tuple(vec![T::Int, T::Bool]);
    let arms = vec![arm(tuple(vec![int(0), tru()])), arm(tuple(vec![int(1), W]))];
    assert_eq!(missing(ty, arms), ["(0, False)", "(_, _)"]);
}

#[test]
fn literal_and_enum_columns() {
    let ty = T::Tuple(vec![T::Int, T::Bool]);
    let arms = vec![arm(tuple(vec![int(0), tru()])), arm(tuple(vec![W, fals()]))];
    assert_eq!(missing(ty, arms), ["(_, True)"]);
}

#[test]
fn guarded_arm_not_counted_for_exhaustiveness() {
    let arms = vec![guarded(circle(W)), arm(rect(W, W)), arm(empty())];
    assert_eq!(missing(T::Shape, arms), ["Circle(_)"]);
    assert_eq!(missing(T::Bool, vec![guarded(W)]), ["_"]);
}

#[test]
fn guarded_arm_does_not_cover_later_arms() {
    assert_eq!(check(T::Bool, vec![guarded(W), arm(W)]), (vec![], vec![]));
    assert_eq!(
        check(T::Bool, vec![guarded(tru()), arm(tru()), arm(fals())]),
        (vec![], vec![])
    );
}

#[test]
fn guarded_arm_can_itself_be_redundant() {
    let arms = vec![arm(tru()), guarded(tru()), arm(fals())];
    assert_eq!(redundant(T::Bool, arms), [1]);
}

#[test]
fn redundant_after_wildcard() {
    assert_eq!(redundant(T::Shape, vec![arm(W), arm(empty())]), [1]);
}

#[test]
fn redundant_covered_by_several_arms() {
    let arms = vec![arm(circle(W)), arm(rect(W, W)), arm(empty()), arm(W)];
    assert_eq!(redundant(T::Shape, arms), [3]);
}

#[test]
fn recursive_tree() {
    let arms = vec![
        arm(leaf()),
        arm(node(leaf(), W, W)),
        arm(node(node(W, W, W), W, leaf())),
    ];
    assert_eq!(
        missing(T::Tree, arms),
        ["Node(Node(_, _, _), _, Node(_, _, _))"]
    );

    let arms = vec![arm(node(W, int(0), W)), arm(leaf())];
    assert_eq!(missing(T::Tree, arms), ["Node(_, _, _)"]);
}

#[test]
fn recursive_through_list_and_option() {
    let arms = vec![
        arm(null()),
        arm(arr(Pat::list(vec![]))),
        arm(arr(Pat::list_rest(vec![none()]))),
        arm(arr(Pat::list_rest(vec![some(null())]))),
    ];
    assert_eq!(missing(T::Json, arms), ["Arr([Some(Arr(_)), ..])"]);
}

#[test]
fn recursive_wildcard_only() {
    assert_eq!(check(T::Json, vec![arm(W)]), (vec![], vec![]));
    assert_eq!(missing(list(T::Json), vec![]), ["_"]);
}

#[test]
fn witness_display() {
    let wild = || Witness::Wild;
    let cases = [
        (
            Witness::List {
                elems: vec![],
                rest: true,
            },
            "[..]",
        ),
        (
            Witness::List {
                elems: vec![],
                rest: false,
            },
            "[]",
        ),
        (
            Witness::List {
                elems: vec![wild(), wild()],
                rest: false,
            },
            "[_, _]",
        ),
        (Witness::Tuple(vec![]), "()"),
        (Witness::Lit(Lit::Float(1.0)), "1.0"),
        (Witness::Lit(Lit::Str("a\"b".into())), r#""a\"b""#),
        (
            Witness::Variant {
                name: "Empty".into(),
                args: vec![],
            },
            "Empty",
        ),
    ];
    for (w, expected) in cases {
        assert_eq!(w.to_string(), expected);
    }
}

#[test]
fn witnesses_are_limited() {
    // Arms from `(0, True)` to `(99, True)` leave 100 values `(k, False)` missing.
    let ty = T::Tuple(vec![T::Int, T::Bool]);
    let arms = (0..100)
        .map(|k| arm(tuple(vec![int(k), tru()])))
        .collect::<Vec<_>>();
    let r = check_match(&Table, &ty, &arms);
    assert_eq!(r.missing.len(), WITNESS_LIMIT);
    assert_eq!(r.missing[0].to_string(), "(0, False)");
    assert!(!r.is_exhaustive());
}

#[test]
fn compact_witnesses_for_wide_tuple() {
    // Seven values are missing, but `_` groups them into three.
    let ty = T::Tuple(vec![T::Bool; 3]);
    let arms = vec![arm(tuple(vec![tru(); 3]))];
    assert_eq!(
        missing(ty, arms),
        ["(True, True, False)", "(True, False, _)", "(False, _, _)"]
    );
}
