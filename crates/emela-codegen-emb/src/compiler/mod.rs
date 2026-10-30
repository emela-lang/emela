//! Pure stage orchestration. Stages exchange only independent contracts.
mod emit;
mod lower;
mod plan;

#[cfg(test)]
#[path = "tests/separation.rs"]
mod separation_tests;
#[cfg(test)]
#[path = "../../tests/support/frontend.rs"]
pub(super) mod support;

pub(crate) fn compile_flow(
    input: &emela_lir::Module,
    options: &crate::Options,
) -> Result<crate::Output, Vec<crate::Error>> {
    let lowered = lower::lower(input)?;
    let planned = plan::prepare(&lowered, options)?;
    emit::render(&planned)
}
