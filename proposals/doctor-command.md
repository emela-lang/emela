---
title: emela doctor
authors: [Alliana]
status: draft
pr: 535
issue:
decided:
touches: [cmd, cmd.doctor (new), target.c-embedded.board]
requires: []
related: []
---

# emela doctor

## Summary

各ターゲットが使う外部プログラム（node，cc，wasmtime，組込み向けのツール）と Pome の取得経路を，emela のコマンドが使う規則どおりに解決して，足りないものを報告するコマンド `emela doctor` を足す．

## Motivation

- 外部プログラムが足りないことは，今は使った時点でしか判らない．`emela run --target wasm` を実行して初めて E0909 が出る．ターゲットを切り替えるたびに 1 つずつ見つけることになる
- 実行ファイルの解決規則（`EMELA_NODE`，`EMELA_CC`，`EMELA_WASMTIME`，`EMELA_PYTHON`，`OPENOCD`，`EMELA_ESPTOOL_PYTHON`）が仕様のあちこちに散っていて，今どの実行ファイルが使われるかを確かめる手段がない
- バグ報告に環境を書いてもらうとき，`emela --version` と各ツールの版を個別に頼むしかない
- 今ある回避策は各ツールの `--version` を手で叩くこと．ただし `EMELA_*` による差し替えは反映されず，版の下限も自分で照らし合わせる必要がある

## Proposal

### Meaning

- `emela doctor [path]` は，ターゲットごとに必要な外部プログラムを，そのターゲットのコマンドと同じ規則で解決し，起動して版を読み，結果をターゲットごとにまとめて標準出力に書く
- 足りないものは「このターゲットにはこれが要るが，見つからない（または古い）」という文と，入れ方の案内で報告する．診断コードは付けない
- 何もインストールせず，環境変数も書き換えず，ネットワークにも出ない
- プロジェクトの中で実行すると，そのプロジェクトのターゲットに要る項目を「必要」とする．プロジェクトの外ではすべて「任意」とする．`--target` と `--board` で指定もできる
- 終了コードは，「必要」の項目がすべて満たされれば 0，そうでなければ 1
- `--format json` で同じ内容を JSON で書く
- Pome の取得経路として，`EMELA_POMEPROXY`，`EMELA_POMESUMDB`，`EMELA_POMEPRIVATE` の実効値と git の有無を報告する．proxy への接続は試さない

出力の例（プロジェクトのターゲットが `wasm` のとき）:

```text
emela 0.28.0+4e84642

js
  ok       node 24.11.0 (/opt/homebrew/bin/node)
c
  ok       cc (/usr/bin/cc)
wasm (required)
  missing  target `wasm` needs wasmtime 49 or later, but `wasmtime` is not on PATH
           install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable

pome
  proxy    https://pomes.emela.dev, direct
  sumdb    https://sum.emela.dev
  private  0 prefixes
  ok       git 2.51.0 (/usr/bin/git)
```

### Node の版

node は `[target]` が前提とする版（別の fix で 22 以上に上げる予定）を満たすかを確かめる．

### cc の版

cc は起動できるかと，`--version` の 1 行目を報告する．C11 を受け付けるかまでは確かめない．

## Specification changes

### `[target.c-embedded.board]`

Replace the last item with:

```markdown
- Making the firmware image needs Python 3 (the environment variable `EMELA_PYTHON`, else `python3`) and the cross toolchain of the board: `arm-none-eabi-gcc` for the STM32F4 boards (in the `bin` directory of the environment variable `ARM_TOOLCHAIN_PATH`, else on PATH), and `riscv32-esp-elf-gcc` and the Python package esptool for `esp32c3` (in the `bin` directory of the environment variable `EMELA_ESP32C3_TOOLCHAIN`, else on PATH; esptool is imported by the Python of the environment variable `EMELA_ESPTOOL_PYTHON`, else by the Python of the build). The compiler does not fetch them.
```

### `[cmd]`

Replace the second sentence of the introduction:

```markdown
`emela test` is in 13.4 and `[cmd.test]`, `emela pome` is in `[cmd.pome]`, `emela flash` is in `[cmd.flash]`, and `emela doctor` is in `[cmd.doctor]`.
```

### `[cmd.doctor]` (new)

