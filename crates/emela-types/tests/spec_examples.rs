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

/// 6.1 関数の宣言．`find` は use を使うので，効果の段まで省く．
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

/// 6.4 if．else のない if の本体は `fail` でもよい（Never）．
#[test]
fn spec_6_4_if() {
    insta::assert_snapshot!(render_bodies(
        r#"
error Invalid(input: String)  # 補

fn label(count: Int) -> String {
  label = if count > 0 { "あり" } else { "なし" }
  label
}

fn validate(input: String, valid: Bool) -> String fails Invalid {  # 補
  if !valid { fail Invalid(input:) }
  input
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

/// 9.2 関数型．`compose` の戻り値の関数は f と g のエラーを持つと推論する．compose 自身は失敗しない．
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

/// 7.1 error の宣言は同じ名前の型と構成子を定義する．エラーの値は普通の値として扱える．
#[test]
fn spec_7_1_error_values() {
    insta::assert_snapshot!(render_bodies(
        r#"
error NotFound(id: Int)
error Unauthorized

fn values() -> Int {  # 補
  e = NotFound(id: 1)
  es = [e, NotFound(id: 2)]
  u = Unauthorized
  e.id
}
"#
    ));
}

/// 7.2 fail と or_fail．型引数が値と fails の両方に出たら1つのエラーの型．
#[test]
fn spec_7_2_fail_and_or_fail() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(id: Int, name: String)  # 補
error NotFound(id: Int)

fn or_fail[A, E](opt: Option[A], err: E) -> A fails E {
  match opt {
    Some(a) -> a
    None    -> fail err
  }
}

fn find(users: List[User], id: Int) {  # 補
  user = List.find(users, fn(u) { u.id == id }) |> or_fail(NotFound(id:))
  user
}

fn raise(id: Int) {  # 補
  fail NotFound(id:)
}
"#
    ));
}

/// 7.3 escape．復帰，一部だけ受ける，別のエラーに変える，値として扱う．
#[test]
fn spec_7_3_escape() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(name: String)  # 補
type Data(n: Int)  # 補
error NotFound(id: Int)  # 補
error DbError(message: String)  # 補
error Unavailable(reason: String)  # 補

fn find_user(id: Int) -> User fails NotFound { fail NotFound(id:) }  # 補
fn guest_user() -> User { User(name: "guest") }  # 補
fn load(id: Int) -> Data fails NotFound | DbError { fail DbError(message: "down") }  # 補
fn default_data() -> Data { Data(n: 0) }  # 補

fn recover(id: Int) {
  user = find_user(id) escape {
    NotFound(_) -> guest_user()
  }
  user
}

fn partial(id: Int) {
  data = load(id) escape {
    NotFound(_) -> default_data()
  }
  data
}

fn convert(id: Int) {
  data = load(id) escape {
    DbError(message:) -> fail Unavailable(reason: message)
  }
  data
}

fn as_value(id: Int) {
  found = Some(find_user(id)) escape {
    NotFound(_) -> None
  }
  found
}
"#
    ));
}

/// 7.3 escape と集合変数．引数の関数の見えない集合変数からは消えない．`fails Timeout | E` と
/// 書き出せば，1回目の呼び出しの Timeout は型からも消える．呼ぶ側では集合が決まる．
#[test]
fn spec_7_3_escape_and_set_variables() {
    insta::assert_snapshot!(render_bodies(
        r#"
error Timeout(ms: Int)  # 補
error DbError  # 補

fn retry[A](f: fn() -> A) -> A {
  f() escape { Timeout(_) -> f() }
}

fn retry_exact[A, E](f: fn() -> A fails Timeout | E) -> A fails Timeout | E {
  f() escape { Timeout(_) -> f() }
}

fn flaky() -> Int fails Timeout | DbError { 1 }  # 補
fn steady() -> Int { 1 }  # 補

fn callers() {  # 補
  a = retry(flaky)
  b = retry_exact(flaky)
  c = retry_exact(steady)
  d = flaky() escape { Timeout(_) -> 0 }
  (a, b, c, d)
}
"#
    ));
}

/// 7.4 fails の注釈．戻り値の型を省略すれば推論，`fails` はちょうどその集合．
/// `fails Timeout | E` の引数に渡した関数のエラーのうち Timeout 以外が E になる．
#[test]
fn spec_7_4_annotations() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(name: String)  # 補
error NotFound  # 補
error DbError  # 補
error Timeout  # 補

fn inferred(n: Int) {
  if n > 0 { fail NotFound } else { fail DbError }
}

fn load(id: Int) -> User fails NotFound | DbError {
  inferred(id)
}

fn retry[A, E](f: fn() -> A fails Timeout | E) -> A fails Timeout | E { f() }

fn only_db() {  # 補
  retry(fn() { if True { fail Timeout } else { fail DbError } })
}

fn no_timeout() {  # 補．Timeout を起こさない関数を渡してもよい
  retry(fn() { fail DbError })
}
"#
    ));
}

