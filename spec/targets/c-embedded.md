# 16.9 The embedded C target `[target.c-embedded]`

The embedded C target maps a program to C for microcontrollers. Passing `--target c-embedded` to `emela build` builds for this target. When a board is selected (`[target.c-embedded.board]`), `emela build` also makes a firmware image for that board, and `emela flash` writes it (`[cmd.flash]`).

- It is a target separate from the C target (`[target.c]`). The memory of a program is fixed areas whose sizes are given by the runtime configuration (`[target.c-embedded.config]`), and running out of one of them is a defect.
- External functions are declared with `@external(c, ...)`, the same binding name as the C target.
- `emela run` and `emela test` are not available for this target. Selecting it for them is an error (``` `run` is not available for target `c-embedded` ```, and the same for `test`), and nothing is written.
- The constructs that this target does not support are compile errors. Which constructs it supports is implementation-defined for now.

## 16.9.1 Output `[target.c-embedded.output]`

- Without a board, the output directory is `target/emela/c-embedded` (under the directory of the project or of the single file, `[module.name]`). The command writes the C sources and the headers of the program and its runtime there, and starts no other program. A `main` is not required: the fns with `@export(c, ...)` are what the host calls.
- With a board, the default output directory is `target/emela/c-embedded/<board>`, so that the outputs of different boards do not mix. Under it, the command writes the following.

| Path | Contents |
|---|---|
| `generated/` | The C sources and headers of the program, and the header of the C aliases (`[target.c-embedded.config]`) |
| `bsp/` | The bundled board support files used to build and flash |
| `firmware/` | `firmware.elf`, `firmware.bin`, `firmware.map`, `firmware.json` and the other outputs of the board build |
| `board-build.json` | The record of the last successful build (`[cmd.flash]`) |

- `--out-dir` changes the output directory in both cases. A library-only project (`[cmd.build]`) is only checked, and nothing is written, with or without a board.

## 16.9.2 Boards `[target.c-embedded.board]`

- A board is selected by `--board <id>`, else by `board` in the `[embedded]` table of `Pome.toml` (`[target.c-embedded.project]`). The ids are `stm32f407vg`, `stm32f412zg`, `stm32f429zi`, `stm32f446re`, `stm32f446`, `stm32f4-discovery`, `nucleo-f446re` and `esp32c3`. Any other id is an error (``unknown embedded board `<id>` ``, with a note listing the ids).
- When a board is selected and neither `--target` nor `[build].target` is given, the target is `c-embedded`.
- `[embedded].board` applies only when the target is `c-embedded`. Building for `js`, `c` or `wasm` ignores it, and leaves the record of the last successful firmware build as it is. `--board` with a target other than `c-embedded` is an error (``a selected embedded board requires target `c-embedded` ``).
- Making the firmware image needs Python 3 (the environment variable `EMELA_PYTHON`, else `python3`) and the cross toolchain of the board. The compiler does not fetch them.

## 16.9.3 The entry `[target.c-embedded.entry]`

- With a board, the firmware starts the fn named by `entry` in `[embedded]` (`main` if omitted). It MUST be a pub, non-generic fn of the form `fn() -> Int use Async` that leaves no failure or other effect unhandled. Otherwise it is an error (``board entry `<name>` must be a public, non-generic, argument-free `fn() -> Int use Async` without unhandled failures or effects``), reported before any cross compiler is started.
- The entry is given the C alias `board_main_start`. An alias in `[exports]` of the runtime configuration that names `board_main_start` for a different fn is an error.

## 16.9.4 The project's `[embedded]` table `[target.c-embedded.project]`

```toml
[embedded]
board = "stm32f446re"          # the default board
config = "embedded.toml"       # the runtime configuration
entry = "main"                 # the fn the firmware starts
optimization = "O2"            # O0 or O2 (O2 if omitted)
sources = ["device.c"]         # C and assembly files added to the firmware

[embedded.modules]
Hardware = "ports/{board}/hardware.emel"
```

- Every key is optional. An unknown key is an error. Relative paths are relative to the project directory. A single file has no `[embedded]` table.
- `config` is the runtime configuration. `--embedded-config` (relative to the current directory) takes precedence. `--embedded-config` requires the target `c-embedded`. Without either, the defaults of `[target.c-embedded.config]` apply. No file is looked up implicitly.
- `sources` lists C and assembly files (`.c`, `.S`, `.s`) compiled and linked into the firmware. A file that does not exist is an error.
- `modules` maps a module name to a file that is read in place of that module's file in the source root (`Hardware` replaces `src/hardware.emel`, by the file name rules of `[module.name]`). `{board}` in the path is replaced by the id of the selected board. Without a board, an entry whose path contains `{board}` is not applied, and the module in the source root is read. A file to read in place that does not exist is an error (``overlay module `<name>` does not exist: <path>``). Only the reading of the module changes; no file is written.
- The replacement applies to `emela check` and to `emela build` for `c-embedded`. It does not apply to builds for other targets, to the language server, or to `emela doc`.
- `[embedded]` takes effect only when building that Pome, and is ignored when the Pome is depended on.

## 16.9.5 The runtime configuration `[target.c-embedded.config]`

The runtime configuration is a TOML file. An unknown key and a value of the wrong type or out of range are errors. Omitted keys take the defaults below.

| Key | Meaning | Default |
|---|---|---:|
| `heap_bytes` | Bytes of the fixed heap. 0 means no heap | 0 |
| `string_capacity` | Maximum bytes of a String made at run time, 1 to 512 | 256 |
| `peripheral_buffer_capacity` | Elements (16-bit words) of each transfer buffer of the board peripherals, 1 to 65535 | 256 |
| `const_diagnostic_bytes` | Bytes kept for the message of a defect raised while evaluating a const | 1024 |
| `fiber_capacity` | Number of fibers, including finished ones whose handle is still alive | 8 |
| `frame_capacity` | Number of suspended frames alive at once | 32 |
| `frame_bytes` | Bytes of one suspended frame | 256 |
| `task_capacity` | Number of external tasks running at once | 8 |
| `event_capacity` | Number of completions waiting to be handled | 8 |
| `deferred_capacity` | Number of Deferreds alive at once | 8 |

- `[exports]` maps a C alias to an fn with `@export(c, ...)` (`device_read = "read"`). An unknown or ambiguous fn, an alias that is not a C identifier, and an alias that collides with a name of the generated C are errors.
- `[task_adapters]` maps the C symbol of a suspend external fn to the C function that starts it as a task. `[external_adapters.<symbol>]` gives the `symbol` of the C function that implements a synchronous external fn, and how its result is owned (`result` is `borrowed`, `owned` or `string-buffer`).
- `isr_entries` lists fns called from interrupt handlers, and `isr_safe_externals` lists the external C symbols that they may reach. What an fn in `isr_entries` reaches is limited to what is safe in an interrupt; reaching anything else is an error.