```markdown
## emela doctor `[cmd.doctor]`

- Reports, for each build target, whether the external programs it uses can be found and started and whether their versions are new enough, and reports the routes used to fetch Pomes. It installs nothing, changes no environment variables, and does not go to the network.
- Takes one path, which defaults to `.`. When the path is in a project (4.1), the build target of the project (`[cmd.build]`) is required, and so is its board (`[target.c-embedded.board]`) when the target is `c-embedded`. Outside a project, no target is required. `--target <js|c|c-embedded|wasm>` makes that target required instead, and `--board <id>` selects the board.
- Each program is resolved by the same rule as the command that uses it, and is checked as follows.

  | Target | Program | Rule | Passes when |
  |---|---|---|---|
  | `js` | node | `[cmd.build]` | it starts and `--version` reports a version that `[target]` supports |
  | `c` | the C compiler | `[target.c.cc]` | it starts; the first line of its `--version` is reported as its version |
  | `wasm` | wasmtime | `[target.wasm.run]` | it starts and its major version is 49 or later |
  | `c-embedded` | Python | `[target.c-embedded.board]` | it starts and reports version 3 |
  | `c-embedded`, STM32F4 boards | `arm-none-eabi-gcc`, OpenOCD | `[target.c-embedded.board]`, `[cmd.flash]` | each starts |
  | `c-embedded`, `esp32c3` | `riscv32-esp-elf-gcc`, esptool | `[target.c-embedded.board]`, `[cmd.flash]` | each starts |

  The items of `c-embedded` are checked only when a board is selected.
- A program passes or fails with one of these reasons: it was not found, it could not be started, its version is older than required, or its version could not be read. A program whose version could not be read passes, as in `[target.wasm.run]`.
- The Pome section reports the values in effect of `EMELA_POMEPROXY` and `EMELA_POMESUMDB`, the number of prefixes in `EMELA_POMEPRIVATE` (`[module.pome]`), and whether git starts. The prefixes themselves are not written, so that the output can be pasted into a public report. git is required when `EMELA_POMEPRIVATE` is set or when `direct` is the only entry of `EMELA_POMEPROXY`. The userinfo part of a URL (`user:token@`) is written as `***@`. Nothing is fetched and no proxy is contacted.
- Output without `--format`, written to standard output:
  - The first line is the same as `emela --version`, followed by a blank line.
  - For each target in the order `js`, `c`, `wasm`, `c-embedded`, a line with the target's name, followed by ` (required)` when required. Under it, one line per program, indented by two spaces: `ok`, `missing`, `too-old` or `failed`, then the program's name and version and the executable resolved in parentheses, or a sentence that names the target and the program that it lacks. A program that fails is followed by a line with the help of the diagnostic that the command using it would report (E0904, E0907, E0909), aligned with the sentence.
  - Then a blank line and the Pome section in the same form.
  - Color is used under the same conditions as diagnostics (`[diag]`).
- With `--format json`, the command writes one JSON object to standard output instead:

  ```json
  {
    "emela": "0.28.0+4e84642",
    "targets": [
      {
        "target": "wasm",
        "required": true,
        "programs": [
          {
            "name": "wasmtime",
            "status": "missing",
            "executable": "wasmtime",
            "version": null,
            "message": "target `wasm` needs wasmtime 49 or later, but `wasmtime` is not on PATH",
            "help": "install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable"
          }
        ]
      }
    ],
    "pome": {
      "proxy": ["https://pomes.emela.dev", "direct"],
      "sumdb": "https://sum.emela.dev",
      "private": 0,
      "git": { "name": "git", "status": "ok", "executable": "/usr/bin/git", "version": "2.51.0", "message": null, "help": null }
    }
  }
  ```

  `status` is one of `ok`, `missing`, `too-old` and `failed`. `executable` is the absolute path when the program was found, and the name that was looked up otherwise. Keys may be added in later versions; existing keys keep their meaning.
- The exit code is 0 when every program of every required target passes and git passes when it is required, and 1 otherwise.
```

## Diagnostics

- [ ] 新しい診断コードは作らない

## Compatibility

- `emela doctor` は新しいコマンドで，既存のコマンドの挙動は変えない

## Implementation

- 解決規則は `emela-driver` の既存の関数（node，cc，wasmtime の解決と，wasmtime の版の読み取り）を使う．doctor 用に別の規則を書かない
- 組込み向けの項目は，今は board の `build.py` が解決している（`ARM_TOOLCHAIN_PATH`，`EMELA_ESP32C3_TOOLCHAIN`）．同じ規則を Rust 側にも持つことになるので，両方を同じテストで縛る
- README のコマンド表に `emela doctor` の行を足す
- `.github/ISSUE_TEMPLATE/bug_report.yml` の環境欄で `emela doctor` の出力を頼む

## Alternatives

- `emela --version --verbose` に混ぜる．`--version` の出力は 1 行であることをスクリプトが前提にしているので，混ぜると壊れる
- 何もしない（使ったときの E0904，E0907，E0909 に任せる）．ターゲットを切り替えるたびに 1 つずつしか見つからない
- 結果を診断（`error[E09xx]`）として出す．ソースに位置を持たない情報を診断の枠に入れることになり，`N errors` の集計とも合わない
- proxy への到達を確かめる．ネットワークに出ることになり，`EMELA_POMEPRIVATE` の設定次第では非公開のソースパスが外へ出かねない．設定値を出すだけにとどめる

## Prior art

`flutter doctor`，`brew doctor`，`react-native doctor` が同じ名前で，必要なツールの有無と版を一覧で報告する．

## Open questions

- 

## Decision
