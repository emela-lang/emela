---
title: Self(...) in the init of a handler
authors: [Alliana]
status: draft
pr: 623
issue:
decided:
touches: [effect.handler, kw.reserved, grammar.expr, diag.list]
requires: []
related: [handler-init-arguments]
---

# Self(...) in the init of a handler

## Summary

handler の init の中で，自分の値を `Self(...)` と書いて作れるようにする．`H(...)` と同じ意味で，handler の名前を繰り返さずに済む．

## Motivation

init の最後で自分の値を作る式は，with の項目で init を呼ぶ式と同じ形をしている（`[effect.handler]`）．

```emel
handler PostgresUsers(conn: Connection): Users {
  init(url: String) {
    PostgresUsers(conn: pg_connect(url)) # the value of this handler
  }
}

with PostgresUsers(url: "...") { ... } # a call of init
```

handler member（`handler-member`）を入れると，同じ名前が init の中で2つの意味で並ぶ．

```emel
handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) {
      Open(inner: Files.Open(path)) # Open(...) makes this value; Files.Open(...) opens the outer member
    }
  }
}
```

`Open(inner: ...)` は値を作り，`Files.Open(path)` は外側の init を呼んで資源を開く．見た目が近いのに意味が違う．

## Proposal

### Syntax

```emel
handler PostgresUsers(conn: Connection): Users {
  init(url: String) {
    Self(conn: pg_connect(url))
  }
}

handler TracedFiles: Files {
  handler Open(inner: use ReadFile): ReadFile {
    init(path: String) {
      Self(inner: Files.Open(path))
    }
  }
}
```

### Meaning

- handler の init の中の `Self(fields)` は，その handler の値を作る．`H(fields)` と同じ意味
- フィールドのない handler では `Self` と書く（`handler H()` なら `Self()`）．今の `ConsoleLogger`，`H()` と同じ
- 書ける場所は，`H(fields)` と同じく init の中（init の中の無名関数を含む）だけ．それ以外は E0418
- `H(fields)` も今のまま書ける

### Typing rules

- `Self(fields)` の型は，その handler の型．フィールドの検査は `H(fields)` と同じ

## Specification changes

### `[effect.handler]`

Add after the item on E0418:

> - Inside the init of a handler, its value can also be constructed as `Self(...)` (`Self` for a handler without parentheses). It has the same meaning as writing the handler's name, and the same restriction (E0418).

### `[grammar.expr]`

```ebnf
primary    = ...
           | "Self" [ "(" [ args ] ")" ]                    (* the value of a handler, inside its init; 8.4 *)
```

## Diagnostics

- [ ] `Self(...)` outside the init of a handler: E0418 inside a handler, E0219 elsewhere

## Compatibility

追加だけ．今 `Self` は型の位置にしか書けないので，式の `Self` は今は構文エラーである．

## Implementation

- emela-syntax: 式の `Self`
- emela-resolve: init の中の `Self` を，その handler の構築子に解決する
- emela-fmt: そのまま出力する（`H(...)` から `Self(...)` への書き換えはしない）

## Alternatives

- 書けるようにしない: handler member の init で，構築と member を開く式が近い見た目で並ぶ
- 小文字の `self(...)`: `self` は値で，init の中では使えない（値がまだないため）．構築に使うと `self.conn` と紛れる
- 構築の専用の構文（`return { conn: ... }` など）: 今の `H(fields)` とレコードの構築の形から離れる

## Prior art

- Referring to the enclosing type by a fixed name inside its own definition, so that a constructor call does not repeat the type's name.

## Open questions

- 構築子のパターン（`Self(conn:)`）とレコードの更新（`Self(n: 1, ..self)`）にも `Self` を許すか．更新は今も操作の本体では書けない（E0418）
- trait と impl の中で `Self(...)` を型の構築に使えるようにするか．この提案は handler の init に限る
- `emela fmt` や lint で，init の中の `H(...)` を `Self(...)` に寄せるか

## Decision
