# Emela for VSCode

Language support for [Emela](https://github.com/emela-lang/emela): diagnostics,
completion and the other requests of the `emela lsp` language server, plus syntax
highlighting for `.emel` files.

Requires the `emela` binary. Install it with
`curl -fsSL https://emela.dev/install.sh | sh` (see [Install](../../README.md#install)).
The extension looks for `emela` on `PATH` and then in `~/.emela/bin`, where the install script
puts it, so it is found even when VSCode is started from the desktop without the shell's `PATH`.
If it is somewhere else, set `emela.serverPath`. When the binary is not found, a notification
links to the install instructions and the setting.

Prebuilt binaries are published for macOS (Apple Silicon) and Linux (x86_64). On other
platforms, build `emela` from source (`cargo build --release -p emela-cli`) and set
`emela.serverPath` to it.

## Features

- Syntax highlighting, following the tree-sitter grammar in `editors/tree-sitter-emela`
  (attributes such as `@export(js, "default", "fetch")` and `@entry(Http.serve)`, string
  interpolation, effects, handlers and layers).
- Everything `emela lsp` provides: diagnostics, completion, hover, go to definition, formatting
  and the rest.
- A status bar item, shown while an Emela file is open, with the state of the server (starting,
  running, stopped, or failed to start). Clicking it restarts the server.
- The command **Emela: Restart Language Server**.
- Changing `emela.serverPath` restarts the server with the new binary.

## Settings

- `emela.serverPath` — the `emela` binary (default: `emela`). A bare name is looked up on `PATH`
  and then in `~/.emela/bin`; a path is used as written, with a leading `~` expanded.

Dependencies declared in a project's `Pome.toml` are resolved by the server without any setting.
The `emela.packageRoots` setting of earlier versions is gone: it passed `--package` arguments,
which `emela lsp` no longer takes. If it is still in your `settings.json`, remove it.

## Develop / try it out

Build the server first (`cargo build -p emela-cli` makes `target/debug/emela`), then:

```sh
cd editors/vscode
npm install
npm run compile   # type checks, then bundles src/ into out/extension.js
```

1. Open this directory in VSCode and press F5 (Run Extension).
2. In the development host, set `emela.serverPath` to the absolute path of `target/debug/emela`
   (not needed if `emela` on `PATH` is that build).
3. Open a folder with `Pome.toml` and `src/main.emel`, and type `const Y = "\q"`. An `E0107`
   squiggle appears, and goes away once you fix the escape.
4. The server's stderr shows up in the Output panel under "Emela Language Server".

## Package

```sh
npm run package   # produces emela-<version>.vsix
code --install-extension emela-*.vsix
```

Releases to the Marketplace are made by `.github/workflows/vscode-release.yml`; see
`docs/release.md`.