/// 13.3，14.3 テスト関数と main の E は空でなくてよい．
#[test]
fn spec_13_3_tests_and_main_may_fail() {
    insta::assert_snapshot!(render_bodies(
        r#"
error NotFound(id: Int)  # 補

fn greet(id: Int) -> String fails NotFound { fail NotFound(id:) }  # 補

@test
fn greet_fails_for_unknown_id() {
  result = Some(greet(999)) escape { NotFound(_) -> None }
  assert result == None
}

@test
fn greet_returns_user() {
  name = greet(1)
  assert name == "alice"
}

fn main() {
  greet(1)
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

/// 8.2 `use X` は能力の値を返し，X を R に載せる．操作は能力の値を通して呼ぶ．
#[test]
fn spec_8_2_use() {
    insta::assert_snapshot!(render_bodies(
        r#"
error NotFound(id: Int)  # 補
type User(name: String)  # 補

effect Logger {  # 補
  fn info(msg: String) -> ()
}

effect Users {  # 補
  fn find(id: Int) -> Option[User]
}

effect Clock {  # 補
  fn sleep(ms: Int) -> ()
}

fn greet(id: Int) {
  log   = use Logger
  users = use Users

  user = match users.find(id) {  # 補: Option.or_fail はまだ同梱していない
    Some(u) -> u
    None -> fail NotFound(id:)
  }
  log.info("hello #{user.name}")

  (use Clock).sleep(1000)
  user
}
"#
    ));
}

/// 8.2 特定のハンドラの能力を使わせるときは，能力を捕まえたクロージャを渡す．
/// 8.3 外側で取り出した能力の値は，内側の with で上書きされても外側のハンドラを指す．
#[test]
fn spec_8_2_copy() {
    insta::assert_snapshot!(render_bodies(
        r#"
type Rows(n: Int)  # 補

effect Db {  # 補
  fn query(q: String) -> Rows
  fn execute(rows: Rows) -> ()
}

handler PrimaryDb implements Db {  # 補
  fn query(q) { Rows(n: 0) }
  fn execute(rows) { () }
}

handler ReplicaDb implements Db {  # 補
  fn query(q) { Rows(n: 1) }
  fn execute(rows) { () }
}

fn copy(read: fn() -> Rows use { Db }, write: fn(Rows) -> () use { Db }) {
  write(read())
}

fn main() {  # 補
  with PrimaryDb {
    primary = use Db
    with ReplicaDb {
      replica = use Db
      copy(fn() { replica.query("...") }, fn(rows) { primary.execute(rows) })
    }
  }
}
"#
    ));
}

/// 8.3 with は供給したエフェクトを本体の R から消す．カンマで並べられる．
/// 8.4 状態を持たないハンドラは，エフェクトの操作を全部実装する．引数の型は書かない．
#[test]
fn spec_8_3_with() {
    insta::assert_snapshot!(render_bodies(
        r#"
effect Logger {  # 補
  fn info(msg: String) -> ()
}

effect Clock {  # 補
  fn now() -> Int
}

@external(js, "console.log")  # 補
fn console_log(msg: String) -> ()

handler ConsoleLogger implements Logger {
  fn info(msg) { console_log(msg) }
}

handler FixedClock implements Clock {  # 補
  fn now() { 0 }
}

fn greet(id: Int) {  # 補
  (use Logger).info("hello")
  (use Clock).now()
}

fn run() {  # 補．ConsoleLogger だけでは Clock が残る
  with ConsoleLogger { greet(1) }
  with ConsoleLogger, FixedClock { greet(1) }
}
"#
    ));
}

/// 9.2 `fn(A) -> B use {}` は効果を持たない関数だけを受け取る．省略すれば何でもよく，呼んだ側の R に加わる．
#[test]
fn spec_9_2_effect_free_parameter() {
    insta::assert_snapshot!(render_bodies(
        r#"
effect Logger {  # 補
  fn info(msg: String) -> ()
}

fn run_pure(f: fn() -> Int use {}) -> Int { f() }  # 補
fn run_any(f: fn() -> Int) -> Int { f() }  # 補

fn callers() {  # 補
  a = run_any(fn() { (use Logger).info("a")
  1 })
  b = run_pure(fn() { (use Logger).info("b")
  1 })
  (a, b)
}
"#
    ));
}

/// 7.4 `-> T` はエラーなしを強制し，`fails X` はちょうどその集合．本体に未処理のエラーがあれば E0318．
#[test]
fn spec_7_4_annotation_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
error NotFound  # 補
error DbError  # 補

fn find() -> Int fails NotFound { fail NotFound }  # 補

fn strict() -> Int {
  find()
}

fn wrong() -> Int fails DbError {
  find()
}

const VALUE: Int = find()  # 5.6 const の値は失敗できない
"#
    ));
}

/// 7.2 `fail` に渡せるのはエラーの値だけ．7.3 escape の腕のパターンはエラーの構成子．
#[test]
fn spec_7_fail_and_escape_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
error NotFound  # 補

fn or_fail[A, E](opt: Option[A], err: E) -> A fails E {  # 補
  match opt {
    Some(a) -> a
    None -> fail err
  }
}

fn not_error() {
  fail 1
}

fn not_error_type() {
  or_fail(Some(1), "no")
}

fn catch_all() -> Int {
  0 escape { _ -> 1 }
}
"#
    ));
}

/// 9.2 `fails {}` の引数には失敗しない関数しか渡せない（E0321）．
#[test]
fn spec_9_2_pure_parameter() {
    insta::assert_snapshot!(render_bodies(
        r#"
error Timeout  # 補

fn run(f: fn() -> Int fails {}) -> Int { f() }

fn flaky() -> Int fails Timeout { fail Timeout }  # 補

fn ok() -> Int { run(fn() { 1 }) }
fn bad() -> Int { run(flaky) }
fn bad_lambda() -> Int { run(fn() { fail Timeout }) }
"#
    ));
}
