# Glossary `[glossary]`

The terms this specification uses, with the Japanese term used in discussions (proposals and issues may be written in Japanese). When a new term is needed, add it here first.

## Requirement keywords `[glossary.keywords]`

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED", "MAY", and "OPTIONAL" in this specification are to be interpreted as described in BCP 14 (RFC 2119, RFC 8174) when, and only when, they appear in all capitals.

| English | 日本語 | Use |
|---|---|---|
| MUST, MUST NOT | 〜しなければならない、〜してはならない | An explicit requirement. Statements that describe meaning are written in the present tense and are normative without a keyword |
| SHOULD, SHOULD NOT | 〜するのがよい | A recommendation |
| MAY | 〜してよい | A permission |
| is a compile error (E0xxx) | エラーになる | The program is rejected at compile time. Always written as "compile error" so that it is not confused with typed errors (`error`, `fail`) |
| is a defect | defect になる | Abnormal termination at run time ([error.defect]) |
| is an error (of a command) | （コマンドの）エラーになる | A command reports an error and stops, for a problem that is not in the program being compiled (an invalid `Pome.lock`, an unreachable proxy). A diagnostic code is given only when the table in [diag.list] lists one |
| implementation-defined | 実装依存 | The implementation chooses and need not document the choice unless stated |
| unspecified | 規定しない | Programs must not depend on it |
| normative, informative | 規範、参考 | An informative section starts with "Informative." |

## Terms that depend on context `[glossary.context]`

| English | 日本語 | Meaning |
|---|---|---|
| suspend, suspension | 中断（suspend fn、resume） | A call that may stop and be resumed later by the handler |
| interrupt, interruption | 中断（`Fiber.interrupt`、スコープの終わり） | Stopping a fiber from outside and unwinding it |
| suspension point | 中断点 | Where fibers switch and where interruptions take effect |
| type parameter | 型引数（宣言） | The `A` in `fn f[A]` |
| type argument | 型引数（使う所） | The `Int` in `List[Int]` |
| continuation | 継続 | What `resume` resumes |
| line continuation | 行の継続 | [lex.continuation] |
| requirement, requires(H) | 依存、依存(H) | Effects a handler needs |
| dependency | 依存（Pome） | A Pome that another Pome uses |
| free | 解放（メモリ） | Returning memory |
| release | release | The cleanup block of a handler |
| version | 版 | A version of a Pome or of the compiler |
| the 0.20 series | 0.20 系 | |
| std layer | 層 | The value, effect, platform and test layers of std ([lib.layers]). A `layer` declaration is always written in code font |
| level | 層（1.2 の日常の層、基盤の層） | The everyday level and the foundation level of the language ([intro.layers]) |
| unwind | 巻き戻す（継続） | Running releases while a computation is abandoned |
| wrap around | 巻き戻る（整数のあふれ） | Two's complement overflow ([target.number]) |
| requirement | 依存（ハンドラ） | See requires(H) above |
| version requirement | 要求（Pome の version） | The minimum version a `Pome.toml` asks for ([module.pome]) |

## Terms `[glossary.terms]`

