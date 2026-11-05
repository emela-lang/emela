# Changelog

The version of the extension follows the version of the `emela` binary it was released with.

## 0.25.0

The first release on the Visual Studio Marketplace.

- Syntax highlighting follows the current grammar: `#` comments, attributes (`@export`,
  `@entry`, `@test`, ...), string interpolation, multi-line strings, effects, handlers and
  layers.
- The command **Emela: Restart Language Server**.
- A status bar item with the state of the language server. Clicking it restarts the server.
- Changing `emela.serverPath` restarts the server.
- The `emela` binary is looked up on `PATH` and then in `~/.emela/bin`. When it is not found, a
  notification links to the install instructions and the setting.
- Removed `emela.packageRoots`. It passed `--package` arguments, which `emela lsp` no longer
  takes, and kept the server from starting.
