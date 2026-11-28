---
title: Go to the definitions in the bundled core and std
authors: [Satoru314]
status: draft
issue:
touches: [lsp.navigation, lsp.bundled-source]
requires: []
related: []
---

# Go to the definitions in the bundled core and std

## Summary

`textDocument/definition` and `textDocument/typeDefinition` return locations in the bundled core and std. The server writes their sources to files of its own, for the version of the compiler, and points into those files.

## Motivation

- 同梱の core と std はバイナリに埋め込まれていて、ファイルがない（`[lib.layers]`）。そのため `[lsp.navigation]` は、それらの定義へのジャンプで `null` を返す。
- プログラムを読むときは、使っている宣言を読みに行く。たとえば `with SystemConsole` の `SystemConsole` から `handler SystemConsole: Console` へ飛び、そこから `effect Console` へ飛べば、Console にどんな操作があり、どんなシグネチャなのかが分かる。effect が何をできるのかを知るには、これがいちばん直接的な方法である。
- 今はそれができないので、読み手はリポジトリの中の `crates/emela-core/lib/std/console.emel` か、仕様の `[api.std.console]` を自分で探すしかない。リポジトリを持っていない利用者は、仕様を探すしかない。さらに、そこに書いてあるものが手元のコンパイラの版と同じだという保証もない。
- hover では `##` コメントが出る（`[lsp.doc]`）。しかし宣言そのものや、周りの宣言（handler が実装する effect、effect の他の操作）は見えない。
- 自分で書いたコードや依存する Pome の定義には飛べるのに、標準ライブラリの定義にだけ飛べない。この非対称は、言語を学び始めた人がいちばん最初に触る `Std.Console` で目立つ。

## Proposal

### Meaning

- For a definition in the bundled core or std, `textDocument/definition` returns a location in a copy of its source (`[lsp.navigation]`). `textDocument/typeDefinition` does the same for a type in the bundled core or std.
- The server writes the copies under a directory for the version of the compiler, the first time a request returns a location in them. A file that already exists with the same contents is not written again (`[lsp.bundled-source]`).
- The copies are only destinations. The server does not read them for checking, does not publish diagnostics for them, and does not include them in references or symbols. Changes to them do not change the bundled core and std.
- `textDocument/references` is unchanged: references inside the bundled core and std are not returned.

## Diagnostics

None.

## Compatibility

Clients that received `null` receive a location. No program changes meaning.

## Implementation

`emela-lsp` and `emela-driver`: keep the path of each bundled module, write its source on first use, and map spans in bundled modules to that path.

## Alternatives

- A URI scheme of its own (`emela-bundled:`) whose contents are served by a custom request. Nothing is written to disk, but each editor extension has to support the request, and editors without an extension cannot open the location.
- Pointing to the sources in a checkout of the repository. Most users do not have one, and its version may differ from the compiler's.
- No change: hover already shows the documentation.

## Prior art

## Open questions

- Where is the directory? (A cache directory of the OS, `~/.emela/bundled/<version>/`, or the project's `target/`.)
- When the directory cannot be written, does the server return `null` as now?
- When a client opens a copy (`didOpen`), is it treated as a single file, or ignored? Today a file outside the project is checked as a single file (`[lsp.project]`), which would report reserved names (E0203).
- Are the copies made read-only on the file system?
- Does `emela doc` or another command need the same files?

## Decision
