//! Built-in handlers with the name of their effect: `Deferred`, `Queue` and `Semaphore`
//! (`[task.deferred]`, `[task.queue]`, `[task.semaphore]`, `[module.resolution]`). Checks that
//! the name means the handler at an item of a with, a layer and the right of `by`, and the effect
//! elsewhere; that the type arguments of an item are inferred or written; that the handler members
//! `Permits` and `TryPermits` open a `Permit`; and that no other handler implements these effects
//! (E4003) or takes the name of an effect (E2103).
//!
//! The output is that of `generic_handler.rs`.

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

/// `with d = Deferred` makes a Deferred whose type argument is inferred from the body, and the
/// capability's operations have the types of `[task.deferred]`. Written type arguments are kept.
#[test]
fn deferred_item() {
    insta::assert_snapshot!(render(
        r#"
import Std.Deferred.{Deferred}

fn filled() -> Int uses { Async } {
  with d = Deferred {
    first = d.complete(1)
    second = d.complete(2)
    polled = d.poll()
    d.await()
  }
}

fn written() -> Bool {
  with d = Deferred[String] { d.complete("a") }
}

fn passed(d: use Deferred[Int]) -> Option[Int] { d.poll() }

fn through() -> Option[Int] {
  with d = Deferred { passed(d) }
}
"#
    ));
}

/// A Deferred whose type argument nothing determines is E4107 (`[effect.with]`).
#[test]
fn deferred_needs_its_type() {
    insta::assert_snapshot!(render(
        r#"
import Std.Deferred.{Deferred}

fn unknown() -> () {
  with d = Deferred { () }
}
"#
    ));
}

/// `with q = Queue(capacity: c)` (`[task.queue]`): the arguments are those of the init, the type
/// argument is inferred or written, and `offer` and `take` fail with `QueueClosed`.
#[test]
fn queue_item() {
    insta::assert_snapshot!(render(
        r#"
import Std.Queue.{Queue, QueueClosed}

type Job(id: Int)

fn produce() -> Option[Job] fails QueueClosed uses { Async } {
  with q = Queue(capacity: 2) {
    q.offer(Job(id: 1))
    sent = q.try_offer(Job(id: 2))
    n = q.size()
    taken = q.take()
    q.close()
    q.poll()
  }
}

fn written() -> Int {
  with q = Queue[Job](capacity: 100) { q.size() }
}

fn drained() -> () uses { Async } {
  with q = Queue[Int](capacity: 1) {
    q.take() escape {
      QueueClosed -> 0
    }
    ()
  }
}

fn bad_capacity() -> () {
  with q = Queue[Int](capacity: "ten") { () }
}
"#
    ));
}

/// `with s = Semaphore(permits: p)` (`[task.semaphore]`): `s.Permits(n:)` and `s.TryPermits(n:)`
/// open a `Permit`, `Permits` may suspend, `TryPermits` fails with `NoPermits`, and
/// `with_permits` and `try_with_permits` are plain functions with the E and R of `f`.
#[test]
fn semaphore_item() {
    insta::assert_snapshot!(render(
        r#"
import Std.Semaphore.{Semaphore, NoPermits}

error Boom

fn boom() -> Int fails Boom { fail Boom }

fn held() -> Int uses { Async } {
  with s = Semaphore(permits: 3) {
    with p = s.Permits(n: 2) { p.count() + s.available() }
  }
}

fn tried() -> Int fails NoPermits {
  with s = Semaphore(permits: 1) {
    with p = s.TryPermits(n: 1) { p.count() }
  }
}

fn wrapped() -> (Int, Option[Int]) fails Boom uses { Async } {
  with s = Semaphore(permits: 1) {
    (Semaphore.with_permit(s, || boom()), Semaphore.try_with_permits(s, 1, || 1))
  }
}
"#
    ));
}

/// The name of a built-in handler means the effect in a uses clause, `use E`, `x <- E` and the
/// type of a parameter, and the handler at an item of a with and of a layer
/// (`[module.resolution]`). A label cannot stand for a built-in effect (E4201, `[effect.label]`),
/// so the right of `by` never meets one.
#[test]
fn the_name_means_the_handler_only_where_a_handler_is_written() {
    insta::assert_snapshot!(render(
        r#"
import Std.Queue.{Queue}

layer Fresh {
  Queue[String](capacity: 1)
}

fn named() -> Int uses { Queue[Int] } {
  q <- Queue
  (use Queue[Int]).size() + q.size()
}

fn given(q: use Queue[Int]) -> Int { q.size() }

fn provided() -> Int {
  with Queue[Int](capacity: 2) {
    named() + given(use Queue[Int])
  }
}

fn layered() -> Int {
  with Fresh {
    q <- Queue[String]
    q.size()
  }
}
"#
    ));
}

/// Only the built-in handlers provide Deferred, Queue, Semaphore and Permit: a handler written
/// for one is E4003, naming the handler that provides it (`[task.deferred]` to `[task.queue]`).
#[test]
fn no_handler_for_builtin_effects() {
    insta::assert_snapshot!(render(
        r#"
import Std.Deferred.{Deferred}
import Std.Queue.{Queue}
import Std.Semaphore.{Semaphore, Permit}

handler MyDeferred: Deferred[Int] {
  fn complete(value) { True }
  suspend fn await(resume) { resume(1) }
  fn poll() { None }
}

handler MyQueue: Queue[Int] {
  suspend fn offer(value, resume) { resume(()) }
  suspend fn take(resume) { resume(1) }
  fn try_offer(value) { True }
  fn poll() { None }
  fn size() { 0 }
  fn close() { () }
}

handler MySemaphore: Semaphore {
  fn available() { 0 }
}

handler MyPermit: Permit {
  fn count() { 1 }
}
"#
    ));
}

/// A handler written by a user with the name of an effect in the same module is still a
/// duplicate (E2103), and `@intrinsic` is only for the bundled sources (E2201).
#[test]
fn users_cannot_share_the_name_of_an_effect() {
    insta::assert_snapshot!(render(
        r#"
effect Counter {
  fn next() -> Int
}

handler Counter: Counter {
  fn next() { 1 }
}

effect Gauge {
  fn read() -> Int
}

@intrinsic
handler Gauge: Gauge
"#
    ));
}
