---
title: emela doctor
authors: [Alliana]
status: draft
pr: 535
issue:
decided:
touches: [cmd, cmd.doctor (new), target.c-embedded.board]
requires: []
related: []
---

# emela doctor

## Summary

Add the command `emela doctor`. It resolves the external programs that each build target uses (node, cc, wasmtime, the tools of the embedded target) and the routes used to fetch Pomes, by the same rules as the commands that use them, and reports what is missing.

## Motivation

- Today a missing external program is found only when it is used: E0909 appears the first time `emela run --target wasm` runs. Each switch of target finds the next missing program, one at a time.
- The rules that resolve the executables (`EMELA_NODE`, `EMELA_CC`, `EMELA_WASMTIME`, `EMELA_PYTHON`, `OPENOCD`, `EMELA_ESPTOOL_PYTHON`) are spread across the specification, and there is no way to see which executable would be used now.
- A bug report has to ask for `emela --version` and the version of each tool separately.
- The workaround is to run `--version` of each tool by hand. That does not reflect the `EMELA_*` overrides, and the minimum versions have to be compared by hand.

## Proposal

### Meaning

- `emela doctor [path]` resolves the external programs that each build target needs by the same rules as the commands of that target, starts them to read their versions, and writes the results to standard output, grouped by target.
- A missing program is reported as a sentence saying that the target needs the program but it was not found (or is too old), with help on how to install it. No diagnostic code is used.
- It installs nothing, changes no environment variables, and does not go to the network.
- Inside a project, the programs of the project's build target are required. Outside a project, every program is optional. `--target` and `--board` select them instead.
- The exit code is 0 when every required program passes, and 1 otherwise.
- `--format json` writes the same contents as JSON.
- For fetching Pomes, it reports the values in effect of `EMELA_POMEPROXY` and `EMELA_POMESUMDB`, the number of prefixes in `EMELA_POMEPRIVATE`, and whether git starts. It does not try to contact the proxy.

Example output, for a project whose target is `wasm`:

```text
emela 0.28.0+4e84642

js
  ok       node 24.11.0 (/opt/homebrew/bin/node)
c
  ok       cc Apple clang version 17.0.0 (/usr/bin/cc)
wasm (required)
  missing  target `wasm` needs wasmtime 49 or later, but `wasmtime` is not on PATH
           install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable

pome
  proxy    https://pomes.emela.dev, direct
  sumdb    https://sum.emela.dev
  private  0 prefixes
  ok       git 2.51.0 (/usr/bin/git)
```

### The version of node

node passes when its version is one that `[target]` supports. The minimum stays in `[target]` alone; raising it to 22 is a separate fix.

### The version of cc

For cc, the command reports whether it starts and the first line of its `--version`. It does not check whether the compiler accepts C11.

## Specification changes

### `[target.c-embedded.board]`

Replace the last item with:

```markdown
- Making the firmware image needs Python 3 (the environment variable `EMELA_PYTHON`, else `python3`) and the cross toolchain of the board: `arm-none-eabi-gcc` for the STM32F4 boards (in the `bin` directory of the environment variable `ARM_TOOLCHAIN_PATH`, else on PATH), and `riscv32-esp-elf-gcc` and the Python package esptool for `esp32c3` (in the `bin` directory of the environment variable `EMELA_ESP32C3_TOOLCHAIN`, else on PATH; esptool is imported by the Python of the environment variable `EMELA_ESPTOOL_PYTHON`, else by the Python of the build). The compiler does not fetch them.
```

### `[cmd]`

Replace the second sentence of the introduction:

```markdown
`emela test` is in 13.4 and `[cmd.test]`, `emela pome` is in `[cmd.pome]`, `emela flash` is in `[cmd.flash]`, and `emela doctor` is in `[cmd.doctor]`.
```

### `[cmd.doctor]` (new)

