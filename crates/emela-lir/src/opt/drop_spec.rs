//! Drop specialisation (C3/PR-027): a type-directed drop for values whose payload holds no
//! reference and no finalizer.
//!
//! Rule ID: C3/PR-027 (drop specialisation).
//!
//! Theory: Perceus frees an object when its reference count reaches zero. The generic drop reads
//! the object's descriptor and releases every slot the descriptor marks as a reference, then gives
//! the block back. When the type of the value proves that the descriptor marks no slot and has no
//! finalizer, the same release is a block release: check the count (so static data with `rc == 0`
//! is never touched), give the block back, and walk nothing. The type decides each drop, and the
//! decision is stored per `Stmt::Drop` for the backend.
//!
//! Equation: `drop(v: T)` becomes the block drop when [`has_reference`]`(T)` and
//! [`has_finalizer`]`(T)` are both false; it stays the generic drop otherwise.
//!
//! Preconditions: the module is at [`Stage::RefCounted`](crate::ir::Stage::RefCounted), so every
//! `drop` is explicit, and the pass runs last in [`run_heap_with`](crate::opt::run_heap_with),
//! after the `rc` cleanup and DCE have settled which drops exist.
//!
//! Postconditions: the module is unchanged except for
//! [`Function::drop_kinds`](crate::ir::Function::drop_kinds), which has an entry for exactly the
//! drops this pass specialises. A missing entry means the generic drop.
//!
//! Preserves: every drop point; the order and count of releases, including the release order of
//! `with`; the evidence tables; evaluation order, effects, failure and suspension edges.
//!
//! Deopt: a drop whose value type the analysis cannot place leaves no entry: a type whose payload
//! may hold a reference (a list, a function, an array of references, a tuple or enum with a
//! reference field) and a type with a finalizer (a handler, an evidence table, a cell, a fiber
//! scope, `Any`, an `@external` type).
//!
//! Budget: one walk of each function's statements; O(statements).
//!
//! Measures: [`Stats::drop_specialized`] and [`Stats::drop_deopts`].
//!
//! # Why the field test uses the representation
//!
//! Whether a slot of a tuple, record or array holds a reference is decided by the *slot's*
//! representation, which both backends take from [`repr_of`]: a tuple field, an array element and
//! an enum field are inline scalars only when their representation is not `ref`, and a value held
//! by value is boxed when it is stored in a slot that needs a reference. A field of type `String`
//! is therefore a reference even though a `String` block itself holds none. Recursing into a
//! field's payload would miss that, so [`has_reference`] stops at the outermost constructor of a
//! field and no type is walked twice; a recursive type such as `enum Tree { Node(Tree) }` ends at
//! the first `ref` field.

use emela_core::{Module as CoreModule, Type, TypeId};
use rustc_hash::FxHashMap;

use crate::ir::{Module, Stmt};
use crate::opt::Stats;
use crate::repr::{Repr, repr_of, repr_of_subst};

pub use crate::ir::DropKind;

/// Classifies every `drop` of the module and records the block drops on its functions.
///
/// Preconditions, postconditions, preserved behaviour, deopt, budget and measures: see the module
/// documentation. Running it again recomputes the same entries (it is idempotent).
pub fn run(m: &mut Module) -> Stats {
    let mut stats = Stats::default();
    let ids: Vec<_> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        let mut kinds: FxHashMap<_, _> = FxHashMap::default();
        {
            let f = &m.functions[id];
            for (stmt_id, stmt) in f.stmts.iter() {
                let Stmt::Drop(v) = stmt else {
                    continue;
                };
                match classify(&m.core, f.vars[*v].ty) {
                    DropKind::Block => {
                        kinds.insert(stmt_id, DropKind::Block);
                        stats.drop_specialized += 1;
                    }
                    DropKind::Generic => stats.drop_deopts += 1,
                }
            }
        }
        m.functions[id].drop_kinds = kinds;
    }
    stats
}

/// How the drop of a value of `ty` must release it.
///
/// `Block` only when freeing the object may not read or release any child: no slot of its
/// descriptor is a reference, and it has no finalizer.
pub fn classify(core: &CoreModule, ty: TypeId) -> DropKind {
    if has_reference(core, ty) || has_finalizer(core, ty) {
        DropKind::Generic
    } else {
        DropKind::Block
    }
}

/// Whether the block of a value of `ty` holds a reference in its payload, so that freeing it must
/// release child objects.
///
/// The fields of a tuple, the elements of an array and the fields of an enum variant are tested
/// with the representation of the field ([`repr_of`], [`repr_of_subst`] for a type argument):
/// exactly the slots a backend's descriptor marks as references.
pub fn has_reference(core: &CoreModule, ty: TypeId) -> bool {
    match core.ty(ty) {
        // Scalar payloads, and the two blocks whose bytes are data only (a String and a Bytes are
        // one kind of block with a length and bytes; neither holds a reference).
        Type::Int
        | Type::Int64
        | Type::Float
        | Type::Bool
        | Type::Unit
        | Type::Never
        | Type::String
        | Type::Bytes => false,
        // An array whose elements are references holds them in slots 1 to the length.
        Type::Array(elem) => is_reference(core, *elem),
        Type::Tuple(fields) => fields.iter().any(|&t| is_reference(core, t)),
        Type::Enum(id, args) => {
            let def = &core.enums[*id];
            def.external
                || def
                    .variants
                    .iter()
                    .any(|v| v.tys.iter().any(|&t| is_reference_subst(core, t, args)))
        }
        // Conservative for this first slice: the tail of a list cell is always a reference, a
        // closure record may capture references, and the rest are the finalizer-bearing or
        // untracked types below.
        Type::List(_)
        | Type::Fn(_)
        | Type::Handler(_)
        | Type::Evidence
        | Type::Cell(_)
        | Type::FiberScope
        | Type::Any
        | Type::Param(_)
        | Type::Set(_) => true,
    }
}

/// Whether a value of `ty` has a finalizer the runtime must run when it is freed, so that the
/// block drop cannot be used even if no slot holds a reference.
///
/// A container of such a value is [`has_reference`] already (the field is held as a reference), so
/// this need not look inside one.
pub fn has_finalizer(core: &CoreModule, ty: TypeId) -> bool {
    match core.ty(ty) {
        Type::Handler(_) | Type::Evidence | Type::Cell(_) | Type::FiberScope | Type::Any => true,
        // An `@external type` is a host handle, whose release the host decides.
        Type::Enum(id, _) => core.enums[*id].external,
        _ => false,
    }
}

/// Whether a slot of type `ty` holds a reference (`ref`) rather than inline data.
fn is_reference(core: &CoreModule, ty: TypeId) -> bool {
    repr_of(core, ty) == Repr::Ref
}

/// [`is_reference`] with the type arguments of the enclosing application substituted.
fn is_reference_subst(core: &CoreModule, ty: TypeId, args: &[TypeId]) -> bool {
    repr_of_subst(core, ty, args) == Repr::Ref
}
