use std::fs;

use crate::ast::Program;
use crate::error::{Error, Result};
use crate::platform::PlatformSpec;
use crate::typecheck::{CheckMode, TypedProgram};

use super::{lowering, EmitOptions};

pub(crate) enum JsRuntime {
    Node,
    Bun,
}

pub(crate) struct JsBackendProfile {
    runtime: JsRuntime,
}

impl JsBackendProfile {
    pub(super) fn node() -> Self {
        Self {
            runtime: JsRuntime::Node,
        }
    }

    pub(super) fn bun() -> Self {
        Self {
            runtime: JsRuntime::Bun,
        }
    }

    pub(super) fn platform(&self) -> PlatformSpec {
        PlatformSpec::js_runtime(self.runtime.name())
    }

    pub(super) fn emit(
        &self,
        platform: &PlatformSpec,
        program: &Program,
        typed: &TypedProgram,
        options: EmitOptions<'_>,
    ) -> Result<()> {
        if options.output.is_some() {
            return Err(Error::new("js backend does not support --output"));
        }
        let Some(path) = options.artifact else {
            return Err(Error::new("js backend requires --artifact"));
        };
        let js = if options.mode == CheckMode::Library {
            emit_js_library_artifact(platform, program, typed)?
        } else {
            emit_js_artifact(platform, program, typed)?
        };
        fs::write(path, js).map_err(|err| {
            Error::new(format!(
                "failed to write backend output `{}`: {err}",
                path.display()
            ))
        })
    }
}

impl JsRuntime {
    fn name(&self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Bun => "bun",
        }
    }
}

pub(crate) fn emit_js_artifact(
    platform: &PlatformSpec,
    program: &Program,
    typed: &TypedProgram,
) -> Result<String> {
    lowering::emit_js(platform, program, typed)
}

pub(crate) fn emit_js_library_artifact(
    platform: &PlatformSpec,
    program: &Program,
    typed: &TypedProgram,
) -> Result<String> {
    lowering::emit_js_library(platform, program, typed)
}
