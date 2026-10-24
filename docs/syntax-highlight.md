# Syntax highlighting

Emela source files use the `.emel` extension. Editors don't share one
highlighting format, so this repository ships several definitions under
`editors/`, each covering comments, strings, numbers, keywords, built-in types,
and operators:

- [`editors/emela.sublime-syntax`](../editors/emela.sublime-syntax) — a Sublime
  Text definition, also read by any [syntect](https://github.com/trishume/syntect)-based
  tool such as the [`bat`](https://github.com/sharkdp/bat) pager.
- [`editors/nvim/`](../editors/nvim) — a Vim/Neovim syntax plugin
  (`syntax/emela.vim` plus `ftdetect/emela.vim` for `.emel` filetype detection).
- [`editors/tree-sitter-emela/`](../editors/tree-sitter-emela) — a
  [tree-sitter](https://tree-sitter.github.io/) grammar with highlight, fold,
  indent and text-object queries, used by Neovim and Zed.
- [`editors/zed/`](../editors/zed) — a Zed extension built on that grammar.

## `bat`

Copy the definition into `bat`'s syntax directory and rebuild its cache:

```sh
mkdir -p "$(bat --config-dir)/syntaxes"
cp editors/emela.sublime-syntax "$(bat --config-dir)/syntaxes/"
bat cache --build
```

Verify that `bat` picked it up:

```sh
bat --list-languages | grep Emela      # => Emela:emel
bat examples/hello.emel                # highlighted
```

`bat` selects the syntax from the `.emel` extension automatically. To force it
(for example when reading from stdin), pass `--language=Emela`:

```sh
cat examples/hello.emel | bat --language=Emela
```

To pick up later changes to the definition, edit
`editors/emela.sublime-syntax`, copy it over again, and re-run `bat cache
--build`.

## Neovim / Vim

The `editors/nvim/` directory is a runtimepath plugin. With a plugin manager,
point it at that directory — for example with
[lazy.nvim](https://github.com/folke/lazy.nvim):

```lua
{ dir = "/path/to/emela/editors/nvim", ft = "emela" }
```

Or install the two files by hand into your config directory (`~/.config/nvim`
on Neovim, `~/.vim` on Vim):

```sh
mkdir -p ~/.config/nvim/syntax ~/.config/nvim/ftdetect
cp editors/nvim/syntax/emela.vim   ~/.config/nvim/syntax/
cp editors/nvim/ftdetect/emela.vim ~/.config/nvim/ftdetect/
```

Open any `.emel` file and confirm the filetype and highlighting are active:

```vim
:set filetype?
" filetype=emela

:echo synIDattr(synIDtrans(synID(line('.'), col('.'), 1)), 'name')
" e.g. Keyword / Type / String under the cursor
```

In Neovim, the tree-sitter grammar in `editors/tree-sitter-emela` gives
incremental parsing and more precise highlights. Once its parser is installed,
the filetype plugin uses it instead of the Vim syntax file. See
[`editors/nvim/README.md`](../editors/nvim/README.md#tree-sitter-neovim) for
installing it with nvim-treesitter or by hand.

## Zed

Install [`editors/zed/`](../editors/zed) as a dev extension (`zed: install dev
extension` in the command palette). See
[`editors/zed/README.md`](../editors/zed/README.md).

## Sublime Text

Copy `editors/emela.sublime-syntax` into your Sublime Text `Packages/User`
directory (Preferences → Browse Packages…). Sublime loads it on save, and
`.emel` files are highlighted from then on.
