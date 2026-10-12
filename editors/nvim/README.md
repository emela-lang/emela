# Emela for Vim and Neovim

Syntax highlighting and filetype settings for Emela 0.20 (`.emel` files). Plain Vim script, so
the same files work in Vim 9 and Neovim. No Tree-sitter.

- `ftdetect/emela.vim`: `*.emel` is the `emela` filetype
- `syntax/emela.vim`: highlighting
- `ftplugin/emela.vim`: `#` comments, two-space indentation as `emela fmt` writes it, and `gf` on
  a module path (`Http.Client` opens `src/http/client.emel`)

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
| `emelaStructure` | `type` `enum` `error` `trait` | `Structure` |
| `emelaEffectDecl` | `effect` `handler` `layer` | `Structure` |
| `emelaEffect` | `use` `with` | `Statement` |
| `emelaStorageClass` | `pub` `opaque` `suspend` | `StorageClass` |
| `emelaInclude` | `import` | `Include` |
| `emelaConditional` | `if` `else` `match` | `Conditional` |
| `emelaException` | `fail` `escape` | `Exception` |
| `emelaStatement` | `assert` | `Statement` |
| `emelaSelf` | `self` `Self` | `Special` |
| `emelaContextual` | `implements` `for` `fails` `derive` `init` `release` in their positions | `Keyword` |
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

The inside of `#{ ... }` is highlighted as ordinary code. Lexical errors (`1e9`, `0XFF`,
`Http_client`, an unknown escape `\q`) are linked to `Error`.

## Test

```sh
editors/nvim/test/run.sh          # Neovim
editors/nvim/test/run.sh vim      # Vim
EMELA_SPEC=path/to/emela-0.20-spec.md editors/nvim/test/run.sh   # also the spec's examples
```

The script opens `test/fixture.emel` and checks the syntax group at chosen positions, then
checks that nothing in `examples/` and `crates/emela-fmt/test_data/` is highlighted as an error.
