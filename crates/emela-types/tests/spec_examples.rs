//! Type-checks the examples of the spec (emela-0.20-spec.md).
//!
//! Many spec examples are fragments (statements only, undefined names, alpha.2 syntax), so missing
//! definitions are filled in to make complete programs; filled-in parts are marked "補". Section
//! numbers are the spec's. The first half holds examples that must pass; the second half holds
//! code the text declares to be errors.

mod common;

use common::render_bodies;

// ---------------------------------------------------------------------------
// Examples that must pass

/// 2.4 Literals: hex and binary, Int64 decided by context, multiline strings and interpolation.
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

/// 5.3 Types and update. Type arguments may change (`Pair[Int, Bool]` to `Pair[String, Bool]`).
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

/// 5.4 enum: named, positional and empty variants.
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

/// 5.6 const. A const without a type is inferred from its value.
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

/// 6.1 Function declarations. `find` uses `use`, so it is left out until the effect stage.
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

/// 6.2 Parameters and labels: positional, named and mixed.
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

/// 6.3 Blocks and bindings: rebinding, tuple destructuring, type annotations. `Map.to_list` is not
/// in core, so it is replaced with `List.zip` (補).
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

/// 6.4 if. The body of an if without else may be `fail` (Never).
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

/// 6.5 match and patterns: guarded arms and exhaustiveness.
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
  shape match {
    Circle(radius:) if radius > 0.0 -> 3.14 * radius * radius
    Circle(_)                       -> 0.0
    Rect(w, h)                      -> w * h
    Empty                           -> 0.0
  }
}
"#
    ));
}

/// 6.7 Pipes. The named arguments of `List.fold(from: 0, f: add)` follow the pipe's left side.
#[test]
fn spec_6_7_pipe() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn is_valid(n: Int) -> Bool { n >= 0 }  # 補
fn add(acc: Int, n: Int) -> Int { acc + n }  # 補

fn total(scores: List[Int]) -> Int {
  scores
    |> List.filter(is_valid)
    |> List.fold(from: 0, f: add)
}
"#
    ));
}

/// 9.2 Function types. The function `compose` returns is inferred to have the errors of f and g;
/// compose itself does not fail.
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

/// 9.2 Functions with effects held in declarations make them explicit with type parameters
/// (`type Handler[E, R]`).
#[test]
fn spec_9_2_handler_type_params() {
    insta::assert_snapshot!(render_bodies(
        r#"
type Request(path: String)  # 補
type Response(body: String)  # 補
error NotFound  # 補

type Handler[E, R](run: fn(Request) -> Response fails E use R)

fn serve[E, R](h: Handler[E, R], req: Request) -> Response fails E use R {  # 補
  h.run(req)
}

fn not_found() -> Handler[NotFound, Never] {  # 補．効果の集合の位置に型を書くと E0323
  Handler(run: fn(req) { fail NotFound })
}
"#
    ));
}

/// 9.4 Never becomes the other type where branches merge. If one side of an if panics, the whole
/// has the other side's type.
#[test]
fn spec_9_4_never_joins() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn pick(c: Bool, n: Int) -> Int {
  a = if c { n } else { panic("no") }
  b = c match {
    True -> todo()
    False -> n
  }
  a + b
}
"#
    ));
}

/// 7.1 An error declaration defines a type and constructor of the same name. Error values are
/// ordinary values.
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

/// 7.2 fail and or_fail. A type parameter appearing both as a value and in fails is the type of a
/// single error.
#[test]
fn spec_7_2_fail_and_or_fail() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(id: Int, name: String)  # 補
error NotFound(id: Int)

