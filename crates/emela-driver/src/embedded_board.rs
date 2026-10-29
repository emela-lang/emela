//! Board selection, source overlays, and firmware tool invocation.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use emela_codegen_emb::boards;
use emela_resolve::{DirEntry, SourceFs};
use serde::{Deserialize, Serialize};

use crate::{Diagnostic, EmbeddedConfig, FileSystem, Input, Location, OsFs};

const STATE_SCRIPT: &str = boards::BUILD_STATE_SCRIPT;

/// Optional board settings from the project's `[embedded]` table.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedProject {
    pub board: Option<String>,
    pub config: Option<PathBuf>,
    pub entry: Option<String>,
    #[serde(default)]
    pub sources: Vec<PathBuf>,
    pub optimization: Option<String>,
    #[serde(default)]
    pub modules: BTreeMap<String, PathBuf>,
}

impl EmbeddedProject {
    /// Reads only the embedded table; other project tables retain their existing meaning.
    pub fn read(input: &Input) -> Result<Self, Diagnostic> {
        if input.single_file {
            return Ok(Self::default());
        }
        let path = input.base.join("Pome.toml");
        let text = std::fs::read_to_string(&path).map_err(|error| {
            Diagnostic::error(format!("cannot read project configuration: {error}"))
                .at(Location::Path(path.clone()))
        })?;
        let table: toml::Table = toml::from_str(&text).map_err(|error| {
            Diagnostic::error(format!("invalid project configuration: {error}"))
                .at(Location::Path(path.clone()))
        })?;
        match table.get("embedded") {
            Some(value) => value.clone().try_into().map_err(|error| {
                Diagnostic::error(format!("invalid `[embedded]` configuration: {error}"))
                    .at(Location::Path(path))
            }),
            None => Ok(Self::default()),
        }
    }

    /// Applies command-line precedence and validates the selected reusable BSP.
    pub fn select(mut self, flag: Option<String>) -> Result<Self, Diagnostic> {
        if let Some(flag) = flag {
            self.board = Some(flag);
        }
        if let Some(id) = &self.board
            && boards::package(id).is_none()
        {
            return Err(
                Diagnostic::error(format!("unknown embedded board `{id}`")).with_note(format!(
                    "expected one of {}",
                    boards::BOARD_NAMES.join(", ")
                )),
            );
        }
        if let Some(optimization) = &self.optimization
            && !matches!(optimization.as_str(), "O0" | "O2")
        {
            return Err(Diagnostic::error(
                "`[embedded].optimization` must be `O0` or `O2`",
            ));
        }
        Ok(self)
    }

    pub fn board_id(&self) -> Option<&str> {
        self.board.as_deref()
    }

    /// Board outputs have separate default directories to prevent accidental board reuse.
    pub fn default_out_dir(&self, input: &Input) -> PathBuf {
        let root = input.default_out_dir_for(crate::Target::CEmbedded);
        match &self.board {
            Some(board) => root.join(board),
            None => root,
        }
    }

    pub fn runtime_config(
        &self,
        input: &Input,
        flag: Option<PathBuf>,
    ) -> Result<EmbeddedConfig, Diagnostic> {
        let path = flag.or_else(|| self.config.as_ref().map(|p| input.base.join(p)));
        let config = match path {
            Some(path) => EmbeddedConfig::read(&OsFs, &path)?,
            None => EmbeddedConfig::default(),
        };
        if self.board.is_some() {
            config.with_board_entry(self.entry.as_deref().unwrap_or("main"))
        } else {
            Ok(config)
        }
    }

