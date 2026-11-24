//! Drop specialisation (C3/PR-027): the type-directed block drop.
//!
//! A `drop` whose value's payload holds no reference and whose type has no finalizer is classified
//! as a block release; every other drop stays generic. The pass is off by default, so a default
//! build's Heap IR (and both backends' output) is unchanged.

use crate::support;

use emela_core::{
    Module as CoreModule, Roots, Type, TypeId, monomorphize_from, verify_monomorphic,
};
use emela_lir::opt::drop_spec::{self, DropKind};
use emela_lir::opt::{self, LirRules, Options, Stats};
use emela_lir::{Module, Stmt, core_ty, from_core, to_heap_with_options, verify};
use support::try_lower_with_opt;

/// Lowers (from the exported functions), monomorphizes and converts to the Flow IR, with the Core
/// IR optimisations off so that the values the program writes reach the LIR.
fn flow(src: &str) -> Module {
    let m = try_lower_with_opt(
        &[("main.emel", src)],
        Roots::Exported,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        false,
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let roots: Vec<_> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(id, _)| id)
        .collect();
    let (mono, _) = monomorphize_from(&m, &roots);
    if let Err(errors) = verify_monomorphic(&mono) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("not monomorphic:\n{}", lines.join("\n"));
    }
    from_core(mono).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("conversion errors:\n{}", lines.join("\n"));
    })
}

/// The Heap IR of `src` under `rules`, verified.
fn heap(src: &str, rules: LirRules) -> (Module, Stats) {
    let mut m = flow(src);
    let stats = to_heap_with_options(&mut m, true, Options { rules });
    if let Err(errors) = verify(&m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("Heap IR verifier errors:\n{}", lines.join("\n"));
    }
    (m, stats)
}

/// The rules of these tests: the default passes, with drop specialisation on.
fn with_drop_spec() -> LirRules {
    LirRules {
        drop_spec: true,
        ..LirRules::default()
    }
}

/// The default passes, with drop specialisation off.
fn without_drop_spec() -> LirRules {
    LirRules::default()
}

/// The drops of the module, as (type name, kind). A drop without an entry is `Generic`.
fn drops(m: &Module) -> Vec<(String, DropKind)> {
    let mut out = Vec::new();
    for (_, f) in m.functions.iter() {
        for (stmt, s) in f.stmts.iter() {
            if let Stmt::Drop(v) = s {
                let ty = core_ty(&m.core, f.vars[*v].ty);
                let kind = f
                    .drop_kinds
                    .get(&stmt)
                    .copied()
                    .unwrap_or(DropKind::Generic);
                out.push((ty, kind));
            }
        }
    }
    out
}

/// The kind of every drop of the named type; empty when the program has no drop of it.
fn kinds_of(m: &Module, ty: &str) -> Vec<DropKind> {
    drops(m)
        .into_iter()
        .filter(|(name, _)| name == ty)
        .map(|(_, kind)| kind)
        .collect()
}

/// A program with drops of each kind the classification distinguishes.
const PROGRAM: &str = "\
import Std.Array.{Array}

type Point(x: Int, y: Bool)
type Named(name: String, n: Int)

enum Colour { Red, Green }
enum Tagged { One(x: Int), Two(y: Int64) }
enum WithText { Text(s: String), Empty }

