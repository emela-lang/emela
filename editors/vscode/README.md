# Emela for VSCode

Language support for [Emela](https://github.com/emela-lang/emela): diagnostics
and completion via the `emela lsp` language server (spec 0033), plus syntax
highlighting for `.emel` files.

Requires the `emela` binary on `PATH` (or set `emela.serverPath`).

## Develop / try it out

```sh
cd editors/vscode
npm install
npm run compile
```

Open this directory in VSCode and press F5 (Run Extension). In the development
host, open any `.emel` file — for example `examples/error_handling.emel` —
and:

- break a `match` arm to see a `Non-exhaustive match` squiggle,
- type `ParseError::` to complete the error variants,
- put the cursor inside `uses { }` to complete effect names.

## Settings

- `emela.serverPath` — path to the `emela` binary (default: `emela`).
- `emela.packageRoots` — directories passed to the server as `--package`
  import roots (each must contain an `emela-package.json`). Dependencies
  declared in a project's `Pome.toml` are resolved automatically and need no
  setting.

## Package (optional)

```sh
npx @vscode/vsce package   # produces emela-<version>.vsix
code --install-extension emela-*.vsix
```