fn or_fail[A, E](opt: Option[A], err: E) -> A fails E {
  opt match {
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

/// 7.3 escape: recovering, catching only some, converting to another error, treating as a value.
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

/// 7.3 escape and set variables. Nothing is removed from a parameter function's invisible set
/// variable. Writing `fails Timeout | E` removes the first call's Timeout from the type too. The
/// set is decided at the caller.
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

/// 7.4 fails annotations. An omitted return type is inferred; `fails` is exactly that set. For a
/// function passed to a `fails Timeout | E` parameter, its errors other than Timeout become E.
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

/// 13.3, 14.3 The E of test functions and `main` may be non-empty.
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

/// 12.7 Exported functions: parameters and return values of boundary types, E empty.
#[test]
fn spec_12_7_exports() {
    insta::assert_snapshot!(render_bodies(
        r#"
@export(c, "TIM2_IRQHandler")
pub fn on_tick() -> () {}  # 補: the body

@export(js, "default", "add")
pub fn add(a: Int, b: Int) -> Int { a + b }
"#
    ));
}

/// `[api.std.bytes]`: a const of Bytes written with `Bytes.hex`, and the functions of
/// `Std.Bytes` with their types, Eq and Ord.
#[test]
fn spec_api_std_bytes() {
    insta::assert_snapshot!(render_bodies(
        r#"
import Std.Bytes.{Bytes}

const PNG_SIGNATURE = Bytes.hex("89504e47")

fn round_trip() -> Option[String] {
  data = Bytes.append(PNG_SIGNATURE, Bytes.encode_utf8("hi"))
  size = Bytes.size(data)
  first = Bytes.get(data, 0)
  hex = Bytes.to_hex(data)
  parsed = Bytes.from_hex("not hex")
  listed = Bytes.from_list([0x89, 0x50])
  bytes = Bytes.to_list(Bytes.slice(data, 4, 6))
  joined = Bytes.concat([data, Bytes.empty()])
  same = data == PNG_SIGNATURE && data < joined && Bytes.is_empty(joined)
  Bytes.decode_utf8(Bytes.slice(data, 4, 6))
}
"#
    ));
}

// ---------------------------------------------------------------------------
// Code the text declares to be errors

/// 2.4 An integer literal exceeding the range of its context's type. Not reinterpreted as two's
/// complement.
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

/// 5.3 In an update, carried-over fields with conflicting types, or a different type after `..`.
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

/// 6.4 An if used as a value needs else. The body of an if without else is () or Never.
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

/// 6.3, 6.5 Binding patterns must always succeed. match must be exhaustive. Redundant arms warn.
/// Omitting named fields without `..` is an error.
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
  shape match {
    Circle(radius:) if radius > 0.0 -> radius
    Rect(w, h) -> w
    _ -> 0.0
    Empty -> 1.0
  }
}
"#
    ));
}

/// 6.5 A pattern may put `-` before a number literal. The range is checked on the `-` and the
/// digits as one value, so the minimums of Int and Int64 can be written and anything below is
/// E0311. `-0.0` is the same value as `0.0`, also for redundancy.
#[test]
fn spec_6_5_negative_patterns() {
    insta::assert_snapshot!(render_bodies(
        r#"
const MINUS_ONE = -1

fn ints(n: Int) -> Int {
  n match {
    -2147483648 -> 0
    -2147483649 -> 1
    -0x8000_0000 -> 2
    -0x8000_0002 -> 3
    -0b1 -> 4
    -1 -> 5
    MINUS_ONE -> 6
    1 -> 7
    _ -> 8
  }
}

fn longs(n: Int64) -> Int {
  n match {
    -9223372036854775808 -> 0
    -9223372036854775809 -> 1
    -2147483649 -> 2
    _ -> 3
  }
}

fn floats(x: Float) -> Int {
  x match {
    0.0 -> 0
    -0.0 -> 1
    -1.5 -> 2
    -1.5 -> 3
    _ -> 4
  }
}

fn not_exhaustive(n: Int) -> Int {
  n match {
    -1 -> 0
    0 -> 1
  }
}

fn not_a_number(s: String) -> Int {
  s match {
    -1 -> 0
    _ -> 1
  }
}

fn binding(p: (Int, Int)) -> Int {
  (-1, x) = p
  x
}
"#
    ));
}

/// 6.6 Arithmetic operators apply only to Int, Int64 and Float, with both sides of the same type.
/// `+` cannot concatenate strings.
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

/// 9.4 Never stands for other types only at the top level; `List[Never]` and `List[Int]` do not
/// match.
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

