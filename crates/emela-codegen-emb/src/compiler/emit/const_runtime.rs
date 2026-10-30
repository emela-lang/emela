//! Lazy const preparation and heap-independent diagnostic storage.

use crate::{Error, Options, writer::Writer};

pub(crate) fn emit_runtime(
    emergency: usize,
    options: &Options,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    writer.line(&format!(
        "#define EMELA_CONST_DIAGNOSTIC_BYTES UINT32_C({})",
        options.const_diagnostic_bytes()
    ));
    writer.line(&format!(
        "#define EMELA_CONST_EMERGENCY_BYTES {}",
        emergency.max(1)
    ));
    writer.line(include_str!("runtime/const.c"));
    Ok(())
}
