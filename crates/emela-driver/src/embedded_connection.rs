//! Standalone connection: reads only generated C/headers/manifest and package inputs.
use emela_codegen_emb::platform::{self, ConnectOptions, Optimization, PlatformPackage};
use std::path::Path;

pub struct ArtifactConnection<'a> {
    pub artifact: &'a Path,
    pub output: &'a Path,
    pub platform: &'a str,
    pub package: Option<&'a Path>,
    pub compiler: &'a Path,
    pub optimization: Optimization,
    pub main: bool,
}
pub fn connect_artifact(request: ArtifactConnection<'_>) -> Result<std::path::PathBuf, String> {
    if request.package.is_none() && request.platform != "native" {
        if emela_codegen_emb::boards::BOARD_NAMES.contains(&request.platform) {
            return Err(format!(
                "connecting a saved artifact to board `{}` is not supported yet; use `emela build --board {}` with the original project for firmware",
                request.platform, request.platform
            ));
        }
        return Err(format!(
            "unknown connection platform `{}`; use `native` or provide `--package`",
            request.platform
        ));
    }
    let output = platform::execution::load_artifact(request.artifact)?;
    let package = if let Some(path) = request.package {
        serde_json::from_slice::<PlatformPackage>(
            &std::fs::read(path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?
    } else {
        platform::native()
    };
    let toolchain = platform::execution::inspect_toolchain(request.compiler)?;
    let options = ConnectOptions {
        optimization: request.optimization,
        executable: if request.platform == "native" {
            "program"
        } else {
            "firmware.elf"
        }
        .into(),
        main: request.main,
    };
    let plan = platform::prepare_connection(&output, &package, &options, &toolchain)?;
    platform::execution::execute(&plan, request.output)
}
pub use emela_codegen_emb::platform::Optimization as ConnectionOptimization;
