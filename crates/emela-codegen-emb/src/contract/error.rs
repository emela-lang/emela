//! Diagnostics shared by input adaptation and code generation.

/// The stage that failed while compiling Flow IR to embedded C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The Core module could not be verified after monomorphization.
    InvalidCore,
    /// Core IR could not be converted to Flow IR.
    FlowLowering,
    /// The Flow IR module was not valid for the Flow stage.
    InvalidFlowIr,
    /// A valid Flow IR construct is not implemented by this backend yet.
    Unsupported,
    /// Backend options are outside their documented range.
    InvalidOptions,
    /// Generated files do not satisfy the C artifact contract.
    InvalidOutput,
}

/// An error encountered while preparing a module for embedded code generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
