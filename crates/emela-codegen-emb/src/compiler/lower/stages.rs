use crate::{Error, ErrorKind};
pub(in crate::compiler::lower) fn flow() -> Result<(), Vec<Error>> {
    Ok(())
}
pub(in crate::compiler::lower) fn closures_converted() -> Result<(), Vec<Error>> {
    Err(vec![Error{kind:ErrorKind::InvalidFlowIr,message:"expected Flow IR, found ClosuresConverted; embedded codegen does not run Heap IR passes".into()}])
}
pub(in crate::compiler::lower) fn defects_explicit() -> Result<(), Vec<Error>> {
    Err(vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message:
            "expected Flow IR, found DefectsExplicit; embedded codegen does not run Heap IR passes"
                .into(),
    }])
}
pub(in crate::compiler::lower) fn state_machines() -> Result<(), Vec<Error>> {
    Err(vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message:
            "expected Flow IR, found StateMachines; embedded codegen does not run Heap IR passes"
                .into(),
    }])
}
pub(in crate::compiler::lower) fn ref_counted() -> Result<(), Vec<Error>> {
    Err(vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message: "expected Flow IR, found RefCounted; embedded codegen does not run Heap IR passes"
            .into(),
    }])
}
pub(in crate::compiler::lower) fn verify(stage: emela_lir::Stage) -> Result<(), Vec<Error>> {
    match stage {
        emela_lir::Stage::Flow => flow(),
        emela_lir::Stage::ClosuresConverted => closures_converted(),
        emela_lir::Stage::DefectsExplicit => defects_explicit(),
        emela_lir::Stage::StateMachines => state_machines(),
        emela_lir::Stage::RefCounted => ref_counted(),
    }
}