```markdown
## emela doctor `[cmd.doctor]`

- Reports, for each build target, whether the external programs it uses can be found and started and whether their versions are new enough, and reports the routes used to fetch Pomes. It installs nothing, changes no environment variables, and does not go to the network.
- Takes one path, which defaults to `.`. When the path is in a project (4.1), the build target of the project (`[cmd.build]`) is required, and so is its board (`[target.c-embedded.board]`) when the target is `c-embedded`. Outside a project, no target is required. `--target <js|c|c-embedded|wasm>` makes that target required instead, and `--board <id>` selects the board.
- Each program is resolved by the same rule as the command that uses it, and is checked as follows.

  | Target | Program | Rule | Passes when |
  |---|---|---|---|
  | `js` | node | `[cmd.build]` | it starts and `--version` reports a version that `[target]` supports |
  | `c` | the C compiler | `[target.c.cc]` | it starts; the first line of its `--version` is reported as its version |
  | `wasm` | wasmtime | `[target.wasm.run]` | it starts and its major version is 49 or later |
  | `c-embedded` | Python | `[target.c-embedded.board]` | it starts and reports version 3 |
  | `c-embedded`, STM32F4 boards | `arm-none-eabi-gcc`, OpenOCD | `[target.c-embedded.board]`, `[cmd.flash]` | each starts |
  | `c-embedded`, `esp32c3` | `riscv32-esp-elf-gcc`, esptool | `[target.c-embedded.board]`, `[cmd.flash]` | each starts |

  The items of `c-embedded` are checked only when a board is selected.
- A program passes or fails with one of these reasons: it was not found, it could not be started, its version is older than required, or its version could not be read. A program whose version could not be read passes, as in `[target.wasm.run]`.
- The Pome section reports the values in effect of `EMELA_POMEPROXY` and `EMELA_POMESUMDB`, the number of prefixes in `EMELA_POMEPRIVATE` (`[module.pome]`), and whether git starts. The prefixes themselves are not written, so that the output can be pasted into a public report. git is required when `EMELA_POMEPRIVATE` is set or when `direct` is the only entry of `EMELA_POMEPROXY`. The userinfo part of a URL (`user:token@`) is written as `***@`. Nothing is fetched and no proxy is contacted.
- Output without `--format`, written to standard output:
  - The first line is the same as `emela --version`, followed by a blank line.
  - For each target in the order `js`, `c`, `wasm`, `c-embedded`, a line with the target's name, followed by ` (required)` when required. Under it, one line per program, indented by two spaces: `ok`, `missing`, `too-old` or `failed`, then the program's name and version and the executable resolved in parentheses, or a sentence that names the target and the program that it lacks. A program that fails is followed by a line with the help of the diagnostic that the command using it would report (E0904, E0907, E0909), aligned with the sentence.
  - Then a blank line and the Pome section in the same form.
  - Color is used under the same conditions as diagnostics (`[diag]`).
- With `--format json`, the command writes one JSON object to standard output instead:

  ```json
  {
    "emela": "0.28.0+4e84642",
    "targets": [
      {
        "target": "wasm",
        "required": true,
        "programs": [
          {
            "name": "wasmtime",
            "status": "missing",
            "executable": "wasmtime",
            "version": null,
            "message": "target `wasm` needs wasmtime 49 or later, but `wasmtime` is not on PATH",
            "help": "install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable"
          }
        ]
      }
    ],
    "pome": {
      "proxy": ["https://pomes.emela.dev", "direct"],
      "sumdb": "https://sum.emela.dev",
      "private": 0,
      "git": { "name": "git", "status": "ok", "executable": "/usr/bin/git", "version": "2.51.0", "message": null, "help": null }
    }
  }
  ```

  `status` is one of `ok`, `missing`, `too-old` and `failed`. `executable` is the absolute path when the program was found, and the name that was looked up otherwise. Keys may be added in later versions; existing keys keep their meaning.
- The exit code is 0 when every program of every required target passes and git passes when it is required, and 1 otherwise.
```

## Diagnostics

- [x] No new diagnostic codes. The results are a report on standard output, not diagnostics.

## Compatibility

- `emela doctor` is a new command. The behavior of the existing commands does not change.

## Implementation

- The programs are resolved by the existing functions of `emela-driver` (the resolution of node, cc and wasmtime, and the reading of the wasmtime version). There is no separate rule for doctor.
- The tools of the embedded target are resolved today by the `build.py` of each board (`ARM_TOOLCHAIN_PATH`, `EMELA_ESP32C3_TOOLCHAIN`). The same rule now also lives in Rust, so one set of tests covers both.
- Add a row for `emela doctor` to the table of commands in the README.
- Ask for the output of `emela doctor` in the environment field of `.github/ISSUE_TEMPLATE/bug_report.yml`.

## Alternatives

- Add the information to `emela --version --verbose`. Scripts rely on `--version` printing one line, so this would break them.
- Do nothing, and rely on E0904, E0907 and E0909 at the time of use. Each switch of target still finds one missing program at a time.
- Report the results as diagnostics (`error[E09xx]`). The information has no location in the sources, and it would not fit the `N errors` count.
- Check that the proxy can be reached. That goes to the network, and depending on `EMELA_POMEPRIVATE` it could send private source paths outside. The command reports the settings only.

## Prior art

`flutter doctor`, `brew doctor` and `react-native doctor` use the same name and list the required tools with their versions.

## Open questions

## Decision