/// 8.2 `use X` returns a capability value and puts X in R. Operations are called through the
/// capability value.
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

  user = users.find(id) match {  # 補: Option.or_fail はまだ同梱していない
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

/// 8.2 To use a specific handler's capability, pass a closure capturing it.
/// 8.3 A capability value taken outside still refers to the outer handler even when an inner
/// `with` overrides it.
#[test]
fn spec_8_2_copy() {
    insta::assert_snapshot!(render_bodies(
        r#"
type Rows(n: Int)  # 補

effect Db {  # 補
  fn query(q: String) -> Rows
  fn execute(rows: Rows) -> ()
}

handler PrimaryDb: Db {  # 補
  fn query(q) { Rows(n: 0) }
  fn execute(rows) { () }
}

handler ReplicaDb: Db {  # 補
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

/// 8.3 `with` removes the provided effects from the body's R; handlers can be listed with commas.
/// 8.4 A stateless handler implements every operation of the effect, without parameter types.
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

handler ConsoleLogger: Logger {
  fn info(msg) { console_log(msg) }
}

handler FixedClock: Clock {  # 補
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

/// 8.3 `with` and invisible effect sets. `with` removes nothing from a parameter function's
/// invisible set. Writing `use { Logger, R }` lets `with` remove Logger and passes as
/// `-> () use R`.
#[test]
fn spec_8_3_with_and_set_variables() {
    insta::assert_snapshot!(render_bodies(
        r#"
effect Logger {  # 補
  fn info(msg: String) -> ()
}

effect Clock {  # 補
  fn now() -> Int
}

handler ConsoleLogger: Logger {  # 補
  fn info(msg) { () }
}

fn run_hidden(f: fn() -> ()) { with ConsoleLogger { f() } }  # 補．仕様の `run(f: fn() -> ())`

fn run[R](f: fn() -> () use { Logger, R }) -> () use R {
  with ConsoleLogger { f() }      # R は R だけ。Logger は消える
}

fn callers() {  # 補
  a = run(fn() {
    (use Logger).info("x")
    (use Clock).now()
    ()
  })
  b = run_hidden(fn() { (use Logger).info("y") })
}
"#
    ));
}

/// 8.1 suspend fn operations, 8.4 suspend fn implementations (resume as the last parameter), 8.7
/// Async. SystemClock captures resume in a closure, so it depends on Async, and Async remains in the R
/// of `with SystemClock`. InstantClock, which resumes immediately, does not require Async.
#[test]
fn spec_8_4_suspend_and_async() {
    insta::assert_snapshot!(render_bodies(
        r#"
type Instant(ms: Int)  # 補

effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Instant
}

@external(js, "setTimeout")  # 補
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

@external(js, "Date.now")  # 補
fn date_now() -> Instant

handler SystemClock: Clock {
  suspend fn sleep(ms, resume) {
    set_timeout(ms, fn() { resume(()) })
  }
  fn now() { date_now() }
}

handler InstantClock: Clock {  # 補
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { Instant(ms: 0) }
}

fn nap() {  # 補
  (use Clock).sleep(1000)
  (use Clock).now()
}

fn real() { with SystemClock { nap() } }  # 補

fn instant() { with InstantClock { nap() } }  # 補
"#
    ));
}

/// 9.5 The bound `R: Immediate` on effects. 12.1 `@external suspend fn` requires Async.
#[test]
fn spec_9_5_immediate() {
    insta::assert_snapshot!(render_bodies(
        r#"
type Ref[A](value: A)  # 補

fn update[A, R: Immediate](
  ref: Ref[A],
  f: fn(A) -> A use R,
) -> () use R {
  f(ref.value)  # 補
  ()
}

@external(js, "./http_ffi.mjs", "fetchText")
@external(wasm, "wasi:http/client", "fetch")
suspend fn fetch_text(url: String) -> String

fn bump(r: Ref[Int]) { update(r, fn(n) { n + 1 }) }  # 補
"#
    ));
}

/// 9.2 `fn(A) -> B use {}` accepts only functions without effects. Omitted, it accepts anything,
/// and the effects join the caller's R.
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

/// 7.4 `-> T` forces no errors, and `fails X` is exactly that set. Unhandled errors in the body are
/// E0318.
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

/// 7.2 Only error values can be passed to `fail`. 7.3 `escape` arm patterns are error
/// constructors.
#[test]
fn spec_7_fail_and_escape_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
error NotFound  # 補

fn or_fail[A, E](opt: Option[A], err: E) -> A fails E {  # 補
  opt match {
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

/// 9.2 Only functions that cannot fail can be passed to a `fails {}` parameter (E0321).
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
