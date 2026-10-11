//! 仕様（emela-0.20-spec.md）の例を型検査にかける．
//!
//! 仕様の例は断片（文だけ，定義のない名前，alpha.2 の構文）が多いので，足りない定義を補って
//! 完全なプログラムにしてある．補った部分には「補」と書く．節の番号は仕様のもの．
//! 前半は通るべき例，後半は本文が誤りと定めている書き方．

mod common;

use common::render_bodies;

// ---------------------------------------------------------------------------
// 通るべき例

/// 2.4 リテラル．16進と2進，文脈で決まる Int64，複数行の文字列と補間．
#[test]
fn spec_2_4_literals() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn literals(id: Int) -> String {
  hex = 0xFF
  bin = 0b1010
  big: Int64 = 0xFFFF_FFFF
  million = 1_000_000
  ratio = 1.0e9
  query = """
    SELECT *
    FROM users
    WHERE id = #{id}
    """
  query
}
"#
    ));
}

/// 5.3 type と部分更新．型引数は変わってよい（`Pair[Int, Bool]` から `Pair[String, Bool]`）．
#[test]
fn spec_5_3_types_and_update() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(id: Int, name: String)

type Pair[A, B](first: A, second: B)

type Todo(
  id:        Int,
  title:     String,
  completed: Bool,
)

fn update(user: User, todo: Todo, p: Pair[Int, Bool]) -> Pair[String, Bool] {
  renamed = User(name: "b", ..user)
  fixed   = Todo(title: "Buy milk", completed: True, ..todo)
  name = renamed.name
  Pair(first: "s", ..p)
}
"#
    ));
}

/// 5.4 enum．名前付き，位置引数，中身なしのバリアント．
#[test]
fn spec_5_4_enum() {
    insta::assert_snapshot!(render_bodies(
        r#"
enum Shape {
  Circle(radius: Float)     # 名前付き
  Rect(Float, Float)        # 位置引数
  Empty                     # 中身なし
}

fn shapes() -> List[Shape] {
  [Circle(1.0), Circle(radius: 2.0), Rect(1.0, 2.0), Empty]
}
"#
    ));
}

/// 5.6 const．型を書かない const も値から推論する．
#[test]
fn spec_5_6_const() {
    insta::assert_snapshot!(render_bodies(
        r#"
type Config(port: Int)  # 補

const MAX_RETRY = 3
const DEFAULT_CONFIG = Config(port: 8080)

fn port() -> Int {
  DEFAULT_CONFIG.port + MAX_RETRY
}
"#
    ));
}

/// 6.1 関数の宣言．`find` は fails と use を使うので alpha.2（ここでは省く）．
#[test]
fn spec_6_1_functions() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn add(x: Int, y: Int) -> Int {
  x + y
}

fn map[A, B](xs: List[A], f: fn(A) -> B) -> List[B] { List.map(xs, f) }

fn inc() -> fn(Int) -> Int {
  fn(x) { x + 1 }
}
"#
    ));
}

/// 6.2 引数とラベル．位置，名前付き，混在の3通り．
#[test]
fn spec_6_2_labels() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn connect(host: String, port: Int) { () }

fn calls() {
  connect("localhost", 5432)
  connect("localhost", port: 5432)
  connect(host: "localhost", port: 5432)
}
"#
    ));
}

/// 6.3 ブロックと束縛．束縛し直し，タプルの分解，型注釈．
/// `Map.to_list` は core にないので `List.zip` に置き換えた（補）．
#[test]
fn spec_6_3_bindings() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn is_even(n: Int) -> Bool { n % 2 == 0 }  # 補

fn bindings(xs: List[Int], names: List[String]) -> Int {
  count = 0
  count = count + 1            # 同名での束縛し直し
  (evens, odds) = List.partition(xs, is_even)
  pairs: List[(String, Int)] = List.zip(names, xs)
  count
}
"#
    ));
}

/// 6.4 if．`if !valid { fail ... }` は fail を使うので alpha.2（ここでは省く）．
#[test]
fn spec_6_4_if() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn label(count: Int) -> String {
  label = if count > 0 { "あり" } else { "なし" }
  label
}
"#
    ));
}

/// 6.5 match とパターン．ガード付きの腕と網羅．
#[test]
fn spec_6_5_match() {
    insta::assert_snapshot!(render_bodies(
        r#"
enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn area(shape: Shape) -> Float {
  match shape {
    Circle(radius:) if radius > 0.0 -> 3.14 * radius * radius
    Circle(_)                       -> 0.0
    Rect(w, h)                      -> w * h
    Empty                           -> 0.0
  }
}
"#
    ));
}

