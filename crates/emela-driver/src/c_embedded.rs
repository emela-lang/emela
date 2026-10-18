//! The embedded C stage: writes C source. Compiling and linking it is up to the board's build.

use emela_codegen_c_embedded::{Options, compile_with};
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;
use crate::output::{BuildOutput, OutputFile};
use crate::pipeline::{Analysis, Backend};

/// Embedded C output with the board headers, configured by `[embedded.*]` of `Pome.toml`
/// ([`crate::embedded_options`]).
#[derive(Debug, Default, Clone)]
pub struct EmbeddedC {
    pub options: Options,
}

impl Backend<emela_core::Module> for EmbeddedC {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        _: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        let output = compile_with(module, &self.options).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| Diagnostic::error(error.message))
                .collect::<Vec<_>>()
        })?;
        let mut files: Vec<OutputFile> = output
            .files
            .into_iter()
            .map(|(name, contents)| OutputFile::new(&name, contents))
            .collect();
        for (name, path) in output.copies {
            match std::fs::read(&path) {
                Ok(contents) => files.push(OutputFile::new(&name, contents)),
                Err(e) => {
                    return Err(vec![Diagnostic::error(format!(
                        "cannot read `{}`, which an external function points to: {e}",
                        path.display()
                    ))]);
                }
            }
        }
        Ok(BuildOutput {
            files,
            entry: PathBuf::from(emela_codegen_c_embedded::MAIN_C_FILE),
        })
    }

    fn runner(&self) -> crate::pipeline::Runner {
        crate::pipeline::Runner::None
    }
}
