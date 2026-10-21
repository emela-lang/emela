# 同梱の core と std

コンパイラに同梱する `.emel` のソース（`lib/core/`，`lib/std/`）の実装の手順．API の一覧は仕様の `spec/library/api.md`（`[api]`）にある．API を足すか変えるときは，そちらを直す．

## 同梱のソースにホストのファイルを足す

同梱の `.emel` から `@external(js, "./x.mjs", "name")` でホストの関数を呼ぶときは，`.mjs` をそのソースと同じディレクトリ（`lib/std/` など）に置いて埋め込む．

1. `lib/std/x.mjs` を書く．別の `.mjs` を `./y.mjs` のように相対で import してよい
2. `src/sources.rs` の `HOST_FILES` に `("std/x.mjs", include_str!("../lib/std/x.mjs"))` を足す．パスは `lib/` からの相対で `/` 区切り．載せ忘れはテスト `host_files_match_lib_dir` が見つける
3. `.emel` に `@external(js, "./x.mjs", "name")` を書く．パスはそのソースのディレクトリからの相対

build，run，test では，lowering がその指定を `lib/` からのパス（`std/x.mjs`）にし，JS の出力はそれを `./emela_core/std/x.mjs` として import する．出力先（`target/emela/js/` など）の `emela_core/` の下に同梱の `.mjs` を全部並べて書き出す．どの外部関数も同梱のファイルを指さなければ何も書き出さない．表にないファイルを指すと内部の誤りになる．

C のターゲット（`[ext.decl]`）も同じ手順で，`lib/std/x.c` を書いて `HOST_FILES` に足し，`.emel` に `@external(c, "./x.c", "name")` を書く．suspend fn の外部関数は `[ext.c-host]` の形（最後に `em_task *` を受け取り，`em_complete_*` で完了を知らせる）で書き，出力先に書き出す `emela.h` を include する．C の出力は，使う外部関数が指す `.c` だけを出力先（`target/emela/c/`）の `emela_core/` の下に書き出し，`main.c` と一緒に cc でコンパイルする．関数のプロトタイプは `[ext.boundary]` の表で決まる（String は `const char *` で受け，返すときは `malloc` した `char *`）．`-std=c11` でコンパイルするので，POSIX の関数を使うときは機能検査マクロに気をつける．
