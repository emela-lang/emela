use super::super::*;

use crate::contract::operations::AppendMode;

fn is_static(args: &[Operand]) -> bool {
    matches!(
        args,
        [
            Operand::Const(Const::String(_)),
            Operand::Const(Const::String(_))
        ]
    )
}

fn has_empty_constant(args: &[Operand]) -> bool {
    matches ! (args . first () , Some (Operand :: Const (Const :: String (value))) if value . is_empty ())
        || matches ! (args . get (1) , Some (Operand :: Const (Const :: String (value))) if value . is_empty ())
}

pub(in crate::compiler::plan) fn mode(args: &[Operand]) -> AppendMode {
    if is_static(args) {
        AppendMode::Constant
    } else if has_empty_constant(args) {
        AppendMode::AliasInput
    } else {
        AppendMode::Buffered
    }
}

pub(in crate::compiler::plan) fn string_behavior(args: &[Operand]) -> StringBehavior {
    let buffered = args.len() == 2 && mode(args) == AppendMode::Buffered;
    StringBehavior {
        output: if buffered {
            StringOutput::Buffer
        } else {
            StringOutput::None
        },
        reusable_input: if buffered {
            match args.first() {
                Some(Operand::Var(var)) => Some(*var),
                _ => None,
            }
        } else {
            None
        },
        input_use: if has_empty_constant(args) {
            InputUse::Escapes
        } else {
            InputUse::ReadOnly
        },
        ..StringBehavior::default()
    }
}

pub(in crate::compiler::plan) fn string_bound(
    args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    let [left, right] = args else { return None };
    let (left, right) = (bounds.operand(left), bounds.operand(right));
    Some(match mode(args) {
        AppendMode::Constant => left.saturating_add(right),
        AppendMode::AliasInput => left.max(right),
        AppendMode::Buffered => left.saturating_add(right).min(usize::from(bounds.capacity)),
    })
}

pub(in crate::compiler::plan) fn result_ownership(args: &[Operand]) -> ResultOwnership {
    if has_empty_constant(args) {
        ResultOwnership::Borrowed
    } else {
        ResultOwnership::Owned
    }
}

pub(in crate::compiler::plan) fn require_runtime(
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    if args.len() == 2 && mode(args) == AppendMode::Buffered {
        requirements.require(crate::contract::runtime::RuntimeHelper::StringAppend);
        requirements.defects = true;
    }
}
