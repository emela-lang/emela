//! Embedded C generation with explicit board adapters and fixed resource limits.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use emela_codegen_emb::{ErrorKind, ExternalAdapter, ExternalResult, Options, compile};
use serde::Deserialize;

use crate::diagnostic::{Diagnostic, Location};
use crate::output::{BuildOutput, OutputFile};
use crate::pipeline::{Analysis, Backend, Runner};
use crate::source::FileSystem;

/// The generated C translation unit, compiled separately by the board toolchain.
pub const EMBEDDED_ENTRY_FILE: &str = "emela_embedded.c";
/// Stable board-facing aliases for selected generated public entry points.
pub const EMBEDDED_EXPORTS_FILE: &str = emela_codegen_emb::artifact::ALIASES_HEADER;

/// Optional TOML settings for the embedded C backend; omitted values use backend defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedConfig {
    heap_bytes: Option<u32>,
    string_capacity: Option<u16>,
    peripheral_buffer_capacity: Option<u16>,
    const_diagnostic_bytes: Option<u16>,
    fiber_capacity: Option<u16>,
    frame_capacity: Option<u16>,
    frame_bytes: Option<u16>,
    task_capacity: Option<u16>,
    event_capacity: Option<u16>,
    deferred_capacity: Option<u16>,
    #[serde(default)]
    isr_entries: Vec<String>,
    #[serde(default)]
    isr_safe_externals: Vec<String>,
    #[serde(default)]
    task_adapters: BTreeMap<String, String>,
    #[serde(default)]
    external_adapters: BTreeMap<String, AdapterConfig>,
    #[serde(default)]
    exports: BTreeMap<String, String>,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    board_entry: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterConfig {
    symbol: String,
    result: AdapterResult,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum AdapterResult {
    Borrowed,
    Owned,
    StringBuffer,
}

impl EmbeddedConfig {
    /// Selects the fixed BSP entry ABI while retaining ordinary C-only exports.
    pub fn with_board_entry(mut self, entry: &str) -> Result<Self, Diagnostic> {
        if entry.trim().is_empty() {
            return Err(self.error("embedded board entry must not be empty"));
        }
        if self
            .exports
            .get("board_main_start")
            .is_some_and(|target| target != entry)
        {
            return Err(
                self.error("export `board_main_start` conflicts with the selected board entry")
            );
        }
        self.exports.insert("board_main_start".into(), entry.into());
        self.board_entry = Some(entry.into());
        Ok(self)
    }

    /// Reads and validates an explicit configuration through the driver's filesystem.
    pub fn read(fs: &dyn FileSystem, path: &Path) -> Result<Self, Diagnostic> {
        let at_path = |message| Diagnostic::error(message).at(Location::Path(path.to_owned()));
        let text = fs
            .read_to_string(path)
            .map_err(|error| at_path(format!("cannot read embedded configuration: {error}")))?;
        let mut config: Self = toml::from_str(&text)
            .map_err(|error| at_path(format!("invalid embedded configuration: {error}")))?;
        config.path = Some(path.to_owned());
        config.validate()?;
        Ok(config)
    }

    fn error(&self, message: impl Into<String>) -> Diagnostic {
        let diagnostic = Diagnostic::error(message);
        match &self.path {
            Some(path) => diagnostic.at(Location::Path(path.clone())),
            None => diagnostic,
        }
    }

    fn validate(&self) -> Result<(), Diagnostic> {
        if let Some(capacity) = self.string_capacity
            && !(1..=512).contains(&capacity)
        {
            return Err(self.error("string_capacity must be between 1 and 512 bytes"));
        }
        for (source, adapter) in &self.task_adapters {
            self.validate_symbol(source, "task adapter source")?;
            self.validate_symbol(adapter, "task adapter symbol")?;
        }
        for (source, adapter) in &self.external_adapters {
            self.validate_symbol(source, "external adapter source")?;
            self.validate_symbol(&adapter.symbol, "external adapter symbol")?;
        }
        for (alias, target) in &self.exports {
            self.validate_symbol(alias, "export alias")?;
            if is_reserved_alias(alias) {
                return Err(self.error(format!(
                    "invalid export alias `{alias}`: conflicts with a reserved C or generated identifier"
                )));
            }
            if target.trim().is_empty() {
                return Err(self.error(format!("export alias `{alias}` has an empty target")));
            }
        }
        for (field, entries) in [
            ("isr_entries", &self.isr_entries),
            ("isr_safe_externals", &self.isr_safe_externals),
        ] {
            let mut seen = HashSet::new();
            for entry in entries {
                if entry.trim().is_empty() || !seen.insert(entry) {
                    return Err(self.error(format!(
                        "{field} must contain nonempty, unique names; found `{entry}`"
                    )));
                }
                if field == "isr_safe_externals" {
                    self.validate_symbol(entry, "ISR-safe external symbol")?;
                }
            }
        }
        Ok(())
    }

    fn validate_symbol(&self, symbol: &str, role: &str) -> Result<(), Diagnostic> {
        if !is_c_identifier(symbol) || is_c_keyword(symbol) {
            return Err(self.error(format!(
                "invalid {role} `{symbol}`: expected a non-reserved C identifier"
            )));
        }
        Ok(())
    }

    fn options(&self) -> Options {
        let mut options = Options::default();
        if let Some(value) = self.heap_bytes {
            options = options.with_heap_bytes(value);
        }
        if let Some(value) = self.string_capacity {
            options = options.with_string_capacity(value);
        }
        if let Some(value) = self.peripheral_buffer_capacity {
            options = options.with_peripheral_buffer_capacity(value);
        }
        if let Some(value) = self.const_diagnostic_bytes {
            options = options.with_const_diagnostic_bytes(value);
        }
        if let Some(value) = self.fiber_capacity {
            options = options.with_fiber_capacity(value);
        }
        if let Some(value) = self.frame_capacity {
            options = options.with_frame_capacity(value);
        }
        if let Some(value) = self.frame_bytes {
            options = options.with_frame_bytes(value);
        }
        if let Some(value) = self.task_capacity {
            options = options.with_task_capacity(value);
        }
        if let Some(value) = self.event_capacity {
            options = options.with_event_capacity(value);
        }
        if let Some(value) = self.deferred_capacity {
            options = options.with_deferred_capacity(value);
        }
        for (source, adapter) in &self.task_adapters {
            options = options.with_task_adapter(source, adapter);
        }
        for (source, adapter) in &self.external_adapters {
            let result = match adapter.result {
                AdapterResult::Borrowed => ExternalResult::Borrowed,
                AdapterResult::Owned => ExternalResult::Owned,
                AdapterResult::StringBuffer => ExternalResult::StringBuffer,
            };
            options = options
                .with_external_adapter(source, ExternalAdapter::new(&adapter.symbol, result));
        }
        for entry in &self.isr_entries {
            options = options.with_isr_entry(entry);
        }
        for symbol in &self.isr_safe_externals {
            options = options.with_isr_safe_external(symbol);
        }
        for (alias, target) in &self.exports {
            options = options.with_export_alias(alias, target);
        }
        if let Some(entry) = &self.board_entry {
            options = options.with_entry(entry);
        }
        options
    }

    fn validate_entry(&self, module: &emela_core::Module) -> Result<(), Diagnostic> {
        if let Some(entry) = &self.board_entry {
            let candidates = module
                .functions
                .iter()
                .filter(|(_, function)| function.exported && &function.name == entry)
                .collect::<Vec<_>>();
            if candidates.len() == 1 && !candidates[0].1.params.is_empty() {
                return Err(self.error("an embedded main function with parameters is unsupported"));
            }
            let valid = candidates.len() == 1
                && candidates.first().is_some_and(|(_, function)| {
                    function.params.is_empty()
                        && function.ret == emela_core::TypeId::INT
                        && function.suspends
                        && !function.fails
                        && function.type_params == 0
                        && function.external.is_none()
                });
            if !valid {
                return Err(self.error(format!("board entry `{entry}` must be a public, non-generic, argument-free `fn() -> Int use Async` without unhandled failures or effects")));
            }
        }
        Ok(())
    }
}

/// Emits embedded C files without invoking a native compiler or executing the program.
#[derive(Debug, Clone, Default)]
pub struct CoreEmbedded {
    config: EmbeddedConfig,
}

impl CoreEmbedded {
    pub fn new(config: EmbeddedConfig) -> Self {
        Self { config }
    }
}

impl Backend<emela_core::Module> for CoreEmbedded {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        _: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        self.config.validate().map_err(|error| vec![error])?;
        self.config
            .validate_entry(module)
            .map_err(|error| vec![error])?;
        let output = compile(module, &self.config.options()).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| match error.kind {
                    ErrorKind::InvalidOptions => self.config.error(error.message),
                    _ => Diagnostic::error(error.message),
                })
                .collect::<Vec<_>>()
        })?;
        let files = output
            .files
            .into_iter()
            .map(|file| OutputFile::new(file.path, file.contents))
            .collect();
        Ok(BuildOutput {
            files,
            entry: EMBEDDED_ENTRY_FILE.into(),
        })
    }

    fn runner(&self) -> Runner {
        Runner::BuildOnly
    }
}

