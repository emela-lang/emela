//! Sets as type arguments of declarations.

use super::render_effects;

/// Declaration type parameters used in fails and use (`type Handler[E, R]` in spec 9.2). The kind
/// of a type parameter is decided by where it is used. A type argument position takes one tag of
/// the right kind or a set type parameter. Constructor calls infer E and R from the field
/// functions, and calling a field function puts its E and R on the caller.
#[test]
fn declaration_sets() {
    insta::assert_snapshot!(render_effects(
        r#"
type Request(path: String)
type Response(body: String)

error NotFound
error Timeout

effect Db {
  fn query(q: String) -> String
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

type Handler[E, R](run: fn(Request) -> Response fails E uses { R })

enum Step[E] {
  Done,
  Next(fn() -> Int fails E),
}

type Server[E](inner: Handler[E, Db])

type Ticker[R: Immediate](tick: fn() -> () uses { R })

handler MemDb: Db {
  fn query(q) { q }
}

fn db_handler() {
  Handler(run: |req| {
    body = (use Db).query(req.path)
    Response(body:)
  })
}

fn failing() -> Handler[NotFound, Db] {
  Handler(run: |req| fail NotFound)
}

fn serve[E, R](h: Handler[E, R], req: Request) -> Response fails E uses { R } {
  h.run(req)
}

fn served() -> Response {
  with MemDb {
    serve(failing(), Request(path: "/")) escape {
      NotFound -> Response(body: "404")
    }
  }
}

fn inferred(req: Request) {
  serve(db_handler(), req)
}

fn too_much() -> Handler[NotFound, Db] {
  Handler(run: |req| {
    (use Clock).sleep(1)
    fail Timeout
  })
}

fn bad_args(a: Handler[Int, Db], b: Handler[NotFound, NotFound], c: List[Db]) { () }

fn steps(s: Step[Timeout]) -> Int fails Timeout {
  s match {
    Done -> 0
    Next(f) -> f()
  }
}

fn server(s: Server[NotFound], req: Request) -> Response fails NotFound uses { Db } {
  s.inner.run(req)
}

fn tickers() {
  a = Ticker(tick: || {
    (use Db).query("x")
    ()
  })
  b = Ticker(tick: || (use Clock).sleep(1))
}
"#
    ));
}

/// Sets written in type argument positions (spec 9.2). Error sets are joined with `|`; effect sets
/// are wrapped in braces. An empty `{}` fits either position. A set in a non-set type argument
/// position is E3008.
#[test]
fn declaration_set_args() {
    insta::assert_snapshot!(render_effects(
        r#"
type Request(path: String)
type Response(body: String)

error NotFound
error Timeout

effect Db {
  fn query(q: String) -> String
}

effect Log {
  fn info(msg: String) -> ()
}

type Handler[E, R](run: fn(Request) -> Response fails E uses { R })

fn both() -> Handler[NotFound | Timeout, { Db, Log }] {
  Handler(run: |req| {
    (use Log).info(req.path)
    if req.path == "/" { fail Timeout }
    body = (use Db).query(req.path)
    if body == "" { fail NotFound }
    Response(body:)
  })
}

fn pure() -> Handler[{}, {}] {
  Handler(run: |req| Response(body: req.path))
}

fn too_much() -> Handler[{}, {}] {
  Handler(run: |req| fail NotFound)
}

fn wrap[E, R](h: Handler[E, R]) -> Handler[E | Timeout, { R, Log }] {
  Handler(run: |req| {
    (use Log).info("wrap")
    h.run(req)
  })
}

fn misplaced(a: Handler[{ Db }, Db], b: Handler[NotFound, NotFound | Timeout], c: List[{}], d: Handler[NotFound | Db, {}]) { () }
"#
    ));
}