    /// A replacement whose path names `{board}` applies only while a board is selected; without
    /// one, the module in the source root is read as it is.
    pub fn overlay(&self, input: &Input) -> Result<EmbeddedSources, Diagnostic> {
        let mut replacements = BTreeMap::new();
        for (name, source) in &self.modules {
            let source = source.to_string_lossy();
            let source = match self.board_id() {
                Some(board) => source.replace("{board}", board),
                None if source.contains("{board}") => continue,
                None => source.into_owned(),
            };
            let mut relative = PathBuf::new();
            for part in name.split('.') {
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return Err(Diagnostic::error(format!(
                        "invalid overlay module name `{name}`"
                    )));
                }
                let mut file = String::new();
                for (index, character) in part.chars().enumerate() {
                    if character.is_ascii_uppercase() && index > 0 {
                        file.push('_');
                    }
                    file.push(character.to_ascii_lowercase());
                }
                relative.push(file);
            }
            relative.set_extension("emel");
            let physical = input.base.join(source);
            if !physical.is_file() {
                return Err(Diagnostic::error(format!(
                    "overlay module `{name}` does not exist: {}",
                    physical.display()
                )));
            }
            replacements.insert(input.root.join(relative), physical);
        }
        Ok(EmbeddedSources { replacements })
    }

    /// Invalidates an older successful build before any frontend or toolchain operation.
    pub fn invalidate(&self, out_dir: &Path) -> Result<(), Diagnostic> {
        match std::fs::remove_file(out_dir.join("board-build.json")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Diagnostic::error(format!(
                "cannot invalidate previous firmware build: {error}"
            ))),
        }
    }

    /// Captures inputs before lowering so edits made during compilation cannot be sealed.
    pub fn prepare_build(
        &self,
        input: &Input,
        out_dir: &Path,
        config_flag: Option<&Path>,
    ) -> Result<(), Diagnostic> {
        std::fs::create_dir_all(out_dir).map_err(io_diagnostic)?;
        let out_dir = absolute_output(out_dir)?;
        let generated = out_dir.join("generated");
        if generated.is_dir() {
            for entry in std::fs::read_dir(generated).map_err(io_diagnostic)? {
                let path = entry.map_err(io_diagnostic)?.path();
                if path.is_file()
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("emela_"))
                    && path
                        .extension()
                        .is_some_and(|extension| extension == "c" || extension == "h")
                {
                    std::fs::remove_file(path).map_err(io_diagnostic)?;
                }
            }
        }
        let spec = self.state_spec(input, &out_dir, config_flag)?;
        execute(
            python()
                .arg("-c")
                .arg(STATE_SCRIPT)
                .arg("prepare")
                .arg("--expected")
                .arg(
                    serde_json::to_string(&spec)
                        .map_err(|error| Diagnostic::error(error.to_string()))?,
                )
                .arg("--manifest")
                .arg(out_dir.join("board-build-pending.json")),
            "firmware input recording",
        )
    }

    /// Installs bundled files and invokes the selected board's compiler boundary.
    pub fn build_firmware(
        &self,
        input: &Input,
        out_dir: &Path,
        config_flag: Option<&Path>,
    ) -> Result<(), Diagnostic> {
        let id = self
            .board_id()
            .ok_or_else(|| Diagnostic::error("no embedded board selected"))?;
        let package = boards::package(id).expect("board was validated");
        let out_dir = absolute_output(out_dir)?;
        let bsp_dir = out_dir.join("bsp");
        std::fs::create_dir_all(&bsp_dir).map_err(io_diagnostic)?;
        for asset in package.assets {
            std::fs::write(bsp_dir.join(asset.name), asset.contents).map_err(io_diagnostic)?;
        }
        std::fs::write(bsp_dir.join("build_state.py"), STATE_SCRIPT).map_err(io_diagnostic)?;
        let mut command = python();
        command
            .arg(bsp_dir.join("build.py"))
            .arg("build")
            .arg("--generated-dir")
            .arg(out_dir.join("generated"))
            .arg("--output-dir")
            .arg(out_dir.join("firmware"))
            .arg("--project-dir")
            .arg(&input.base)
            .arg("--optimization")
            .arg(self.optimization.as_deref().unwrap_or("O2"));
        for source in &self.sources {
            let source = input.base.join(source);
            if !source.is_file() {
                return Err(Diagnostic::error(format!(
                    "embedded C source does not exist: {}",
                    source.display()
                )));
            }
            command.arg("--source").arg(source);
        }
        execute(&mut command, "firmware build")?;
        for name in [
            "firmware.elf",
            "firmware.bin",
            "firmware.map",
            "firmware.json",
        ] {
            if !out_dir.join("firmware").join(name).is_file() {
                return Err(Diagnostic::error(format!(
                    "board build did not produce `{name}`"
                )));
            }
        }
        let spec = self.state_spec(input, &out_dir, config_flag)?;
        let spec_path = out_dir.join("board-inputs.json");
        let contents = serde_json::to_vec_pretty(&spec)
            .map_err(|error| Diagnostic::error(error.to_string()))?;
        std::fs::write(&spec_path, contents).map_err(io_diagnostic)?;
        execute(
            python()
                .arg(bsp_dir.join("build_state.py"))
                .arg("seal")
                .arg("--spec")
                .arg(spec_path)
                .arg("--manifest")
                .arg(out_dir.join("board-build.json"))
                .arg("--prepared")
                .arg(out_dir.join("board-build-pending.json")),
            "firmware recording",
        )
    }

    /// Verifies the exact selected project and recorded artifacts before invoking a flasher.
    pub fn flash(
        &self,
        input: &Input,
        out_dir: &Path,
        config_flag: Option<&Path>,
        port: Option<&str>,
        dry_run: bool,
    ) -> Result<(), Diagnostic> {
        let out_dir = absolute_output(out_dir)?;
        let spec = self.state_spec(input, &out_dir, config_flag)?;
        let manifest = out_dir.join("board-build.json");
        if !manifest.is_file() {
            return Err(Diagnostic::error(
                "no successful firmware build exists; run `emela build` first",
            ));
        }
        // The current compiler's verifier runs before any code from the build directory.
        let mut verify = python();
        verify
            .arg("-c")
            .arg(STATE_SCRIPT)
            .arg("verify")
            .arg("--manifest")
            .arg(&manifest)
            .arg("--expected")
            .arg(
                serde_json::to_string(&spec)
                    .map_err(|error| Diagnostic::error(error.to_string()))?,
            );
        execute(&mut verify, "firmware verification")?;
        let mut command = python();
        command
            .arg(out_dir.join("bsp/build.py"))
            .arg("flash")
            .arg("--output-dir")
            .arg(out_dir.join("firmware"));
        if let Some(port) = port {
            command.arg("--port").arg(port);
        }
        if dry_run {
            command.arg("--dry-run");
        }
        execute(&mut command, "firmware flashing")
    }

    fn state_spec(
        &self,
        input: &Input,
        out_dir: &Path,
        config_flag: Option<&Path>,
    ) -> Result<serde_json::Value, Diagnostic> {
        let explicit_config = config_flag
            .map(|path| std::fs::canonicalize(path).map_err(io_diagnostic))
            .transpose()?;
        let mut files = self
            .sources
            .iter()
            .map(|path| input.base.join(path))
            .collect::<Vec<_>>();
        files.extend(self.modules.values().map(|path| {
            input.base.join(
                path.to_string_lossy()
                    .replace("{board}", self.board_id().unwrap_or("")),
            )
        }));
        if let Some(path) = &explicit_config {
            files.push(path.clone());
        }
        if explicit_config.is_none()
            && let Some(path) = &self.config
        {
            files.push(input.base.join(path));
        }
        if !input.single_file {
            files.push(input.base.join("Pome.toml"));
            let lock = input.base.join("Pome.lock");
            if lock.is_file() {
                files.push(lock);
            }
        }
        files.extend(
            input
                .dependencies
                .pomes
                .iter()
                .map(|pome| pome.dir.join("Pome.toml")),
        );
        let dependencies: Vec<_> = input
            .dependencies
            .pomes
            .iter()
            .map(|pome| {
                serde_json::json!({
                    "name": pome.name, "directory": pome.dir, "version": pome.version,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "board": self.board_id(), "settings": self,
            "project": input.base, "entry": input.entry, "source_root": input.root,
            "single_file": input.single_file,
            "output": out_dir, "explicit_config": explicit_config, "files": files,
            "dependencies": dependencies,
            "bsp_files": boards::package(self.board_id().unwrap_or("")).map(|package| package.assets.iter().map(|asset| asset.name).chain(std::iter::once("build_state.py")).collect::<Vec<_>>()),
        }))
    }
}

