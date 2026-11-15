//! Strings built from pieces: interpolation and assert messages, concatenated with
//! `String.append`.
//!
//! Two lowerings of the syntax are here, both known from the syntax alone (no analysis):
//!
//! - Adjacent texts are merged into one, and empty strings are dropped ([`normalize`]).
//! - The chain associates to the right ([`chain`]): `a + (b + c)` rather than `(a + b) + c`. A
//!   String is immutable and append is associative, so the value is the same. Interpolation
//!   usually puts the string it grows first (a fold's accumulator) and short suffixes after it,
//!   and the right chain then copies that first piece once instead of once per later piece. The
//!   operands are values only (constants and variables), so no evaluation order changes.

use emela_core::{Builtin, TypeId};

use super::Lower;
use crate::ir::{Const, Operand, Value};

/// A piece of a string to build.
pub(super) enum Piece {
    Text(String),
    Op(Operand),
}

/// The operands of the appends: adjacent texts merged into one and empty ones dropped, in order.
/// A constant String operand counts as text, so it merges too.
pub(super) fn normalize(pieces: Vec<Piece>) -> Vec<Operand> {
    let mut ops = Vec::new();
    let mut text: Option<String> = None;
    for p in pieces {
        match p {
            Piece::Text(t) | Piece::Op(Operand::Const(Const::String(t))) if t.is_empty() => {}
            Piece::Text(t) | Piece::Op(Operand::Const(Const::String(t))) => {
                text.get_or_insert_with(String::new).push_str(&t);
            }
            Piece::Op(o) => {
                if let Some(t) = text.take() {
                    ops.push(Operand::Const(Const::String(t)));
                }
                ops.push(o);
            }
        }
    }
    if let Some(t) = text {
        ops.push(Operand::Const(Const::String(t)));
    }
    ops
}

/// Folds the operands with `append` so that the chain associates to the right: the rightmost pair
/// is appended first and the leftmost operand is the outermost append. Without operands, the empty
/// string.
pub(super) fn chain(
    ops: Vec<Operand>,
    mut append: impl FnMut(Operand, Operand) -> Operand,
) -> Operand {
    let mut ops = ops.into_iter().rev();
    let Some(mut acc) = ops.next() else {
        return Operand::Const(Const::String(String::new()));
    };
    for o in ops {
        acc = append(o, acc);
    }
    acc
}

/// The string of `pieces`: `String.append` in a right-associated chain ([`chain`]).
pub(super) fn build(lower: &mut Lower<'_>, pieces: Vec<Piece>) -> Operand {
    chain(normalize(pieces), |left, right| {
        Operand::Var(lower.assign(
            "s",
            TypeId::STRING,
            Value::Builtin {
                op: Builtin::StringAppend,
                args: vec![left, right],
            },
        ))
    })
}

#[cfg(test)]
mod tests {
    use emela_core::{Builtin, TypeId};
    use la_arena::Arena;
    use rustc_hash::{FxHashMap, FxHashSet};

    use super::{Piece, build, chain, normalize};
    use crate::from_core::{Lower, Shared};
    use crate::ir::{Const, Operand, Stmt, Value};

