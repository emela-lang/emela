//! The output stage that turns the Core IR into JS with emela-codegen-js.

use emela_codegen_js::{BUNDLED_DIR, Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};

use crate::diagnostic::Diagnostic;
use crate::output::{BuildOutput, OutputFile};
use crate::pipeline::{Analysis, Backend};

pub const JS_ENTRY_FILE: &str = "main.mjs";

/// Turns one Core IR module into JS. The runtime is written to a sibling file and imported.
///
/// Lowering has already loopified self tail calls. This shape assumes a single module; `Program`
/// must change once lowering emits several.
#[derive(Debug, Default, Clone, Copy)]
pub struct CoreJs;

impl Backend<emela_core::Module> for CoreJs {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        analysis: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        let options = Options {
            runtime: RuntimeMode::Import(None),
        };
        // Files `@external` points to are copied into the output and imported relatively.
        let (module, host_files) = crate::host_files::place(module, analysis, "js")?;
        let mut files = vec![
            OutputFile::new(JS_ENTRY_FILE, emit_module(&module, &options)),
            OutputFile::new(RUNTIME_FILE, RUNTIME),
        ];
        files.extend(bundled_outputs(&module));
        files.extend(host_files);
        Ok(BuildOutput {
            files,
            entry: JS_ENTRY_FILE.into(),
        })
    }
}

/// Maps the IR's bundled JS host files (`Module::bundled_files`, without the C ones) to files
/// under [`BUNDLED_DIR`] in the output directory. The entry output (at the top of the output
/// directory) imports them as `./emela_core/x.mjs`.
pub(crate) fn bundled_outputs(module: &emela_core::Module) -> impl Iterator<Item = OutputFile> {
    module
        .bundled_files
        .iter()
        .filter(|file| file.path.ends_with(".mjs"))
        .map(|file| {
            OutputFile::new(
                format!("{BUNDLED_DIR}/{}", file.path),
                file.contents.clone(),
            )
        })
}