/// 6.7 パイプ．`List.fold(init: 0, f: add)` の名前付き引数もパイプの左辺の後に並ぶ．
#[test]
fn spec_6_7_pipe() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn is_valid(n: Int) -> Bool { n >= 0 }  # 補
fn add(acc: Int, n: Int) -> Int { acc + n }  # 補

fn total(scores: List[Int]) -> Int {
  scores
    |> List.filter(is_valid)
    |> List.fold(init: 0, f: add)
}
"#
    ));
}

/// 9.2 関数型．`compose` の戻り値の関数（fails と use の推論は alpha.2）．
#[test]
fn spec_9_2_compose() {
    insta::assert_snapshot!(render_bodies(
        r##"
fn compose[A, B, C](f: fn(A) -> B, g: fn(B) -> C) -> fn(A) -> C {
  fn(x) { g(f(x)) }
}

fn use_it() -> String {
  h = compose(fn(n: Int) { n + 1 }, fn(n) { "#{n}" })
  h(1)
}
"##
    ));
}

/// 9.4 Never は分岐の合流で他の型になる．if の一方が panic でも全体はもう一方の型．
#[test]
fn spec_9_4_never_joins() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn pick(c: Bool, n: Int) -> Int {
  a = if c { n } else { panic("no") }
  b = match c {
    True -> todo()
    False -> n
  }
  a + b
}
"#
    ));
}

// ---------------------------------------------------------------------------
// 本文が誤りと定めている書き方

/// 2.4 整数リテラルが文脈の型の範囲を超える．2の補数として読み替えない．
#[test]
fn spec_2_4_out_of_range() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn f() -> Int {
  a: Int = 0xFFFF_FFFF
  b: Int64 = 0xFFFF_FFFF
  c: Float = 1
  a
}
"#
    ));
}

/// 5.3 部分更新で引き継ぐフィールドの型が矛盾する，または `..` の後が別の型．
#[test]
fn spec_5_3_update_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(id: Int, name: String)
type Pair[A, B](first: A, second: B)

fn f(u: User, p: Pair[Int, Bool]) -> Int {
  a: Pair[String, String] = Pair(first: "s", ..p)
  b = User(name: "b", ..p)
  c = User(nmae: "b", ..u)
  1
}
"#
    ));
}

/// 6.4 値として使う if は else が必要．else のない if の本体は () か Never．
#[test]
fn spec_6_4_if_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn f(count: Int) -> Int {
  a = if count > 0 { 1 }
  b = if count { 1 } else { 2 }
  count
}
"#
    ));
}

/// 6.3，6.5 束縛のパターンは必ず成功するものに限る．match は網羅する．冗長な腕は警告．
/// 名前付きフィールドを `..` なしに省いたパターンはエラー．
#[test]
fn spec_6_5_pattern_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(id: Int, name: String)
enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn f(shape: Shape, o: Option[Int], u: User) -> Float {
  Some(n) = o
  User(name:) = u
  match shape {
    Circle(radius:) if radius > 0.0 -> radius
    Rect(w, h) -> w
    _ -> 0.0
    Empty -> 1.0
  }
}
"#
    ));
}

/// 6.6 算術演算子は Int，Int64，Float にだけ使え，両辺は同じ型．文字列の連結に + は使えない．
#[test]
fn spec_6_6_operator_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn f(n: Int, big: Int64, s: String) -> Int {
  a = n + big
  b = s + s
  c = n + 1.5
  d = !n
  n
}
"#
    ));
}

/// 9.4 Never が他の型として扱われるのは最上位だけ．`List[Never]` と `List[Int]` は一致しない．
#[test]
fn spec_9_4_list_never_is_not_list_int() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn nothing() -> List[Never] { [] }

fn f() -> Int {
  xs: List[Int] = nothing()
  1
}
"#
    ));
}

/// alpha.2 の構文（fail，use，with）は「まだ扱えない」の診断になる．
#[test]
fn alpha2_constructs_are_not_supported_yet() {
    insta::assert_snapshot!(render_bodies(
        r#"
error Invalid(input: String)

fn validate(input: String, valid: Bool) -> String {
  if !valid { fail Invalid(input:) }
  input
}
"#
    ));
}
