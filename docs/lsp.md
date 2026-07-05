# Language server (`emela lsp`)

The compiler binary doubles as an LSP server (spec 0033):

```sh
emela lsp [--package DIR ...]
```

It speaks LSP over stdio and provides:

- **Diagnostics** on open/change/save, covering every error the compiler can
  emit — lexer, parser, imports, and the type checker (type errors, missing
  trait methods, unhandled effects, `throws` mismatches, non-exhaustive
  matches, …). Errors are collected across declarations, so independent
  mistakes all show at once.
- **Completion**, context-aware: `import` paths, enum variants in `match`
  arms (typed from the scrutinee where possible), error variants in `catch`
  arms, effect names inside `uses { … }`, variants after `Enum::`, and the
  default mix of keywords, types, functions, and locals.

Unsaved buffers are used during import resolution, so cross-file diagnostics
track what the editor sees, not what is on disk. `--package` has the same
meaning as for `emela check`; dependencies from a project's `Pome.toml` are
resolved automatically per file.

## Neovim

Filetype detection ships in this repository (`editors/nvim/`, see
[syntax-highlight.md](syntax-highlight.md)); if you don't use that plugin,
register the extension yourself:

```lua
vim.filetype.add({ extension = { emel = "emela" } })
```

Then define and enable the server with the built-in LSP client (Neovim 0.11+):

```lua
vim.lsp.config("emela", {
  cmd = { "emela", "lsp" },
  filetypes = { "emela" },
  root_markers = { "Pome.toml", ".git" },
})
vim.lsp.enable("emela")
```

For nvim-lspconfig, the equivalent custom-server registration:

```lua
require("lspconfig.configs").emela = {
  default_config = {
    cmd = { "emela", "lsp" },
    filetypes = { "emela" },
    root_dir = require("lspconfig.util").root_pattern("Pome.toml", ".git"),
  },
}
require("lspconfig").emela.setup({})
```

Pass `--package` roots by extending `cmd`, e.g.
`cmd = { "emela", "lsp", "--package", "/path/to/stdlib" }`.

Open any `.emel` file and check `:LspInfo` shows the client attached;
`:lua vim.diagnostic.open_float()` shows the diagnostic under the cursor and
`<C-x><C-o>` (or your completion plugin) triggers completion.

## VSCode

A minimal client extension lives in [`editors/vscode/`](../editors/vscode).
See its [README](../editors/vscode/README.md) for the develop-and-run (F5) and
packaging steps. It expects `emela` on `PATH` (setting: `emela.serverPath`)
and forwards `emela.packageRoots` as `--package` flags.

## Other editors

Any LSP client works: run `emela lsp` over stdio for files of type `emela`
(extension `.emel`), with `Pome.toml` as the workspace root marker.