fn absolute_output(path: &Path) -> Result<PathBuf, Diagnostic> {
    if path.exists() {
        std::fs::canonicalize(path).map_err(io_diagnostic)
    } else if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir().map_err(io_diagnostic)?.join(path))
    }
}

fn io_diagnostic(error: io::Error) -> Diagnostic {
    Diagnostic::error(format!("embedded firmware I/O failed: {error}"))
}

fn python() -> Command {
    Command::new(std::env::var_os("EMELA_PYTHON").unwrap_or_else(|| "python3".into()))
}

fn execute(command: &mut Command, role: &str) -> Result<(), Diagnostic> {
    let status = command
        .status()
        .map_err(|error| Diagnostic::error(format!("cannot start {role}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Diagnostic::error(format!("{role} failed ({status})")))
    }
}

/// Read-only source replacement at the driver's filesystem boundary.
pub struct EmbeddedSources {
    replacements: BTreeMap<PathBuf, PathBuf>,
}

impl EmbeddedSources {
    fn has_directory(&self, path: &Path) -> bool {
        self.replacements
            .keys()
            .any(|file| file != path && file.starts_with(path))
    }
}

impl SourceFs for EmbeddedSources {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let mut entries = match OsFs.read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound && self.has_directory(dir) => {
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        for file in self.replacements.keys() {
            if let Ok(relative) = file.strip_prefix(dir) {
                let mut components = relative.components();
                if let Some(first) = components.next() {
                    let entry = DirEntry {
                        name: first.as_os_str().to_owned(),
                        is_dir: components.next().is_some(),
                    };
                    entries.retain(|existing| existing.name != entry.name);
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }
}

impl FileSystem for EmbeddedSources {
    fn absolute(&self, path: &Path) -> io::Result<PathBuf> {
        if self.replacements.contains_key(path) || self.has_directory(path) {
            Ok(path.to_owned())
        } else {
            OsFs.absolute(path)
        }
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        OsFs.read_to_string(
            self.replacements
                .get(path)
                .map(PathBuf::as_path)
                .unwrap_or(path),
        )
    }
    fn is_file(&self, path: &Path) -> bool {
        self.replacements.contains_key(path) || OsFs.is_file(path)
    }
    fn is_dir(&self, path: &Path) -> bool {
        self.has_directory(path) || OsFs.is_dir(path)
    }
}