fn main() {
  s = \"hello\"
  a = Array.from_list([1, 2, 3])
  b = Array.from_list([Int64.from_int(1), Int64.from_int(2)])
  t = (1, True)
  p = Point(x: 1, y: True)
  c = Red
  g = One(x: 2)
  h = Two(y: Int64.from_int(3))
  l = [1, 2, 3]
  strs = Array.from_list([\"a\", \"b\"])
  nt = Named(name: \"n\", n: 1)
  wt = Text(s: \"t\")
  dbg(s)
  dbg(Array.size(a) + Array.size(b) + Array.size(strs))
}
";

/// The classification table: a value whose payload has no reference becomes a block drop; a
/// reference field, a list and a finalizer keep the generic drop.
#[test]
fn the_drop_kind_is_directed_by_the_type() {
    let (m, _) = heap(PROGRAM, with_drop_spec());

    // No references in the payload: the block holds data only. A nullary enum has no reference
    // either, but its representation is the tag, so it has no drop.
    for ty in [
        "String",
        "Array[Int]",
        "Array[Int64]",
        "(Int, Bool)",
        "Point",
        "Tagged",
    ] {
        let kinds = kinds_of(&m, ty);
        assert!(!kinds.is_empty(), "the program has no drop of `{ty}`");
        assert!(
            kinds.iter().all(|k| *k == DropKind::Block),
            "`{ty}` has a generic drop: {kinds:?}"
        );
    }

    // A reference field, a list (its tail is a reference), and an array of references stay
    // generic.
    for ty in ["List[Int]", "Array[String]", "Named", "WithText"] {
        let kinds = kinds_of(&m, ty);
        assert!(!kinds.is_empty(), "the program has no drop of `{ty}`");
        assert!(
            kinds.iter().all(|k| *k == DropKind::Generic),
            "`{ty}` was specialised: {kinds:?}"
        );
    }
}

/// The finalizer-bearing and untracked types are generic even when they hold no reference, and a
/// type argument is substituted before a field's representation is read.
#[test]
fn a_finalizer_or_an_untracked_type_stays_generic() {
    let mut core = CoreModule::new();
    let int = TypeId::INT;
    let effect = core
        .effects
        .alloc(emela_core::EffectDef::new("E", Vec::new()));
    let handler = core.handler(effect);
    let cell = core.cell(int);
    let list = core.list(int);
    let function = core.fn_type(vec![int], int);
    let tuple = core.tuple(vec![int, TypeId::BOOL]);
    let tuple_with_text = core.tuple(vec![int, TypeId::STRING]);
    let arr_int = core.intern(Type::Array(int));
    let arr_text = core.intern(Type::Array(TypeId::STRING));
    let point = {
        let id = core.enums.alloc(emela_core::EnumDef {
            name: "Point".into(),
            params: 0,
            variants: vec![emela_core::VariantDef::positional(
                "Point",
                vec![int, TypeId::BOOL],
            )],
            error: false,
            external: false,
        });
        core.enum_type(id, Vec::new())
    };
    let (boxed_int, boxed_text) = {
        let param = core.type_param(0);
        let id = core.enums.alloc(emela_core::EnumDef {
            name: "Box".into(),
            params: 1,
            variants: vec![emela_core::VariantDef::positional("Box", vec![param])],
            error: false,
            external: false,
        });
        (
            core.enum_type(id, vec![int]),
            core.enum_type(id, vec![TypeId::STRING]),
        )
    };

    for ty in [
        handler,
        TypeId::EVIDENCE,
        cell,
        TypeId::FIBER_SCOPE,
        TypeId::ANY,
        list,
        function,
    ] {
        assert_eq!(
            drop_spec::classify(&core, ty),
            DropKind::Generic,
            "{:?} must keep the generic drop",
            core.ty(ty)
        );
    }
    // A field held as a reference keeps the parent generic, including an instantiated type
    // argument.
    for ty in [tuple_with_text, arr_text, boxed_text] {
        assert_eq!(
            drop_spec::classify(&core, ty),
            DropKind::Generic,
            "{:?} holds a reference field",
            core.ty(ty)
        );
    }
    for ty in [
        tuple,
        point,
        arr_int,
        boxed_int,
        TypeId::STRING,
        TypeId::BYTES,
    ] {
        assert_eq!(
            drop_spec::classify(&core, ty),
            DropKind::Block,
            "{:?} holds no reference",
            core.ty(ty)
        );
    }
}

/// The flag is off by default: no function carries a kind and the dump is what it was without the
/// pass. The counters stay zero.
#[test]
fn the_flag_off_leaves_the_heap_ir_and_the_counters_alone() {
    let (m, stats) = heap(PROGRAM, without_drop_spec());
    assert!(
        m.functions.iter().all(|(_, f)| f.drop_kinds.is_empty()),
        "a default build carries a drop kind"
    );
    assert_eq!(stats.drop_specialized, 0);
    assert_eq!(stats.drop_deopts, 0);

    // A module the pass is not run on is the same as one where it is switched off: `run_heap`
    // uses the default rules.
    let (mut off, _) = heap(PROGRAM, without_drop_spec());
    let stats = emela_lir::opt::run_heap(&mut off, true);
    assert!(
        off.functions.iter().all(|(_, f)| f.drop_kinds.is_empty()),
        "`run_heap` specialised a drop"
    );
    assert_eq!(stats.drop_specialized, 0);
    assert_eq!(stats.drop_deopts, 0);
}

/// The classification is not a rewrite: the dump is the same with the flag on and off.
#[test]
fn specialising_a_drop_does_not_change_the_statements() {
    let (off, _) = heap(PROGRAM, without_drop_spec());
    let (on, stats) = heap(PROGRAM, with_drop_spec());
    assert_eq!(emela_lir::dump(&off), emela_lir::dump(&on));
    assert!(stats.drop_specialized > 0, "the pass specialised nothing");
    assert!(stats.drop_deopts > 0, "the pass left nothing generic");
    assert_eq!(
        stats.drop_specialized as usize + stats.drop_deopts as usize,
        drops(&on).len()
    );
}

/// Running the pass again gives the same kinds and no stale entries.
#[test]
fn the_pass_is_idempotent() {
    let (mut m, _) = heap(PROGRAM, with_drop_spec());
    let before = drops(&m);
    let first = drop_spec::run(&mut m);
    let second = drop_spec::run(&mut m);
    assert_eq!(drops(&m), before);
    assert_eq!(first.drop_specialized, second.drop_specialized);
    assert_eq!(first.drop_deopts, second.drop_deopts);
    assert!(verify(&m).is_ok());
}

/// The borrow signature is untouched and no other counter moves: the pass only classifies drops.
#[test]
fn the_pass_writes_no_other_stat() {
    let (_, stats) = heap(PROGRAM, with_drop_spec());
    let default = opt::Stats::default();
    assert_eq!(stats.sra_tuples, default.sra_tuples);
    assert_eq!(stats.sra_records, default.sra_records);
    assert_eq!(stats.devirt, default.devirt);
    assert_eq!(stats.borrow_functions, default.borrow_functions);
}