    fn shared() -> Shared {
        Shared {
            core: emela_core::Module::new(),
            functions: Arena::new(),
            fn_map: Vec::new(),
            names: FxHashSet::default(),
            derived: FxHashMap::default(),
            pending: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn text(s: &str) -> Piece {
        Piece::Text(s.to_owned())
    }

    fn const_str(s: &str) -> Operand {
        Operand::Const(Const::String(s.to_owned()))
    }

    fn const_int(n: i32) -> Operand {
        Operand::Const(Const::Int(n))
    }

    /// A string variable of `lower`.
    fn var(lower: &mut Lower<'_>, hint: &str) -> Operand {
        Operand::Var(lower.var(hint, TypeId::STRING))
    }

    /// The pairs of operands of the `String.append` statements of `lower`, in creation order.
    fn appends(lower: &Lower<'_>) -> Vec<(Operand, Operand)> {
        lower
            .f
            .stmts
            .iter()
            .filter_map(|(_, stmt)| match stmt {
                Stmt::Let {
                    value:
                        Value::Builtin {
                            op: Builtin::StringAppend,
                            args,
                        },
                    ..
                } => Some((args[0].clone(), args[1].clone())),
                _ => None,
            })
            .collect()
    }

    /// The destination of each `String.append` statement of `lower`, in creation order.
    fn append_dsts(lower: &Lower<'_>) -> Vec<Operand> {
        lower
            .f
            .stmts
            .iter()
            .filter_map(|(_, stmt)| match stmt {
                Stmt::Let {
                    dst: Some(dst),
                    value:
                        Value::Builtin {
                            op: Builtin::StringAppend,
                            ..
                        },
                    ..
                } => Some(Operand::Var(*dst)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn adjacent_texts_merge() {
        let mut sh = shared();
        let mut lower = Lower::new(&mut sh, "test");
        let v = var(&mut lower, "v");
        assert_eq!(
            normalize(vec![text("a"), text("b"), Piece::Op(v.clone()), text("c")]),
            vec![const_str("ab"), v, const_str("c")]
        );
    }

    #[test]
    fn a_constant_string_merges_like_text() {
        assert_eq!(
            normalize(vec![text("a"), Piece::Op(const_str("b"))]),
            vec![const_str("ab")]
        );
    }

    #[test]
    fn empty_texts_are_dropped() {
        assert_eq!(
            normalize(vec![
                text(""),
                Piece::Op(const_str("")),
                text("a"),
                text(""),
                Piece::Op(const_str("")),
                text("b"),
            ]),
            vec![const_str("ab")]
        );
    }

    #[test]
    fn all_empty_is_the_empty_string() {
        let ops = normalize(vec![
            text(""),
            Piece::Op(const_str("")),
            Piece::Op(const_str("")),
        ]);
        assert!(ops.is_empty());
        assert_eq!(
            chain(ops, |_, _| panic!("no append without operands")),
            const_str("")
        );
    }

    #[test]
    fn a_single_operand_is_itself() {
        let mut sh = shared();
        let mut lower = Lower::new(&mut sh, "test");
        let v = var(&mut lower, "v");
        assert_eq!(chain(vec![v.clone()], |_, _| panic!("no append")), v);
    }

    #[test]
    fn chain_associates_to_the_right() {
        let mut seen = Vec::new();
        let result = chain(
            vec![const_int(1), const_int(2), const_int(3)],
            |left, right| {
                seen.push((left, right));
                const_int(10 + seen.len() as i32)
            },
        );
        // `1 + (2 + 3)`: the rightmost pair first, then the left operand with that result.
        assert_eq!(
            seen,
            vec![(const_int(2), const_int(3)), (const_int(1), const_int(11))]
        );
        assert_eq!(result, const_int(12));
    }

    /// An assert with two sides: the texts and the shown sides in a right-associated chain, the
    /// leading text outermost.
    #[test]
    fn assert_message_is_a_right_associated_chain() {
        let mut sh = shared();
        let mut lower = Lower::new(&mut sh, "test");
        let left = var(&mut lower, "left");
        let right = var(&mut lower, "right");
        let head = "assertion failed: left == right\n  left: ";
        let msg = build(
            &mut lower,
            vec![
                text(head),
                Piece::Op(left.clone()),
                text("\n right: "),
                Piece::Op(right.clone()),
            ],
        );
        let appends = appends(&lower);
        let dsts = append_dsts(&lower);
        assert_eq!(appends.len(), 3);
        assert_eq!(
            appends,
            vec![
                (const_str("\n right: "), right),
                (left, dsts[0].clone()),
                (const_str(head), dsts[1].clone()),
            ]
        );
        assert_eq!(msg, dsts[2]);
    }
}
