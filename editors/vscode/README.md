# Emela for VSCode

Language support for [Emela](https://github.com/emela-lang/emela): diagnostics,
completion and the other requests of the `emela lsp` language server, plus syntax
highlighting for `.emel` files.

Requires the `emela` binary on `PATH` (or set `emela.serverPath`).

## Develop / try it out

Build the server first (`cargo build -p emela-cli` makes `target/debug/emela`), then:

```sh
cd editors/vscode
npm install
npm run compile
```

1. Open this directory in VSCode and press F5 (Run Extension).
2. In the development host, set `emela.serverPath` to the absolute path of `target/debug/emela`
   (not needed if `emela` on `PATH` is that build).
3. Open a folder with `Pome.toml` and `src/main.emel`, and type `const Y = "\q"`. An `E0107`
   squiggle appears, and goes away once you fix the escape.
4. The server's stderr shows up in the Output panel under "Emela Language Server".

## Settings

- `emela.serverPath` — path to the `emela` binary (default: `emela`).
- `emela.packageRoots` — leave it empty. It adds `--package` arguments from Emela 0.1x, and the
  0.20 `emela lsp` takes no arguments, so the server fails to start. Dependencies declared in a
  project's `Pome.toml` are resolved without any setting.

## Package (optional)

```sh
npx @vscode/vsce package   # produces emela-<version>.vsix
code --install-extension emela-*.vsix
```