| English | 日本語 |
|---|---|
| capability, capability value | 能力、能力の値 |
| operation | 操作 |
| provide, provides(H) | 供給する、供給(H) |
| leak (a capability, a fiber, a Deferred) | 持ち出す |
| provider (the `P` of `L by P`) | 担い手 |
| underlying effect (of a label) | 元のエフェクト |
| system handler (`System*`) | 本物のハンドラ |
| test handler (`Test*`) | テスト用のハンドラ |
| effect set | 効果の集合 |
| error set | エラーの集合 |
| set type parameter | 集合の型引数 |
| hidden set variable | 見えない集合変数 |
| opaque set | 中身の分からない集合 |
| kind | 種 |
| unification, generalization, inference | 単一化、汎化、推論 |
| type annotation | 注釈（型） |
| attribute | 注釈（`@test` など） |
| constructor, variant | 構成子、バリアント |
| record update | 部分更新 |
| binding | 束縛 |
| arm, guard | 腕、ガード |
| exhaustive, redundant, refutable | 網羅、冗長、反駁可能 |
| tail call, tail position | 末尾呼び出し、末尾位置 |
| supertrait | 上位の Trait |
| default implementation | 既定の実装 |
| deriving | 導出 |
| bound (`[A: Ord]`), constraint | 制約 |
| anonymous function | 無名関数 |
| function value | 関数の値 |
| unwind, unwinding | 巻き戻す、巻き戻り |
| yield (for fairness) | 譲渡 |
| atomically (without interleaving) | 一息に |
| fairness | 公平性 |
| launch | 起動 |
| sibling | 兄弟 |
| discarded continuation | 捨てられた継続 |
| completion, cancellation | 完了、取り消し |
| queue | 待ち行列 |
| bundled core, bundled std | 同梱の core、同梱の std |
| external function | 外部関数 |
| host | ホスト |
| boundary type | 境界を越えられる型 |
| entry adapter | 入口アダプタ |
| single-file mode | 単独ファイル |
| source root, test root | ソースのルート、テストのルート |
| root name | ルート名 |
| dependency graph | 依存の木 |
| entry module, entry point | エントリ |
| output directory | 出力先 |
| build target | ビルド対象 |
| diagnostic, note, help, primary location | 診断、注記、助言、主位置 |
| reserved name, reserved word, contextual keyword | 予約名、予約語、文脈依存の語 |
| qualified | 修飾 |
| lexical | 字句 |
| terminator | 区切り |
| grapheme cluster, code point | 書記素クラスタ、コードポイント |
| static data | 静的なデータ |
| reference counting | 参照カウント |
| product type, sum type | 直積、直和 |
| set variable, opaque set variable | 集合変数、中身の分からない集合変数 |
| tag (of an error) | タグ |
| parameter, argument, argument name | 仮引数、引数、引数名 |
| positional argument, named argument | 位置引数、名前付き引数 |
| matching, scrutinee | 照合、match の対象 |
| postfix, prefix | 後置、前置 |
| precedence, associativity, short-circuit evaluation | 優先順位、結合、短絡評価 |
| unary minus | 単項マイナス |
| interpolation | 補間 |
| rebinding | 束縛し直し |
| instantiate, monomorphize | 具体化、単相化 |
| recover | 復帰 |
| combinator | 組み合わせ関数 |
| total order, lexicographic order | 全順序、辞書式順序 |
| suspending call, suspending function, non-suspending context | 中断しうる呼び出し、中断しうる関数、中断しない文脈 |
| suspending copy | 中断用の写し |
| operation implementation | 操作の実装 |
| main continuation | 本筋の継続 |
| on the spot (resume, complete) | その場で |
| the launch stalls | 起動が止まる |
| virtual time | 仮の時刻 |
| evidence table | 証拠の表 |
| plug in (with `by`), merge (`L by E`), serve (a provider serves E) | 差し込む、合流、担う |
| hold (a capability) | 抱える |
| expected type, actual type, common type | 期待する型、実際の型、共通の型 |
| lower bound, upper bound | 下界、上界 |
| principal type | 主要型 |
| row polymorphism, polymorphic recursion, monomorphic | 行多相、多相再帰、単相 |
| syntax error, lexical error | 構文のエラー、字句のエラー |
| lexing, parsing, token, terminal symbol, longest match | 字句解析、構文解析、字句、終端記号、最長一致 |
| band (of diagnostic codes), stage, message | 帯、段、文面 |
| retire (a diagnostic code) | 欠番にする |
| conformance test | 適合テスト |
| edit distance | 編集距離 |
| source text (of an expression) | 字面 |
| trailing comment, indentation, align, head, header | 行末のコメント、字下げ、揃える、頭、見出し |
| ingest, transparency log, identity, version selection, version bump | 取り込む、透明性ログ、識別、version の選び方、版の上げ方 |
| self-import, re-export, public API | 自己 import、再公開、公開 API |
| launcher module, early exit | 起動用のモジュール、途中の終了 |
| exit code, external linkage | 終了コード、外部結合 |
| completion candidate, modifier | 補完の候補、修飾子 |
| doc comment, encoding, replacement character | ドキュメントコメント、符号化、置換文字 |
| overflow, saturate | あふれ、飽和 |
| balanced tree, weight-balanced tree | 平衡木、重み平衡木 |
| request, response | 要求、応答（Http） |
| percent-encoding | パーセントエンコーディング |
| bundle (a layer of handlers), replacement | 束、差し替え |
| assertion helpers | テストの検証の部品 |
| constructed form | 構築の形 |
| segment (of a path) | 要素 |
| proleptic Gregorian calendar, leap second | 先発グレゴリオ暦、閏秒 |
| cryptographically secure random source, pseudo-random number | 暗号論的に安全な乱数源、擬似乱数 |
| design flaw | 設計の不具合 |
| last resort | 逃げ道 |

Used as they are: defect, escape, fail, fails, use, with, handler, layer, effect, Immediate, Async, Scope, fiber, Deferred, Prelude, Pome, trait, Tier.
