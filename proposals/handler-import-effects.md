---
title: Importing a handler brings its effects
authors: [Satoru314]
status: draft
issue:
touches: [module.import, module.resolution]
requires: []
related: [handler-member]
---

# Importing a handler brings its effects

## Summary

Listing a handler in an import also imports the effects that the handler implements. `import Std.Console.{SystemConsole}` makes `Console` usable unqualified.

## Motivation

### effect と handler が並列に import されていて、読みにくく気持ちが悪い

今の hello world はこう書く。

```emel
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    console <- Console
    console.print_line("Hello, world!")
  }
}
```

`Console` と `SystemConsole` は役割がまったく違う。`Console` は effect で、操作の名前とシグネチャを宣言するだけで、中身を持たない。コードはこれに「頼む」。`SystemConsole` は handler で、その操作を実際に受け持つ実装である。`with` に渡すのはこちらになる。

ところが import の行では、この 2 つが同じ並びに、区別なく置かれる。読み手には、どちらが宣言でどちらが実装なのか、行から読み取れない。std は effect とその system / test handler を同じモジュールに置くので（`[lib.layers]`）、この形はほぼすべてのプログラムの先頭に現れる。最初に書く hello world からそうなっていることが、読みにくく、気持ちが悪い。

### effect を別に import するのは、本質的に必要ないはず

handler の宣言 `handler SystemConsole: Console` は、自分が実装する effect をすでに名指ししている。handler を `with` で使うコードは、その effect の操作を呼ぶためにそうしている。したがって、どの effect を使うかは handler から分かる。それなのに effect の名前を import にもう一度書かせるのは、同じ情報を 2 回書かせているだけである。

この提案のあとは、こう書ける。

```emel
import Std.Console.{SystemConsole}

fn main() {
  with SystemConsole {
    console <- Console
    console.print_line("Hello, world!")
  }
}
```

### 今できる回避策

`import A.B.{x}` は B もモジュールとして scope に入れる（`[module.resolution]`）。なので `import Std.Console.{Console}` だけで `with Console.SystemConsole { ... }` と書けるはずである（コンパイラではまだ確かめていない）。ただしこれは、モジュール名と effect 名がたまたま同じであることに頼っており、読み手には分かりにくい。

## Proposal

### Meaning

- When an import lists a handler H, each effect named after `:` in the declaration of H is imported as if it were listed in the same import.
- The rest of the import is unchanged: names that are not listed and are not effects of a listed handler are not imported.

### 同じ effect を実装する handler を同時に import したとき

`import Std.Console.{SystemConsole, TestConsole}` のように、同じ effect を実装する handler を 2 つ import すると、どちらも `Console` を連れてくる。これは同じ定義なので、競合しない。同じ定義を 2 回 import しても重複にはならない（`[module.resolution]`）。

競合するのは、別々のモジュールにある同じ名前の別の effect を、それぞれの handler が連れてくるときである。

```emel
import Std.Console.{SystemConsole} # Std.Console.Console を連れてくる
import MyLog.{FileConsole} # MyLog.Console を実装しているとする
```

このとき `Console` は 2 つの異なる定義を指すので、今の規則のままなら E0213 になる。ユーザは import の行に `Console` を一度も書いていないのに、そのエラーを受けることになる。扱いは Open questions に置く。

## Diagnostics

- [ ] Conflicts between an effect imported through a handler and another definition of the same name (see Open questions)

## Compatibility

A program that defines or imports another definition with the same name as an effect implemented by an imported handler may get E0213 (see Open questions). Other programs keep their meaning; listing the effect explicitly stays valid, and importing the same definition twice is not a duplicate.

## Implementation

Name resolution of imports (`emela-resolve`): after resolving a listed handler, add the effects of its declaration to the import's names.

## Alternatives

- Importing an effect brings every handler of the same module. More names come in than the import shows, and both `SystemConsole` and `TestConsole` are imported.
- Naming handlers under their effect (`with Console.System { ... }`). This collides with handler members (`E.M`, `[effect.with]`).
- No change, and documenting the workaround above as the recommended form.

## Prior art

## Open questions

- Is an exception to "only the names listed in braces" (`[module.import]`) acceptable?
- When the effect is in another module than the handler (a user handler implementing `Std.Console.Console`), is it imported too? What if the effect is not pub?
- A handler that implements several effects (`[effect.handler]`): are all of them imported?
- A layer (`Std.Platform.Platform`): are the effects of its handlers imported?
- When two handlers bring different effects of the same name, or another definition of the same name is already in scope: is it E0213, or is the implicit effect dropped and only the qualified name left?
- Are the effects of handler members included?

## Decision
