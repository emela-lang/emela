# Syntax highlighting

Emela source files use the `.emel` extension. This repository ships a
[Sublime Text syntax definition](../editors/emela.sublime-syntax) at
`editors/emela.sublime-syntax`, which covers comments, strings, numbers,
keywords, built-in types, and operators.

The same `.sublime-syntax` file works with any tool built on the
[syntect](https://github.com/trishume/syntect) library — most notably the
[`bat`](https://github.com/sharkdp/bat) pager — as well as Sublime Text itself.

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

## Sublime Text

Copy `editors/emela.sublime-syntax` into your Sublime Text `Packages/User`
directory (Preferences → Browse Packages…). Sublime loads it on save, and
`.emel` files are highlighted from then on.
