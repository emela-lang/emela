# Emela for Vim and Neovim

Syntax highlighting and filetype settings for Emela 0.20 (`.emel` files). Plain Vim script, so
the same files work in Vim 9 and Neovim. No Tree-sitter.

- `ftdetect/emela.vim`: `*.emel` is the `emela` filetype
- `syntax/emela.vim`: highlighting
- `ftplugin/emela.vim`: `#` comments, two-space indentation as `emela fmt` writes it, and `gf` on
  a module path (`Http.Client` opens `src/http/client.emel`)
- `plugin/emela.vim`: in Neovim, underlines capability values reported by the language server
  (see below)

## Install

### lazy.nvim

Point `dir` at this directory in a local checkout:

```lua
{
  dir = "~/src/emela/editors/nvim",
  name = "emela.vim",
  ft = "emela",
}
```

With `ft = "emela"` the plugin loads on the first `.emel` buffer; lazy.nvim reads `ftdetect/`
up front, so detection works before that.

### By hand

Add the directory to `runtimepath`, before `filetype plugin on` and `syntax on` run:

```vim
" ~/.vimrc or init.vim
set runtimepath^=~/src/emela/editors/nvim
filetype plugin on
syntax on
```

```lua
-- init.lua
vim.opt.runtimepath:prepend(vim.fn.expand("~/src/emela/editors/nvim"))
```

Neovim enables filetype plugins and syntax by default. Vim's `packadd` also works if you link
this directory into `~/.vim/pack/emela/start/`.

## Highlight groups

Every group is linked with `highlight default link`, so a color scheme or your config can
override any of them. The groups are split by meaning:

| Group | Words | Linked to |
|---|---|---|
| `emelaKeyword` | `fn` `const` `impl` | `Keyword` |
| `emelaStructure` | `type` `enum` `error` `trait` | `Keyword` |
| `emelaEffectDecl` | `effect` `handler` `layer` | `Keyword` |
| `emelaEffect` | `use` `with` | `Statement` |
| `emelaStorageClass` | `pub` `opaque` `suspend` | `Keyword` |
| `emelaInclude` | `import` | `Include` |
| `emelaConditional` | `if` `else` `match` | `Conditional` |
| `emelaException` | `fail` `escape` | `Exception` |
| `emelaStatement` | `assert` | `Statement` |
| `emelaSelf` | `self` `Self` | `Special` |
| `emelaContextual` | `fails` `derive` `init` `release` in their positions | `Keyword` |
| `emelaReserved` | `as` and the words reserved for the future (`let` `return` ...) | `Error` |
| `emelaAnnotation` | `@external` `@test` `@intrinsic` `@internal` `@deprecated` | `PreProc` |
| `emelaComment` / `emelaDocComment` | `#` / `##` | `Comment` / `SpecialComment` |
| `emelaString` / `emelaEscape` / `emelaInterpDelim` | `"..."` `"""` / `\n` `\u{...}` / `#{` `}` | `String` / `SpecialChar` / `Delimiter` |
| `emelaNumber` / `emelaFloat` | `42` `0xFF` `0b1010` / `3.14` `1.0e9` | `Number` / `Float` |
| `emelaBoolean` | `True` `False` | `Boolean` |
| `emelaType` / `emelaTypeDecl` | type names / the name a declaration introduces | `Type` / `Typedef` |
| `emelaConstructor` | a type name applied to `(` | `Function` |
| `emelaConstant` / `emelaTypeParam` | upper names / single capitals and declared type parameters | `Constant` / `Identifier` |
| `emelaOperator` | `\|>` `->` `..` `==` `&&` `\|` ... | `Operator` |

Declarations and modifiers are linked to `Keyword`, so they get the same color as `fn` (some
color schemes leave `Structure` and `StorageClass` uncolored). To tell them apart again, link
the groups yourself:

```vim
highlight link emelaEffectDecl Structure
```

The inside of `#{ ... }` is highlighted as ordinary code. Lexical errors (`1e9`, `0XFF`,
`Http_client`, an unknown escape `\q`) are linked to `Error`.

## Capability values (Neovim and the language server)

A capability value is a variable bound to `use X` (`log = use Logger`). Neovim underlines it
where it is bound and wherever it is used, including inside closures. Which names hold a
capability is only known after name resolution and type checking, so this needs `emela lsp`;
the regex-based syntax file cannot tell. Vim gets no underline.

Start the server for `.emel` buffers:

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "emela",
  callback = function(args)
    vim.lsp.start({
      name = "emela",
      cmd = { "emela", "lsp" },
      root_dir = vim.fs.root(args.buf, { "Pome.toml" }),
    })
  end,
})
```

To try a local build of the server, run `cargo build -p emela-cli` and put the absolute path of
`target/debug/emela` in `cmd` instead of `"emela"`.

The server sends semantic tokens of type `variable` with the modifier `capability`, and only
those, so the rest of the highlighting stays with the syntax file. Neovim shows them through
the group `@lsp.typemod.variable.capability.emela`, which `plugin/emela.vim` sets to underline
with `highlight default`. Override it in your config or color scheme:

```lua
-- A color instead of an underline.
vim.api.nvim_set_hl(0, "@lsp.typemod.variable.capability.emela", { fg = "#d7875f", bold = true })
-- No highlighting at all.
vim.api.nvim_set_hl(0, "@lsp.typemod.variable.capability.emela", {})
```

Set it after your color scheme loads (or from a `ColorScheme` autocommand), since loading a
color scheme clears it.

## Test

```sh
editors/nvim/test/run.sh          # Neovim
editors/nvim/test/run.sh vim      # Vim
EMELA_SPEC=path/to/emela-0.20-spec.md editors/nvim/test/run.sh   # also the spec's examples
```

The script opens `test/fixture.emel` and checks the syntax group at chosen positions, then
checks that nothing in `examples/` and `crates/emela-fmt/test_data/` is highlighted as an error.
It also checks the links of the declaration keywords and, in Neovim, the capability underline.
The semantic tokens themselves are tested in `crates/emela-lsp/tests/server.rs`.