fn is_c_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_reserved_alias(value: &str) -> bool {
    // Protect generated declarations, header guards, and identifiers supplied by <stdint.h>.
    value.starts_with('_')
        || value.starts_with("emela_")
        || value.starts_with("Emela")
        || value.starts_with("EMELA_")
        || is_stdint_identifier(value)
        || matches!(
            value,
            "main"
                | "NULL"
                | "bool"
                | "true"
                | "false"
                | "size_t"
                | "ptrdiff_t"
                | "wchar_t"
                | "sig_atomic_t"
                | "max_align_t"
                | "offsetof"
        )
}

fn is_c_keyword(value: &str) -> bool {
    matches!(
        value,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

fn is_stdint_identifier(value: &str) -> bool {
    for (type_prefix, macro_prefix) in [("int", "INT"), ("uint", "UINT")] {
        for suffix in [
            "8", "16", "32", "64", "_least8", "_least16", "_least32", "_least64", "_fast8",
            "_fast16", "_fast32", "_fast64", "ptr", "max",
        ] {
            if value == format!("{type_prefix}{suffix}_t")
                || value == format!("{macro_prefix}{}_MIN", suffix.to_ascii_uppercase())
                || value == format!("{macro_prefix}{}_MAX", suffix.to_ascii_uppercase())
            {
                return true;
            }
        }
        for suffix in ["8", "16", "32", "64", "MAX"] {
            if value == format!("{macro_prefix}{suffix}_C") {
                return true;
            }
        }
    }
    matches!(
        value,
        "SIG_ATOMIC_MIN"
            | "SIG_ATOMIC_MAX"
            | "SIZE_MAX"
            | "PTRDIFF_MIN"
            | "PTRDIFF_MAX"
            | "WCHAR_MIN"
            | "WCHAR_MAX"
            | "WINT_MIN"
            | "WINT_MAX"
    )
}
