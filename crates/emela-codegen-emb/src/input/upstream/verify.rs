//! Pure upstream validation shared by input adaptation and lower.
use crate::{Error, ErrorKind};

/// Verifies that a LIR module is still in the Flow stage and satisfies its invariants.
pub fn verify_flow(flow: &emela_lir::Module) -> Result<(), Vec<Error>> {
    if flow.stage != emela_lir::Stage::Flow {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "expected Flow IR, found {:?}; embedded codegen does not run Heap IR passes",
                flow.stage
            ),
        }]);
    }

    emela_lir::verify(flow).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidFlowIr,
                message: format!("invalid Flow IR: {error}"),
            })
            .collect::<Vec<_>>()
    })?;

    // The shared verifier checks representations but does not reject every operation that
    // belongs to a later stage or that from_core must already have lowered into Flow control.
    let mut errors = Vec::new();
    for (_, function) in flow.functions.iter() {
        for (_, statement) in function.stmts.iter() {
            let emela_lir::Stmt::Let { value, .. } = statement else {
                continue;
            };
            let reason = match value {
                emela_lir::Value::Runtime {
                    op: emela_lir::RuntimeOp::DefectAt,
                    ..
                } => Some(reject_defect_at()),
                emela_lir::Value::Builtin {
                    op: emela_core::Builtin::AssertFail,
                    ..
                } => Some(reject_assert_fail()),
                emela_lir::Value::Runtime {
                    op:
                        op @ (emela_lir::RuntimeOp::Defect
                        | emela_lir::RuntimeOp::DefectTake
                        | emela_lir::RuntimeOp::DefectResume
                        | emela_lir::RuntimeOp::Yield
                        | emela_lir::RuntimeOp::YieldPerform),
                    ..
                } => Some(format!("Heap IR runtime `{}` at stage Flow", op.name())),
                emela_lir::Value::Builtin {
                    op:
                        op @ (emela_core::Builtin::Panic
                        | emela_core::Builtin::Todo
                        | emela_core::Builtin::Dbg
                        | emela_core::Builtin::Inspect
                        | emela_core::Builtin::ShowNested
                        | emela_core::Builtin::ShowString
                        | emela_core::Builtin::DebugPrint),
                    ..
                } => Some(format!(
                    "unlowered builtin `{}` at stage Flow",
                    op.info().path()
                )),
                _ => None,
            };
            if let Some(reason) = reason {
                errors.push(Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!("invalid Flow IR in function `{}`: {reason}", function.name),
                });
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn reject_defect_at() -> String {
    format!(
        "Heap IR runtime `{}` at stage Flow",
        emela_lir::RuntimeOp::DefectAt.name()
    )
}

fn reject_assert_fail() -> String {
    format!(
        "unlowered builtin `{}` at stage Flow",
        emela_core::Builtin::AssertFail.info().path()
    )
}
