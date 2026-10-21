use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use emela_codegen_emb::{Options, boards, compile};
use emela_core::Roots;

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

const USAGE: &str = "Usage: board_probe --board BOARD --out-dir DIRECTORY";

struct Arguments {
    board: String,
    output: PathBuf,
}

fn arguments(mut args: impl Iterator<Item = OsString>) -> Result<Option<Arguments>, String> {
    let mut board = None;
    let mut output = None;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                println!("{USAGE}\nBoards: {}", boards::BOARD_NAMES.join(", "));
                return Ok(None);
            }
            Some("--board") if board.is_none() => {
                board = Some(
                    args.next()
                        .ok_or("--board requires a value")?
                        .into_string()
                        .map_err(|_| "--board requires a UTF-8 identifier")?,
                );
            }
            Some("--out-dir") if output.is_none() => {
                output = Some(PathBuf::from(
                    args.next().ok_or("--out-dir requires a value")?,
                ));
            }
            _ => {
                return Err(format!(
                    "unknown or repeated argument {argument:?}\n{USAGE}"
                ));
            }
        }
    }
    Ok(Some(Arguments {
        board: board.ok_or(USAGE)?,
        output: output.ok_or(USAGE)?,
    }))
}

fn source(board: &str) -> String {
    let pin = match board {
        "stm32f446" | "stm32f446re" | "nucleo-f446re" => 5,
        "esp32c3" => 3,
        _ => 60,
    };
    format!(
        r#"import Std.Embedded.Gpio.{{Gpio, GpioPin, OutputPushPull, NoPull, High, Low}}
import Std.Embedded.Board.{{BoardGpio, BoardConsole, BoardClock}}
import Std.Console.{{Console}}
import Std.Clock.{{Clock}}

pub fn main() -> Int use Async {{
  with BoardGpio, BoardConsole, BoardClock {{
    gpio = use Gpio
    pin = GpioPin({pin})
    gpio.configure(pin, OutputPushPull, NoPull)
    gpio.write(pin, High)
    (use Console).print_line("board probe: high")
    (use Clock).sleep(10)
    gpio.write(pin, Low)
    (use Console).print_line("board probe: low")
    42
  }}
}}
"#
    )
}

fn require_absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
        Ok(_) => Err(format!("output already exists: {}", path.display())),
    }
}

struct Staging(PathBuf);

impl Staging {
    fn create(parent: &Path) -> Result<Self, String> {
        for attempt in 0..1000 {
            let path = parent.join(format!(
                ".emela-board-probe-{}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        Err("cannot reserve a staging directory".into())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(directory: &Path, name: &str, contents: &[u8]) -> Result<(), String> {
    if !matches!(
        Path::new(name).components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    ) {
        return Err(format!("invalid bundled file name: {name}"));
    }
    let path = directory.join(name);
    fs::write(&path, contents).map_err(|error| format!("{}: {error}", path.display()))
}

fn run() -> Result<(), String> {
    let Some(arguments) = arguments(std::env::args_os().skip(1))? else {
        return Ok(());
    };
    let package = boards::package(&arguments.board).ok_or_else(|| {
        format!(
            "unknown board {}; expected one of {}",
            arguments.board,
            boards::BOARD_NAMES.join(", ")
        )
    })?;
    require_absent(&arguments.output)?;
    let source = source(package.id);
    let core = support::try_lower(&[("main.emel", &source)], Roots::Exported)
        .map_err(|errors| errors.join("\n"))?;
    let options = Options::default()
        .with_heap_bytes(2048)
        .with_string_capacity(64)
        .with_peripheral_buffer_capacity(16)
        .with_fiber_capacity(2)
        .with_frame_capacity(8)
        .with_frame_bytes(512)
        .with_task_capacity(2)
        .with_event_capacity(2)
        .with_deferred_capacity(1);
    let output = compile(&core, &options).map_err(|errors| {
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let main = output
        .exports
        .iter()
        .find(|entry| entry.source_name == "main" && entry.suspends)
        .ok_or("the board probe must export a suspending main entry")?;
    let aliases = format!(
        "#ifndef EMELA_EMBEDDED_EXPORTS_H\n#define EMELA_EMBEDDED_EXPORTS_H\n#include \"emela_embedded.h\"\n#define board_main_start {}\n#endif\n",
        main.c_symbol
    );
    let parent = arguments
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if arguments.output.file_name().is_none() {
        return Err("--out-dir must name a new directory".into());
    }
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let staging = Staging::create(parent)?;
    let generated = staging.0.join("generated");
    let bsp = staging.0.join("bsp");
    for directory in [&generated, &bsp] {
        fs::create_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    }
    write(&staging.0, "main.emel", source.as_bytes())?;
    for file in output.files {
        write(&generated, &file.path, file.contents.as_bytes())?;
    }
    write(&generated, "emela_embedded_exports.h", aliases.as_bytes())?;
    for asset in package.assets {
        write(&bsp, asset.name, asset.contents)?;
    }
    // Publish complete generated/BSP inputs and leave an existing output untouched.
    require_absent(&arguments.output)?;
    fs::rename(&staging.0, &arguments.output)
        .map_err(|error| format!("{}: {error}", arguments.output.display()))?;
    println!("{}: {}", package.id, arguments.output.display());
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("board_probe: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
