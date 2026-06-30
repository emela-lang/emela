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
    Array(Box<Type>),
    Record,
    Enum,
    Function(FunctionType),
    OpaqueFunction,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionType {
    pub params: Vec<Type>,
    pub ret: Box<Type>,
    pub effects: EffectRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Eq,
    Lt,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct EffectRow {
    pub effects: Vec<String>,
}

impl EffectRow {
    pub fn sorted(mut effects: Vec<String>) -> Self {
        effects.sort();
        effects.dedup();
        Self { effects }
    }

    pub fn union(&mut self, other: &EffectRow) {
        self.effects.extend(other.effects.iter().cloned());
        self.effects.sort();
        self.effects.dedup();
    }

    pub fn is_subset_of(&self, other: &EffectRow) -> bool {
        self.effects
            .iter()
            .all(|effect| other.effects.contains(effect))
    }
}
