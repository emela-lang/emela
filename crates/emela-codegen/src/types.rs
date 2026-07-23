//! The shared Emela type-system types referenced by the IR.
//!
//! These live in `emela-codegen` (not the frontend AST) because they are part
//! of the IR contract: backends and external plugins reason about them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Type {
    Unit,
    Bool,
    Int,
    Float,
    String,
    /// A single Unicode scalar value (spec 0017).
    Char,
    /// An immutable byte sequence (spec 0051). Shares `String`'s `[len][bytes]`
    /// representation, but counts and indexes in bytes and carries no UTF-8
    /// interpretation.
    Bytes,
    Array(Box<Type>),
    Record,
    /// A named enum type (spec 0005), identified by its declared name and its
    /// type arguments (spec 0028). The argument list is empty for a
    /// non-generic enum such as `Color`, and holds one type per declared type
    /// parameter for a generic one such as `List<Int>` or `Either<Int, String>`.
    /// `Option<T>` (spec 0042) is one of these — an ordinary Core-Prelude enum,
    /// not a dedicated variant.
    Enum(String, Vec<Type>),
    /// The empty type of `throw` and `panic` (spec 0011). It is assignable to
    /// any expected type; no value ever has this type.
    Never,
    Function(FunctionType),
    OpaqueFunction,
    /// A generic function's type parameter (spec 0014), e.g. `T`. It only ever
    /// appears in the frontend (function signatures and the AST while checking a
    /// generic body); monomorphization substitutes it for a concrete type before
    /// lowering, so it never reaches the typed IR or a backend.
    Var(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionType {
    pub params: Vec<Type>,
    pub ret: Box<Type>,
    /// The error type the function may throw (spec 0008/0011), if any. `None`
    /// is a non-throwing function. It is part of the type: two functions that
    /// differ only in `throws` are different types.
    #[serde(default)]
    pub throws: Option<Box<Type>>,
    pub effects: EffectRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// String concatenation `++` (spec 0017).
    Concat,
    Eq,
    Lt,
    /// Derived comparisons (spec 0027). The frontend desugars these to `Eq.eq` /
    /// `Ord.lt`, so a lowered IR never carries them; they exist so the surface
    /// operator survives type checking with a faithful error message.
    Ne,
    Gt,
    Le,
    Ge,
    /// Bitwise operators (spec 0053), each an operator trait like `+` (spec
    /// 0020): `BitAnd`/`BitOr`/`BitXor` and the shifts `Shl` (left), `Shr`
    /// (arithmetic right), `UShr` (logical right). Like the other operators they
    /// desugar through their trait's impl to an intrinsic, so a lowered IR never
    /// carries them as a `Binary` node.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct EffectRow {
    pub effects: Vec<String>,
    /// Row-variable tails of an open row (spec 0022): the `e` of `uses e` /
    /// `uses { Io, ..e }`, sorted and deduplicated like `effects`. Empty for a
    /// closed row. Lowering erases rows to their concrete part (`concrete`), so
    /// a lowered IR never carries tails — which also keeps the serialized form
    /// identical to the pre-0022 one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tails: Vec<String>,
}

impl EffectRow {
    pub fn sorted(mut effects: Vec<String>) -> Self {
        effects.sort();
        effects.dedup();
        Self {
            effects,
            tails: Vec::new(),
        }
    }

    /// A normalized row with row-variable tails (spec 0022).
    pub fn open(effects: Vec<String>, mut tails: Vec<String>) -> Self {
        let mut row = Self::sorted(effects);
        tails.sort();
        tails.dedup();
        row.tails = tails;
        row
    }

    pub fn union(&mut self, other: &EffectRow) {
        self.effects.extend(other.effects.iter().cloned());
        self.effects.sort();
        self.effects.dedup();
        self.tails.extend(other.tails.iter().cloned());
        self.tails.sort();
        self.tails.dedup();
    }

    /// Component-wise subset (spec 0023): row variables are universally
    /// quantified, so `{C1, ..T1} ⊆ {C2, ..T2}` holds exactly when `C1 ⊆ C2`
    /// and `T1 ⊆ T2`.
    pub fn is_subset_of(&self, other: &EffectRow) -> bool {
        self.effects
            .iter()
            .all(|effect| other.effects.contains(effect))
            && self.tails.iter().all(|tail| other.tails.contains(tail))
    }

    /// The concrete part of the row, with row-variable tails erased (spec 0022):
    /// the form lowering writes into the IR.
    pub fn concrete(&self) -> EffectRow {
        EffectRow {
            effects: self.effects.clone(),
            tails: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EffectRow;

    /// A pre-0022 IR (no `tails` key) still deserializes, and a closed row
    /// serializes back to the identical shape.
    #[test]
    fn effect_row_serde_compat() {
        let row: EffectRow = serde_json::from_str(r#"{"effects":["Io"]}"#).unwrap();
        assert_eq!(row.effects, vec!["Io"]);
        assert!(row.tails.is_empty());
        assert_eq!(
            serde_json::to_string(&row).unwrap(),
            r#"{"effects":["Io"]}"#
        );
    }

    #[test]
    fn effect_row_open_normalizes_and_subsets() {
        let open = EffectRow::open(vec!["Io".into()], vec!["e".into(), "e".into()]);
        assert_eq!(open.tails, vec!["e"]);
        let wider = EffectRow::open(vec!["Io".into(), "Log".into()], vec!["e".into()]);
        assert!(open.is_subset_of(&wider));
        assert!(!wider.is_subset_of(&open));
        // A tail is never implied by a wider concrete row.
        assert!(!open.is_subset_of(&EffectRow::sorted(vec!["Io".into(), "Log".into()])));
        assert!(open.concrete().tails.is_empty());
    }
}
