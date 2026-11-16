---
title: Predicate function names with ?
authors: []
status: draft
pr: 618
issue:
decided:
touches: [lex.name, grammar.lex, grammar.decl, grammar.expr, module.import, module.resolution, fn.decl, fn.local, trait.decl, trait.impl, trait.call, effect.decl, effect.handler, effect.use, type.rules, ext.decl, ext.export, ext.entry, fmt.space, lsp.completion, lsp.navigation, lsp.rename, diag.list]
requires: []
related: [method-call, local-fn, export, native-target]
---

# Predicate function names with `?`

## Summary

関数系の名前に、識別子の一部として末尾の `?` を一つ許すことを提案する。`even` と `even?` は別の名前であり、`?` は演算子でもエラー伝播の指示でもない。Bool を返す関数を示す命名規約として利用する。

**これはレビュー中のDraft Proposalであり、採択された仕様ではない。** 作者は関数名への限定、識別子としての解釈、既存の関数値参照と呼び出しの区別の維持を決定した。ゼロ引数呼び出しも `()` を必要とする。変数・引数・フィールドへの拡張と束縛済みメソッド値は今回の対象外である。本Proposalが採択された場合、標準ライブラリのBool判定関数も `?` を用いた一貫した命名に移行することは作者の確定方針である。ただし個別APIの改名・別名追加・命名規約の具体化・移行方法は別Proposalで決定し、今回は既存APIの名前も一覧も変更しない。`has_key?`、`has_children?`などの`has_*?`は自然な名前として許容し、接頭辞を機械的に削除しない。Bool制約と警告AおよびB1/B2の採否は未決である。作者は警告導入に前向きだが承認済みとは扱わない。以下の構文詳細は検討案であり、Proposal全体の採択やDecisionの記入を意味しない。

## Motivation

現在も `is_even`、`is_active`、`contains` といった通常の名前で述語関数を定義できる。既存の `[api.list]`、`[api.option]`、`[api.string]`、`[api.float]` には次の API がある。

| API | 現在の署名 | 実装区分（仕様の API 表） |
|---|---|---|
| `List.is_empty` | `is_empty[A](xs: List[A]) -> Bool` | Emela |
| `Option.is_some` | `is_some[A](opt: Option[A]) -> Bool` | Emela |
| `Option.is_none` | `is_none[A](opt: Option[A]) -> Bool` | Emela |
| `String.contains` | `contains(s: String, sub: String) -> Bool` | intrinsic |
| `Float.is_nan` | `is_nan(x: Float) -> Bool` | Emela |

これらの署名・区分は `crates/emela-core/lib/core/{list,option,string,float}.emel` の宣言でも確認した。実際の実行結果は検証していない。

現行仕様で書ける例（CLI による整形・実行は未検証）:

```emel
fn is_even(n: Int) -> Bool {
  n % 2 == 0
}

fn main() {
  values = [2, 4]
  dbg(values.is_empty())
  dbg(List.filter(values, is_even))
}
```

主案で書く例（未実装の提案構文、整形・実行は未検証）:

```emel
fn even?(n: Int) -> Bool {
  n % 2 == 0
}

fn main() {
  predicate = even?
  dbg(predicate(4))
  dbg(List.filter([2, 3, 4], even?))
}
```

利点は、返り値を調べずに名前の末尾から述語である意図を読み取れることと、`is_` を自然に付けにくい `contains` などにも同じ目印を付けられることである。`is_even` や既存の API でも同じ計算はできるため、表現能力の不足を解消する提案ではない。規約だけなら期待が破られる可能性は残るが、警告は不一致の発見に役立つ。将来Bool制約を採れば、静的型検査で名前と正常終了時の結果型の整合性を保証できる。これは読みやすさに加えた実質的な利点である。

短い名前にするだけなら現行の命名規約を改善する方が安い。新しい字句クラス、エディタ対応、将来の構文上の制約に見合う読みやすさがあるかが採否の中心となる。

### 標準ライブラリ統一との関係

現行の `[lib.api-rules]` は `is_*` をBoolを返す判定関数とする。一方 `[api.*]` には `contains`、`starts_with`、`any`、`all` など接頭辞のないBool関数もあり、末尾`?`でこれらを含めた一貫した目印を提供できる。構文の導入後に標準ライブラリも統一する方針は確定している。現在の規約を今回すぐ置き換えるのではなく、採択後の後続Proposalで `[lib.api-rules]` を改訂し、既存API一覧と整合させる。

| 現在（API表で確認） | 将来の命名候補（未決定） |
|---|---|
| `List.is_empty` | `List.empty?` |
| `Option.is_some` | `Option.some?` |
| `Option.is_none` | `Option.none?` |
| `String.contains` | `String.contains?` |
| `String.starts_with` | `String.starts_with?` |
| `List.any` | `List.any?` |
| `List.all` | `List.all?` |
| `Float.is_nan` | `Float.nan?` |

右列は候補であり採用名ではない。`is_empty?` と `empty?` の選択は別の判断であり、`is_*?`は冗長になり得るが禁止は未決である。`has_*?`は所有・包含の意味を保つ自然な名前として許容する。`has_key?`を`key?`へ機械的に変換しない。すべてのBool関数を一貫した命名に統一することを目指すが、具体的な名前・対象範囲・例外は後続Proposalで定める。

後続ProposalはBool判定関数の棚卸し、命名規則、`[lib.api-rules]`改訂、個別APIの改名または別名追加、非推奨化・移行期間・適用時期・互換性、API命名Lintとの整合を扱う。コンパイラ内蔵API、intrinsic認識、ドキュメント、テスト、補完候補も同期させる。分割するのは方法と実施時期を決めるためであり、統一自体を任意にするためではない。今回は後続Proposalを作成せず、未作成slugを`requires`にも追加しない。

## Proposal

### Syntax

#### 主案の許可範囲

`lower_name` は変更しない。新しい `predicate_name` トークンと、文法上の `function_name = lower_name | predicate_name` を導入する。`predicate_name` の基部は有効な `lower_name` であり、末尾の `?` と間に空白・コメント・改行を置けない。単独の `_` と予約語を基部にできない案とする。この細部も Q4 の検討対象である。

| 位置 | 主案 | 理由・参照 |
|---|---|---|
| トップレベル fn、外部 fn、export fn | 許可 | 名前と host 側の文字列名を区別する |
| ローカル fn | 許可 | `[fn.local]` の可視性・非汎化を維持 |
| Trait の関数、既定実装、impl の関数 | 許可 | 宣言と実装を `?` 込みの名前で照合 |
| effect operation、handler の operation 実装 | 許可 | 宣言と実装を `?` 込みの名前で照合 |
| handler の `init` / `release` 構文 | 拡張しない | `fn init?` は operation の実装であり初期化ではない |
| 変数束縛、引数名、フィールド名、名前付き引数ラベル | 禁止 | 引き続き `lower_name` のみ |
| パターンの束縛、フィールドパターンのラベル、リスト末尾の束縛 | 禁止 | `[grammar.pattern]` を変更しない |
| 型、モジュール、const、型パラメータ、属性名 | 禁止 | 既存の字句クラスを維持 |
| import の列挙、裸の関数参照、修飾参照、呼び出しのメンバー名 | 許可 | 宣言した名前を参照できるようにする |

`users.empty?()` と `user.active?()` は構文の例である。実際に呼べるためには、受信値の型に対して `[type.rules]` の探索で該当関数が見つかる必要がある。`Option.is_some?(value)` も同様であり、この提案がその API を追加することは意味しない。

#### 字句解析・構文解析と型検査

`foo?` を最長一致で一つの `predicate_name` として読む。単独の `?` トークンや、`??`、`?.` 演算子は導入しない。

| 入力 | 主案での字句・構文 | 名前解決・型検査 |
|---|---|---|
| `foo?` | `predicate_name`。式として有効 | 名前が見つかれば関数値。自動呼び出しはしない |
| `foo?()` | `predicate_name` `(` `)`。通常の呼び出し | 存在・引数数・型を通常どおり検査 |
| `foo??` | `foo?` の後の `?` が不正文字 | コンパイルエラー。二重接尾辞にはしない |
| `foo ? ()` | `foo` の後の `?` が不正文字 | `foo?()` と同一視しない |
| `foo? + 1` | 関数名参照、`+`、整数。式の構文として有効 | 関数値を数値に加算するので型エラー。`foo?` は呼び出されない |
| `foo?.bar` | `foo?` `.` `bar`。postfix の構文として有効 | `foo?` の関数値に対する通常のメンバー検査。optional chaining ではない |

`foo?bar` は `foo?` と `bar` という隣接した名前トークンになり、そのままでは式として不正である。`Foo?`、`FOO?`、`_?`、`?foo` は predicate name ではない。`foo_?`、`_foo?` は有効な基部を持つので許す主案である。`if?` や将来用予約語 `try?` は許さず、`fails?`、`uses?`、`init?`、`release?` は完全一致の contextual keyword ではない通常の関数名として許す。

改行の継続規則は変えない。`foo?` の次の行の `()` は呼び出しにならない。`foo` と `?` の間にコメントを置いても名前は結合しない。

### Meaning

`?` を含む綴り全体が名前の同一性に含まれる。`foo`、`foo?`、`foo_` は別の名前であり、探索時に接尾辞の追加・除去や候補へのフォールバックをしない。二つの同じ `foo?` は既存の重複定義規則に従う。

`f?`、`Module.f?`、`Trait.f?` は通常の名前付き関数と同じ条件で関数値として参照でき、別名の変数 `predicate = f?` に束縛できる。`predicate? = f?` のような変数名は主案では許さない。関数型には名前の接尾辞を記録せず、引数名に関する既存の規則も変えない。

**既存設計との具体的な衝突:** `[type.rules]` は関数を見つけた `x.f` を束縛済みメソッド値にしない。従って `x.f?()` は許せても `x.f?` は同じ制約で拒否する。必要なら `Module.f?` または `|| x.f?()` を渡す。一方、`[effect.use]` の `cap.op` は既に関数値なので `cap.op?` も許す。これは現行の二つの規則を接尾辞付きの名前にも適用するものであり、すべてのドット参照を同じ関数値に変える提案ではない。束縛済みメソッド値は作者の決定により今回の対象外であり、必要なら別Proposalとする。

`?` は成功・失敗、例外、早期終了、suspend、真偽値への変換に意味を加えない。通常の呼び出しと同じ評価順序、E、R を使う。`[error.not]` の「`?` operator はない」とは衝突しない。`fails` は関数型のエラー集合、`escape` はエラーを扱う式のままであり、名前の接尾辞でエラーを取り除いたり変換したりしない。

### Typing rules

Q2は **`?`付き関数の戻り値** だけを扱う。`?`のないBool関数への警告はQ3で扱い、ここには含めない。Bool制約は未決であり、以下を対等な設計選択肢として比較する。作者は警告に前向きだが、警告導入・型エラー化・将来の移行時期はいずれも未承認である。

| 選択肢 | 利点 | コスト・必要な判断 |
|---|---|---|
| 命名規約のみ | 推論・汎化を変更せず導入できる | 名前からBoolを保証できない |
| 非Boolに警告 | 不一致を発見しつつコードを受理できる | 対象、未確定型、抑制、診断コードの設計が必要 |
| 非Boolを型エラー | 静的に名前と結果型の整合性を保証し、呼び出し側の期待を支える | ジェネリック・Never・推論の規則と互換性判断が必要 |
| 当初は警告、将来型制約 | 利用者に準備期間を与えながら保証へ進める | 後のリリースで受理集合が狭まる。移行は別の明示的決定が必要 |

`fn identity?[A](x: A) -> A { x }` の受理・診断は選択肢に依存する。定義の推論終了後に検査する案と、名前からBoolの期待型を先に与える案は同じではない。後者は名前が推論を変えるため、単なる命名規約と混同しない。呼び出しの一部で `A = Bool` になったことだけでジェネリックな定義全体をBool述語とは扱わない。

既存仕様ではTraitの戻り値省略は `()` である。`fn ready?(self)` を暗黙にBoolへ変更しない。型制約を選ぶならこの省略を診断するかも決める。impl / handlerの型は宣言から決まり、effect operationの戻り値明記・`uses`禁止は維持する。

Boolは正常終了時のTであり、`fails Missing`、`uses { Probe }`、suspendの有無とは独立する。Boolを要求しても純粋性や成功を保証するわけではない。

### Predicate naming lint

以下は未採択の警告案であり、Aの採否はQ2だけで決める。Q3は **正常終了時にBoolを返す関数に`?`がない場合** だけを扱い、警告なし・B1・B2から選ぶ。Q2とQ3は独立である。Bは接頭辞限定のB1とBool関数全般のB2を比較し、どちらも未採択とする。標準ライブラリの将来的統一は確定しているが、それだけで利用者コードへの警告を有効化するとは決めない。

- **A: 非Bool警告（Q2の警告選択肢）。** `?`付きの関数がBool以外を返す場合に名前と結果型の不一致を報告する。`fn ready?() -> String { "ready" }` や `has_key?() -> Int` が候補である。接頭辞は禁止理由にしない。
- **B1: 接頭辞限定の接尾辞不足警告（Q3）。** 名前が `is_` または `has_` で始まり、かつ正常終了時の戻り値がBoolと判明しており、`?`がない場合に報告する。`is_ready` → `is_ready?`、`has_children` → `has_children?` を提案する。字句上の接頭辞だけでは警告しない。`have_`・`does_`への拡張は今回の案に含めない。

**B2: Bool関数全般の接尾辞不足警告（Q3）。** 型推論後の正常終了時の戻り値がBoolと判明した関数全般で、末尾`?`がなければ報告する案。B1と異なり `contains(s: String, sub: String) -> Bool` も `contains?` の候補とする。標準ライブラリの統一方針と整合するが、全利用者関数に要求するか、演算子に対応するTrait関数や特殊な関数に例外を設けるかは未決である。B1を選ぶ場合とB2を選ぶ場合を混同せず、同じ宣言に重複警告を出す案にはしない。

`is_ready?` は `ready?` と書けて冗長になり得るが、冗長性の警告は別論点として当面提案しない。`has_key?`、`has_children?` は所有・包含という関係を明示するため、同じ基準で冗長としない。`has_key?` → `key?` の機械的な改名は推奨しない。

| 名前・戻り値 | AとB1またはB2を採用した場合の警告候補 |
|---|---|
| `ready?() -> Bool` | なし |
| `ready?() -> String` | A: 非Bool警告 |
| `is_ready() -> Bool` | B1/B2: 接尾辞不足警告 |
| `has_key() -> Bool` | B1/B2: 接尾辞不足警告 |
| `is_ready?() -> Bool` | 当面なし。冗長性は別論点 |
| `has_key?() -> Bool` | なし |
| `has_children?() -> Bool` | なし |
| `has_key?() -> Int` | A: 非Bool警告 |
| `contains() -> Bool` | B1ではなし、B2では接尾辞不足警告 |

この表は警告規則を採用した場合の期待挙動であり、確定仕様ではない。表中のB1/B2は採用した方だけを適用する。

#### B1/B2の範囲と標準ライブラリ移行の同期

B2を採れば、静的なBool判定はB1と同じく推論・型解決後に行い、名前の接頭辞条件だけを外す案になる。型エラーとしてBoolを強制する案とは独立である。演算子と関連する `Eq.eq` などのTrait関数、エントリやホスト規約に従う特殊な関数も一律に改名すべきかは未決であり、例外候補を棚卸しする。一般のTrait宣言・既定実装・impl、effect operation・handler、外部fn、ローカルfnも対象候補だが、宣言で一度だけ報告する案や外部名とソース名の区別を維持する。B2の採用や例外をここで確定しない。

標準ライブラリの統一と利用者全般へのLintは別の決定である。B1は対象が狭く段階導入しやすい一方、`contains`や`any`を拾わない。B2は統一方針と広く整合する一方、特殊関数・既存コードへの影響が増える。後続のAPI移行前に同梱ライブラリへB2を全面適用して大量の警告を出さないよう、core/stdを直接開発するときの適用と、依存として読むときの抑制を区別し、移行完了と警告有効化のリリースを同期させる案を検討する。Lintそのものを別Proposalに分離するか、移行まで有効化を待つかも未決である。既存の抑制機構があるとは仮定しない。

#### 警告の実装候補と未決事項

推奨する検討案は、名前から推論を誘導せず、各定義の推論・型の解決が完了した後に、明示型と推論型を同じ基準で調べることである。

| 対象・型 | 検討用の扱い（未承認） |
|---|---|
| 解決済みBool / 非Bool | Aと選択したB1/B2を独立に適用。`Option[Bool]`やBoolを返す関数型自体はBoolではない |
| Never | 正常な戻り値がないのでA/B1/B2を抑える案。型エラー案でも例外とするか判断が必要 |
| 未確定型・型エラー由来のunknown | 二次診断を避けて保留する案。未確定型を非Boolと断定しない |
| 汎化された戻り値型パラメータ | Boolを保証しないためA候補。ただし専用説明にするか、保留するかは未決。BはBoolと判明したときだけ |
| Trait宣言・既定実装・impl | 宣言の署名で一度だけ報告し、同じ名前・署名を実装するimplで反復しない案 |
| effect operation / handler | operation宣言で一度だけ報告し、handlerごとに反復しない案 |
| 外部fn / ローカルfn | 外部fnは明示署名、ローカルfnは本体の推論後に判定する案 |
| 警告の位置 | 宣言名の完全なトークン範囲（`?`を含む）。Bの変更候補でも元の宣言名を指す |
| bundled core/std・依存 | 利用者プロジェクトの定義だけに報告し、依存・同梱ライブラリを読み込んだだけでは報告しない案 |
| 無効化・抑制 | 必要性・コマンド設定・粒度を未決とする。仕様にない `@allow` やLint抑制属性は実装前提にしない |

警告を選ぶ場合は型情報に依存するAと選択したB1/B2それぞれに新しいW03xxコードを割り当てる候補とする。番号は未割当であり、警告を選ばないうちに予約しない。型エラー化する場合は別途E03xxの設計が必要である。

## Specification changes

The following is proposed replacement/additional text for the syntax design within the author-confirmed scope; grammatical details remain unapproved. Return-type enforcement and naming lints are unresolved and are not decided by this text. It is ready to paste section by section if that option and its details are adopted; it MUST NOT be applied before the author resolves the Open questions and adopts the proposal. No concrete standard-library API rename, API-list change, or revision of the current library naming conventions is included in these specification edits. If this proposal is adopted, the author intends to unify standard-library predicate names using `?`; a separate proposal will specify the names, exceptions, library conventions and migration. This separation does not make unification optional. Diagnostic numbers for a new syntax diagnostic remain to be assigned.

### `[lex.name]`

Add a name class:

| Class | Form | Used for | Examples |
|---|---|---|---|
| Predicate name | A lowercase name immediately followed by one `?` | Functions, trait functions, effect operations, and their implementations | `even?` `contains?` |

The base of a predicate name MUST be a valid lowercase name other than `_`; it MUST NOT be a reserved word, including a word reserved for the future. A contextual keyword followed by `?` is a predicate name, not a keyword. A predicate name is one token. Whitespace, comments, and line breaks cannot occur between its base and `?`. The snake_case convention applies to its base. The full spelling, including `?`, is the name.

Predicate names cannot name variables, parameters, fields, pattern bindings, types, modules, consts, type parameters, or attributes. On the left of `.`, a predicate name denotes a function value, not a module. Its members are checked by the ordinary value-access rules.

### `[grammar.lex]`

Keep `lower_name` unchanged and add the following lexical production:

```ebnf
predicate_name = lower_name "?"  (* one token; base is not "_" or a reserved word *)
```

Here `lower_name` describes the characters of the base, not a separate token. Longest match selects a predicate name over its base. No standalone `?` token exists. A second `?`, or a `?` not forming a predicate name, is an unrecognized character (E0105). A reserved word or `_` immediately followed by `?` does not form a predicate name: the base retains its existing token kind and `?` is E0105.

### `[grammar.decl]`

Add the syntactic production `function_name` and replace these productions; all others remain unchanged:

```ebnf
function_name = lower_name | predicate_name
import_item   = function_name | type_name | upper_name
qualified     = type_name { "." type_name } "." function_name
annot_arg     = lower_name | predicate_name | literal | lower_name ":" literal | qualified

fn_decl       = [ "suspend" ] "fn" function_name [ type_params ]
                "(" [ params ] ")" [ signature ] [ block ]
op_sig        = [ "suspend" ] "fn" function_name [ type_params ] "(" [ params ] ")"
                "->" type [ "fails" error_set ]
handler_item  = "init" [ "(" [ params ] ")" ] block
              | "release" block
              | [ "suspend" ] "fn" function_name "(" [ lower_name { "," lower_name } ] ")" block
trait_item    = "fn" function_name "(" [ params ] ")" [ signature ] [ block ]
              | derive_rule
impl_item     = "fn" function_name "(" [ params ] ")" block
```

Parameters, fields, attribute names and named attribute argument labels continue to use `lower_name`. A predicate-name annot_arg, like a qualified annot_arg, is allowed only as the argument of `@entry`. These extensions permit a predicate-named entry adapter to be referenced; they do not change the requirements on entry adapters. Other attribute forms, including the language argument of `@external` and `@export`, are unchanged.

### `[grammar.expr]`

Replace `local_fn`, `postfix`, and `primary`:

```ebnf
local_fn = "fn" function_name "(" [ params ] ")" [ signature ] block
postfix  = primary { "(" [ args ] ")" | "." function_name }
primary  = literal | lower_name | predicate_name | upper_name | type_ref | "self"
         | "(" ")"
         | "(" expr ")"
         | "(" expr "," expr { "," expr } ")"
         | "[" [ expr { "," expr } [ "," ".." expr ] ] "]"
         | block | if_expr | with_expr
```

A statement starting with `fn` followed by a `function_name` is a local_fn. No production for patterns, binding labels, or named argument labels is extended to accept predicate names. Whether a dot expression denotes a field, a capability operation, a static function reference, or a method call is determined by the existing name resolution and typing rules; accepting a member name in this grammar does not create a field with that name. The line continuation rules are unchanged.

### `[module.import]`

A function with a predicate name can be listed in braces in an import, as in `import Numbers.{even?}`. The imported name includes `?`. Importing `even?` does not import `even`.

### `[module.resolution]`

Predicate names use the existing namespaces, visibility and shadowing rules. Names are compared by their full spelling: `f`, `f?`, and `f_` are distinct. Resolution MUST NOT remove or add `?` when looking up a name. Duplicate checks for functions, trait functions, and effect operations include `?`. A predicate name in a value reference denotes a function or an effect operation, not a new variable binding.

### `[fn.decl]`

Drafting note (not normative): the following text specifies syntax and the naming intent only. Before adoption, it must be completed with the author's choice of return-type enforcement and warnings from Typing rules / Predicate naming lint. It does not select the convention-only option.

A function name MAY be a predicate name. A predicate name SHOULD be used for a function whose normal return type is Bool. The suffix adds no evaluation step, error propagation, effect, or suspension behavior. References to such functions can be used as function values under the same rules as references to other named functions. Function types do not carry the spelling of the name.

### `[fn.local]`

A local fn MAY have a predicate name. Its scope, capture, recursion, signature inference, and lack of generalization follow the rules for other local fns. The suffix does not permit type parameters, attributes, `pub`, or `suspend` on a local fn, and does not make a local fn a method-call candidate.

### `[trait.decl]`

A trait function, including a function with a default implementation, MAY have a predicate name. The return type is governed by the existing signature rules; omitting it still means `()`, including for a predicate-named function. The suffix does not constrain Self or select an implementation.

### `[trait.impl]`

A function in an impl MAY have a predicate name. It implements only the trait function with the same full spelling. Implementing `f` does not implement `f?`, and conversely. Parameter and return types continue to come from the trait declaration.

### `[trait.call]`

Trait function references and calls accept predicate names, including `Trait.f?`, `Trait.f?(value)`, and, for a function taking `self`, `value.f?()` under the ordinary method lookup rules. Bounds and implementation selection also apply when the trait function is used as a value.

### `[effect.decl]`

An effect operation MAY have a predicate name. Its return type is still required, its `fails` clause keeps its existing meaning, and a `uses` clause remains forbidden. The suffix does not determine whether the operation suspends.

### `[effect.handler]`

An operation implementation MAY have a predicate name. The full spelling MUST match the effect's operation declaration. Implementing `f` does not implement `f?`, and conversely. The implementation's types and suspension requirements are determined by that declaration as before. `fn init?` and `fn release?` name operations; they do not denote the handler's `init` or `release` item.

### `[effect.use]`

An operation with a predicate name is called through a capability value, as in `cap.ready?()`. Referencing it without a call, as in `cap.ready?`, makes a function value under the existing capability-operation rules. Its effect is retained in the function type. The suffix does not permit calls through an effect name.

### `[type.rules]`

Predicate-named function references, calls and method calls use the same typing rules, evaluation order, error sets and effect sets as other function references and calls.  Method lookup uses the full spelling, including `?`. A bare method reference `x.f?` does not make a bound function value; when `f?` is found as a module or trait function rather than a field, it is E0312 with help to call it as `x.f?(...)`. This restriction does not apply to capability operation references, which follow `[effect.use]`.

### `[ext.decl]`

An external fn MAY have a predicate name in Emela. Its `@external` strings specify host names independently of that Emela name. The suffix does not modify those strings or relax any target's host-name or boundary-type requirements.

### `[ext.export]`

A predicate-named top-level fn MAY have `@export` under the existing restrictions. The host sees the explicitly specified export name, not an implicit transformation of the Emela name. A source name containing `?` does not require a host export name containing `?`, and does not relax the existing requirements on an export.

### `[ext.entry]`

Replace the rule describing the argument of `@entry` with:

`@entry(T)` is written on a top-level fn `f` of the entry module. Writing it outside the entry module is E0437. Its one argument is a `function_name` (a fn of the same module) or a `qualified` path that names an entry adapter; anything else is E0439. `f` MUST be accepted as the first argument of `T` (E0440). Predicate names are accepted in both forms. The suffix does not create a different entry protocol or implicit host export name.

### `[fmt.space]`

The formatter preserves a predicate name, including its `?`, as one name token. It inserts no space inside it. Calls and dot references use the ordinary rules, producing `f?(x)` and `x.f?()`. It MUST NOT turn `f ?` into `f?`; input containing that lexical error is not formatted.

### `[lsp.completion]`

Completion labels and inserted names include the complete predicate name. `f` and `f?` are distinct candidates. Their completion kinds and signature help follow the declaration kind, not the suffix.

### `[lsp.navigation]`

Definition, references, and document highlights treat a predicate name including its suffix as one occurrence of its symbol. A position on `?` selects that same symbol. References to `f` and `f?` are not combined.

### `[lsp.rename]`

For functions, local fns, trait functions and their implementations, and effect operations and their implementations, a valid lowercase name and a valid predicate name are compatible rename classes. This is an exception to the requirement that the new name have the same lexical class as the old name. The new name MUST still be one complete valid token. Variables, parameters and fields cannot be renamed to a predicate name. Rename edits include the suffix and update imports and matched trait or effect implementations together with the declaration. Existing non-renamable symbols and the lack of conflict checking are unchanged.

### `[diag.list]`

Add the following syntax diagnostic (code to be assigned in the E01xx syntax band):

| Code | Condition | Example message |
|---|---|---|
| Code to be assigned | A predicate name is used in a position requiring an ordinary lowercase name, including a variable binding, parameter, field, pattern binding, or named argument label | `predicate names can only name functions and operations` |

The primary range is the complete predicate-name token. Parsing continues after that token where possible. Report this diagnostic instead of a redundant expected-lowercase-name or invalid-pattern diagnostic for the same token. Other independent errors may still be reported.

E0105 continues to apply to a `?` that is not part of a valid predicate name, including the second `?` of `f??`. Undefined-name, duplicate-name, missing-member, missing-implementation, and type mismatch diagnostics apply to predicate names under the existing rules, and display the full name including `?`. 

Drafting note (not normative): A belongs only to Q2 (the return type of a predicate-named function); B1/B2 belong only to Q3 (a missing suffix on a Bool-returning function). The following rows are independent alternatives to add only if the author selects the corresponding warning. The syntax text above does not enable any of these warnings or impose a Bool constraint.

| Code | Condition | Example message |
|---|---|---|
| W03xx, to be assigned if warning A is adopted | A predicate-named function has a resolved non-Bool return type, subject to the separately decided Never and generic-type policy | `predicate function ready? returns String, not Bool` |
| W03xx, to be assigned if warning B1 is adopted | A function has a resolved Bool return type and a name starting with `is_` or `has_` without a trailing `?` | `consider naming this function is_ready?` |
| W03xx, to be assigned if warning B2 is adopted instead of B1 | A function has a resolved Bool return type without a trailing `?`, subject to the separately decided exceptions and scope | `consider naming this function contains?` |

If adopted, the selected warning's primary range is the full declaration-name token. The final normative text must also specify the unresolved scope, inference timing, deduplication and suppression policy before adoption. No implicit migration from a warning to a compile error is specified here. B1 and B2 are competing scope options for the suffix warning, not two warnings to emit together. Their inclusion in this proposal or a separate lint proposal remains unresolved.

## Diagnostics

警告AはQ2、接尾辞不足のB1/B2はQ3に属する。Q3で警告なしを選んでもQ2の警告Aは独立に採用可能であり、その逆も同様である。

- [ ] E0105 を孤立した `?`、重複接尾辞の後半、無効な基部の後に再利用する。`?` の位置を指す。`foo?` 内の正しい接尾辞は診断しない。
- [ ] 名前トークン自体は正しいが宣言・束縛位置が不正な場合には、上記の新しい構文診断を提案する。番号は未割当。型・効果のエラーにはしない。
- [ ] E0212（未定義）、E0213（重複）、E0325（Trait 実装不足）、E0404（operation 実装不足）、E0405（handler 内重複）、E0401（未定義 operation）などは `?` 込みの綴りを使う。`foo` と `foo?` の共存は重複ではない。
- [ ] `x.f?` の束縛済みメソッド参照は既存 E0312 と呼び出しへの help。`foo? + 1` は既存の型診断を使い、不正な接尾辞とは診断しない。`foo?.bar` も字句エラーにはせず、通常の受信値・メンバー診断とする。
- [ ] Aと選択したB1/B2の警告は独立にW03xx新設の候補とする。採否・番号・発生位置・重複抑止は未決。型エラー案には別のE03xxが必要であり、未採択の警告を既存診断として扱わない。

## Compatibility

### 1. 構文の導入

現在有効な Emela プログラムには、文字列・コメントの外で predicate name の接尾辞になる `?` がないため、主案は既存の有効なプログラムの意味を変えない。文字列・コメント内の `?` は従来どおり内容であり、識別子として読まない。現在不正なコードの診断・回復結果は変わる。

### 2. 警告・型制約の導入

Q2の非Bool述語への対応（警告A/型制約等）と、Q3の接尾辞不足への対応（なし/B1/B2）は独立に評価する。Aから型エラーへの移行と、B1からB2への対象拡大を同じ移行として扱わない。

将来のBool型制約への移行の互換性リスクは**中程度**と評価する。警告の追加だけなら通常は受理集合を変えないが、利用者に診断対応の負担が生じる。型エラー化すれば、以前受理された非Boolの`?`付き関数が拒否される。Emelaは0.20系以降も言語設計が進行しており、破壊的変更を検討できる開発段階である。リスクだけで保証の利点を否定せず、警告期間・変更の告知・適用リリースを作者が別途決める。警告の導入だけで将来の型エラー化まで承認したとは扱わない。

### 3. 後続Proposalでの標準ライブラリ移行

今回の構文Proposalでは既存APIの名前と一覧を変更せず、`values.is_empty()`などの既存コードを維持する。採択後は標準ライブラリも統一する確定方針に従い、後続Proposalで具体的な移行を定める。改名すると、既存の呼び出し、メソッド形式の呼び出し、import、関数値参照などに破壊的変更が生じ得る。intrinsic認識・組込みAPI・文書・テスト・補完も同期が必要である。開発段階を踏まえて破壊的変更を検討できるため、リスクだけで統一を否定せず、別名・移行期間・非推奨化を使うか、いつ改名するかを後続Proposalで判断する。

| API 移行案 | 利点 | 欠点 |
|---|---|---|
| 改名 | API の一貫した移行 | 呼び出し・import・関数値参照・文書が壊れる。intrinsic とツールの対応も必要 |
| 別名追加 | 旧ソースを維持し新表記を選べる | 補完候補・API 表・保守対象が増え、どちらを使うか残る |
| 構文導入時は現行APIを維持し、後続で移行（今回の分割方針） | 構文と具体的なAPI移行を分離 | 移行まで新表記の利点は得られず、後続作業が必要。永久据え置きの案ではない |

API 変更は、`is_some?` と `some?` のどちらか、`is_empty?` と `empty?` のどちらかなど、それ自体に命名判断がある。具体的なAPI名と移行方法は構文拡張と別Proposalで決定することが作者の方針である。従って `[api.*]`、`[lib.api-rules]` は今回の `touches` に含めない。

### 将来の後置 `?` との関係

| 表記 | 今回の読み | 将来の競合 |
|---|---|---|
| `foo?` | 単一の述語名。関数値であり呼び出しではない | 裸の`foo`への後置演算子と競合 |
| `foo?()` | 述語名の呼び出し | `foo`への後置演算後の呼び出しとは読まない |
| `foo()?` | 現在は孤立した`?`でエラー | 呼び出し結果への演算子として字句上の余地はある |
| `(foo)?` | 現在は孤立した`?`でエラー | 括弧付き式への演算子として字句上の余地はある |
| `foo?.bar` | `foo?`、`.`、`bar` | Optional Chainingの`?.`と競合 |
| `foo??bar` | `foo?`、不正な`?`、`bar` | Null Coalescingの`??`と競合。将来トークン化を変えるだけでは既存の名前境界を保てない |

Emelaでは通常の呼び出しからEが伝播し、`escape`で処理できるため、各呼び出しに伝播用の記号を付ける必要性は低い。ただしOptionの扱いや別の将来用途もあり、後置演算子が絶対に不要とは結論しない。Q5として構文設計上のトレードオフを維持する。

最長一致で `foo?` を名前にすると、将来 `foo` の値に後置 `?` を適用する構文を追加しても `foo?` をその演算に読み替えられない。呼び出し結果への `foo()?` や括弧付き `(foo)?` は字句上の余地があるが、この Proposal はそれらを許可も予約もせず、エラー伝播の意味も定めない。将来演算子を導入するなら、裸の変数と呼び出し結果で表記が異なる制約、`foo?()` との関係、空白依存や括弧の必要性を解決する別 Proposal が必要となる。

`foo?.bar` は既に `foo?` のメンバーアクセスとして構文上読まれるため、将来 optional chaining として無条件に読み替える設計も制約する。`foo??` を現在エラーにしても、既にある `foo?` とのトークン境界問題はなくならない。これらの構文予算を使う価値があるかを Q5 に残す。

## Implementation

以下は 2026-11-15 のローカルソースを静的に調査した変更候補であり、実装済みという報告ではない。

| 層・根拠となるファイル | 必要となる検討 |
|---|---|
| `crates/emela-syntax/src/lexer.rs`, `lexer/name.rs`, `syntax_kind.rs` | Logos の単語切り出しと分類に専用トークンを追加。予約語基部、`_`、最長一致、文字列補間、エラー回復を検証 |
| `crates/emela-syntax/src/parser/`, `ast.rs`, `ast/items.rs`, `validation.rs` | AST の名前アクセサに `LOWER_NAME` 固定の箇所がある。fn と値参照の名前ヘルパーを分け、param/field/pattern を広げない。ローカル fn の先読みも対応 |
| `crates/emela-resolve/src/lower.rs`, `resolve.rs`, `hir.rs` | HIR は `SmolStr` の完全な綴りと定義IDを保持する。接尾辞を削除せず、宣言・import・参照・operation/impl の対応を同じ名前経路に通す |
| `crates/emela-types/src/methods.rs`, `body/methods.rs`, `traits.rs`, `signature.rs` | メソッド候補と Trait/handler の照合を監査。Bool制約は未決。警告なら推論後の結果型を取得する経路を検討する |
| Linter / `crates/emela-resolve/src/naming.rs` | 仕様は snake_case の Lint に言及するが、このファイルはファイル名・モジュール名の検証であり、述語戻り値の Lint 機構ではない。調査した frontend には戻り値命名Lintの既存基盤を確認できていない。警告案を安価な既存拡張と断定しない |
| `crates/emela-fmt/src/printer/`, `crates/emela-driver/src/fmt.rs` | CST トークンを一つの名前として出力。再解析による同値性確認、idempotence、コメント・改行の維持 |
| `crates/emela-driver/src/nav.rs`, `complete.rs`, `tokens.rs`, `crates/emela-lsp/src/server/navigation.rs` | 補完・シンボル探索のトークン処理を監査し、参照・Rename範囲は `?` まで含める。Rename は現在 `lex_one(new_name) == old_kind` であり専用トークン導入だけでは `foo` ↔ `foo?` が拒否される。シンボル種別で互換な名前クラスを判断する |
| `crates/emela-core/src/ir.rs`, `lower.rs`, `crates/emela-lir/` | Core は関数・ローカルをIDで参照し、表示名と意味上の同一性を分離する。閉包・Trait dispatch・handler lowering・単相化にも完全な綴りを渡し、表示名から同一性を作り直さない |

`[lsp.semantic-tokens]` は現在capability変数だけを返し、legendも `variable` / `capability` のみである。predicate関数に `function` / `method` semantic tokenを新設する提案ではなく、この仕様は変更しない。エディタ側の正規表現による色付けは別に監査が必要である。補完の現在の内部 `Completion` はlabel/insert textを持ち、置換範囲を持たない。既に末尾`?`がある位置での補完・接尾辞重複はエディタとの統合テストで確認し、必要なら同じ補完機構に置換編集を追加する。ソースから接尾辞を削除して回避しない。

### 型情報を使う警告経路（未採択案）

Q2で選ぶ警告Aは`?`付き宣言の結果型を調べ、Q3で選ぶB1/B2はBool型の宣言の名前を調べる。共通の推論後データを利用できても、判定と採否は別に持つ。

2026-11-16に再確認した `crates/emela-types/src/signature.rs` は `Signature` / `Signatures` に関数・effect operationの型を保持し、戻り値等を省略したfnは本体推論後に追加する。`check.rs` の `ProgramTypes`、`body.rs` のローカルfnの検査、`ty.rs` の `Ty` / `Prim::Bool` を使えば、Lexerではなく型検査層で結果型を判定する構成を検討できる。汎化・型変数解決を終えたTを調べ、E/RからBoolを推定しない。診断は `emela-types` の診断表現からdriverのコード・表示・LSPへ渡す既存経路を使う候補とする。Trait/operationの宣言IDとimpl/handlerの対応から重複を抑え、ソースの所属から同梱core/std・依存と利用者定義を区別する必要がある。推論未完了の `Signatures` だけを一律走査すれば足りるとは断定しない。

`[diag.list]` のW03xxは型に関する警告帯である。参照した仕様に一般的な命名Lint抑制機構は確認できないため、抑制が必要なら仕様・CLI/LSPとの接続も設計対象になる。今回は追加実装しない。

### B2向けの特殊名・Bool関数の棚卸し

2026-11-16の仕様・ソースの静的調査。仕様の意味、名前に依存する処理、単なる呼び出し側の参照を区別した。分類は改名可否や例外の採択を意味しない。実行・全backendでの改名試験は行っていない。

| 現在の名前・戻り値 | 分類／特殊扱い | 仕様上の根拠 | 実装上の根拠 | B2適用時の懸念 |
|---|---|---|---|---|
| `Eq.eq(self, other) -> Bool` | 特殊なコンパイラ連携あり。`==`/`!=`に対応 | `[trait.builtin]`、`[trait.call]`、`[type.rules]` | `emela-resolve/src/builtins.rs`の`PRELUDE_TRAITS`、`emela-types/src/signature.rs::prelude_trait_fn_signature`の`"eq"`、`emela-core/src/lower.rs::eq_op_ty`の`prelude_trait_fn_id("Eq", "eq")` | 改名可能性はあるがPrelude・署名・演算子認識を一緒に変更する必要。特殊扱いだけでは例外としない |
| Eqのderiveで生成する`eq -> Bool` | 特殊なコンパイラ連携あり。生成名・dispatch | `[trait.derive]`、`[trait.builtin]` | `emela-types/src/traits.rs`の`DERIVABLE`、`emela-core/src/lower.rs`の`("Eq", "eq") => DeriveOp::Eq`と`eq: lowered(self, "Eq", "eq")` | deriveと手書きimplを揃える。生成コードに同じ命名警告を重複させない |
| `List`/`Option`、`Std.Array`/`Bytes`/`Map`/`Set`のimpl `eq -> Bool` | 特殊なコンパイラ連携あり。Trait契約に依存 | `[trait.impl]`、`[trait.builtin]`、各`[api.*]` | `emela-core/lib/core/{list,option}.emel`、`lib/std/{array,bytes,map,set}.emel`の`impl ...: Eq` | 個々のimplだけの改名は不可。Trait宣言と同時に移行 |
| `Ord.compare -> Ordering`、`Show.show -> String`、Immediate（関数なし） | 特殊なコンパイラ連携あり。ただしBool関数ではない | `[trait.builtin]`、`[trait.derive]` | `builtins.rs::PRELUDE_TRAITS`、`signature.rs`、`lower.rs`のcompare/show/derive認識 | 比較演算の最終結果がBoolでもcompare自身にB2を適用しない。組込みTraitのBool関数として確認したものはEq.eq |
| 利用者Traitの任意のBool関数 | 通常の関数（Traitの宣言・実装照合は必要）。固定名はない | `[trait.decl]`、`[trait.impl]` | `emela-resolve/src/hir.rs`の`TraitFn`/`ImplFn`、`emela-types/src/traits.rs` | 宣言・全impl・呼び出しを同期。任意Traitのderiveは現在不可。全利用者Trait一覧は未確認 |
| `String.contains`/`starts_with`/`ends_with -> Bool` | 特殊なコンパイラ連携あり。intrinsicの名前表 | `[api.string]`、`[lib.api-rules]` | `lib/core/string.emel`の`@intrinsic`、`emela-core/src/intrinsics.rs::Builtin::info`と`lookup_intrinsic(module,name)` | 宣言だけの改名では認識できない。Builtinへの対応表を更新。例外でなく対応して改名する案が可能 |
| `Std.Deferred.complete[A] -> Bool` | 特殊なコンパイラ連携あり。通常のBuiltin表とは別 | `[api.std.deferred]` | `lib/std/deferred.emel`の`@intrinsic`、`emela-core/src/sources.rs`の宣言一覧、`lower/fiber.rs`の`("Std.Deferred", "complete") => FiberFn::Complete` | 専用loweringも同期。副作用を持ち成功を返す関数を述語命名に含めるか判断が必要 |
| `List.is_empty`/`any`/`all`、`Option.is_some`/`is_none`、`Float.is_nan -> Bool` | 通常の関数。確認範囲では固定名の特別認識なし | `[api.list]`、`[api.option]`、`[api.float]` | `lib/core/{list,option,float}.emel`のEmela本体。intrinsic表はこれらの名前を登録しない | API呼び出し・import・関数値参照の移行が中心。全ソースの文字列依存が皆無とまでは保証しない |
| `Std.Bytes/Array/Map/Set.is_empty`、`Std.Map/Set.contains -> Bool` | 通常の関数（Eq実装とは分ける） | `[api.std.bytes]`、`[api.std.array]`、`[api.std.map]`、`[api.std.set]` | `lib/std/{bytes,array,map,set}.emel`の通常の本体 | 通常のAPI移行。`filter`等の引数関数がBoolを返すだけのケースは対象にしない |
| `Bool.parse -> Bool fails ParseError` | 通常の関数だが変換というAPI上の役割あり | `[api.bool]`、`[lib.api-rules]` | `lib/core/bool.emel`の通常の本体 | Boolを返すことと意味上の判定関数であることは同義でない。`parse`規約との整合から例外または統一名を検討する候補 |
| `FileSystem.exists`/`is_directory -> Bool` とhandler実装 | 外部仕様・プロトコルに依存。effectの署名一致も必要 | `[api.std.file-system]`、`[effect.decl]`、`[effect.handler]` | `lib/std/file_system.emel`のoperationとhost wrapper | operationと全handlerを同時更新。host名は別文字列として維持可能 |
| `Std.Env.env_has`、`FileSystem.host_exists`/`host_is_directory -> Bool` | 外部仕様・プロトコルに依存 **かつ特殊なコンパイラ連携あり** | `[api.std.env]`、`[ext.decl]`、`[target.wasm.external]` | `lib/std/{env,file_system}.emel`の`@external`、`emela-codegen-wasm/src/std_host.rs`の`Std.Env.env_has`等の名前表 | JS/Cの明示host名とソース名は別。WASMの標準host認識はソース名にも依存するため同時更新が必要 |
| `Http.host_received_text -> Bool uses {}` | 外部仕様・プロトコルに依存 | `[api.std.http]`、`[ext.decl]` | `lib/std/http.emel`の外部宣言 | hostとの境界は確認。全backendの個別認識経路は未確認であり、通常wrapperと断定しない |
| `main`（戻り値は利用者宣言による）、任意名の`@test`関数 | 外部仕様・プロトコルに依存（entry/runner規約） | `[module.name]`、`[cmd.build]`、`[test.attr]`、`[test.constraints]` | `emela-driver/src/frontend.rs`と`emela-core/src/lower.rs`の`name == "main"`、JS emitter/C/WASM各`lib.rs`のmain識別。テスト収集は`emela-driver/src/testing/collect.rs` | Boolのmainを`main?`にしてもentryは移行しない。役割に基づく例外候補。テストは固定名より属性で指定し、Boolの結果があるだけで失敗/成功判定を変えない。Boolの全実行挙動は未確認 |
| private helper: `eq_lists`、`eq_from`、`valid_utf8_from`、`continues`、`too_heavy`、`is_dir`、`has_file_ancestor`、`is_token`、`is_field_value`、`well_formed`、`decodes`、`unreserved -> Bool` | 通常の関数（調査した宣言と本体）。全ての名前依存は未確認 | `[type.inference]`、`[lib.api-rules]`。private一覧はAPI表の対象外 | `lib/core/list.emel`、`lib/std/{array,bytes,map,file_system,http}.emel`、`lib/std/http/request.emel` | public APIだけでなくprivate helperにもB2が及ぶ案なら一括棚卸しが必要 |

この一覧は `rg '-> Bool'` の候補を宣言の最終戻り値と照合したものを含む。`List.filter`や`find_index`は引数にBoolを返す関数を取るが、自身の結果はList/OptionでありB2対象ではない。戻り値省略の全関数・ローカルfn・汎化された全定義はソース検索だけでは網羅できず **未確認** とする。最終的な全数調査には型推論後の結果型が必要である。

backendの名前依存も二段階に分ける。文字列intrinsicはJSの`emit/builtins.rs`（例`StringContains => "$strhas"`）、Cの`emit.rs`（`em_string_contains`）、WASMの`emit/builtins.rs`がBuiltinで処理する。これらはソース名そのものではなくIRの分類に依存し、名前表で同じBuiltinへ到達できればruntime名まで改名する必要があるとはいえない。Eqの演算子・派生実装、DeferredのFiberOpもIRへ到達するまでの名前認識を監査する。WASMのstd host名表はこれとは別の直接依存である。既存のID・内部名生成の調査は後節に維持する。

#### B2と特殊名に対する三つの未決案

| 方針 | 一貫性・理解しやすさ | コンパイラ影響・互換性 |
|---|---|---|
| 原則適用 | 特殊関数もBoolなら`?`という単一原則。ただしparse等の意味を説明する必要 | Eq/derive・intrinsic・std host認識も対応して改名。変更範囲と利用者のimpl移行が増える |
| 明示的な例外 | entryや変換等の役割を説明して除外。一律ではないが理由を示せる | 名前を保つ合理性があるものだけ維持。単に実装コストがあるという理由では例外にしない |
| 段階的な適用 | 通常関数から開始し、特殊名は保留と明示。一時的な差を説明する必要 | 初期の破壊的変更を絞れるが、残件を後続Proposalで決定し移行を継続する |

例外を選ぶ場合は`"eq"`などの文字列を全関数から除くのではなく、解決済みの組込みTrait関数ID・entryの役割・外部宣言種別などで区別できるか検討する。同名の利用者の通常関数まで除外しない。外部fnはhost名を維持してソース名を変えられるので一律免除は自明ではない。Eq.eqやintrinsicも対応を実装して改名できるなら例外は必須ではない。一方、`main`は現在のentry規約、`Bool.parse`は変換の意味と規約が例外候補の理由になる。採否・個別名・例外・移行方法は後続Proposalで決め、今回確定しない。

### コード生成と境界名

- **JS:** `crates/emela-codegen-js/src/emit.rs` の `sanitize` / `fn_name` は非英数字を `_` にし、`function_names` が `FnId` ごとの表を作り、重複を `$$2` などで解消する。従って `foo?` と `foo_` は同じ基部に変換されても既存の一意名生成で分離できる見込みがある。直接呼び出し・関数値・単相化したコピー・suspend のコピーがすべてこの表を使うかをテストする。局所的な `replace("?", ...)` は追加しない。
- **C / native:** `crates/emela-codegen-c/src/names.rs::function` は `e{index}_` に表示用の名前（最大48文字、非英数字を `_` に変換）を付ける。IDが一意性を担うので同じ表示基部になっても区別できる。`fn_names` を介して呼び出す既存経路を維持する。C は現在の `native` target の中間言語であり、host 言語の指定は `c` のままである。
- **WASM:** `crates/emela-codegen-wasm/src/emit/mod.rs` の `fn_index(FnId)`、関数インデックス・テーブル・静的閉包を使う。ソース名をC/JS識別子として出力する必要はない。デバッグ名・エラー表示とhost/component境界名を混同せず、直接/間接呼び出しを確認する。
- **外部関数:** `@external(c, "host_ready") fn ready?() -> Bool` のソース名とCリンク名は別である。`@external` の文字列を内部名のマングリングに通さない。JS は既存の import 名・プロパティアクセスの規則を保持する。
- **export:** JS の emitter は非識別子名に対する文字列 export と default object の文字列キーを既に扱う。Cの外部公開名は別経路であり、既存の C 識別子検証（`names::is_c_identifier` の呼び出し側）を維持する。`@export(c, "host_ready") pub fn ready?() -> Bool` は内部名の文字種と外部名を分離できる例である。`@export(c, "ready?")` を内部名として置換し救済してはならない。host名の診断の仕様化に不足があれば別の既存ギャップとして扱い、本案で新ABIを作らない。
- **WASM の境界:** `[target.wasm.external]` はユーザー定義の WASM import/export をまだ許さない。`?`付きの内部関数が生成できることとhost公開できることは別であり、後者の対応をこの提案に含めない（`[open.list]` #56）。

### 調査範囲・既存提案

`proposals/README.md`、`TEMPLATE.md`、`spec/README.md`、`glossary.md`、両スキルを参照し、`scripts/spec.py index` / `show` / `refs` / `proposals` で関連節を調べた。ローカルの `proposals/` と `[open.list]` に同じ機能を決めた記述は見つからなかった。

GitHub は読み取り専用のコネクターで `repo:emela-lang/emela` に対して proposal ラベルの全状態検索（上限100件）と、`predicate`、`"question mark"`、`"述語"` の PR/Issue 検索を実施した。これらの語で直接重複する提案は見つからなかったが、別表現の全履歴を網羅したという保証ではない。Git コマンド、Issue/PR 作成は行っていない。

関連する既存提案:

- [Method calls #454](https://github.com/emela-lang/emela/pull/454): ローカルの proposal は `implemented`。`x.f` の制約と探索規則を維持する。
- [Local functions #502](https://github.com/emela-lang/emela/pull/502): ローカルの proposal は `accepted`、仕様には反映されている。実装Issue [#571](https://github.com/emela-lang/emela/issues/571) が検索で見つかったが、ローカル fn の現在の実行可否は未検証であり、採択と実装を同一視しない。
- [Exported functions and entries #427](https://github.com/emela-lang/emela/pull/427): 明示的なhost名とソース名の区別を維持する。
- [The C target becomes the native target #564](https://github.com/emela-lang/emela/pull/564): ローカルの proposal は `accepted`。古い `[target.c.*]` ではなく現在の `[target.native.*]` を参照する。

### 検証結果と必要な適合テスト

PATH（`Get-Command` / `where.exe`）、既存の `target` 配下、通常のユーザー用bin配置を探索したが、実行可能な Emela CLI は見つからなかった。`crates/emela-cli/build.rs` は Git コマンドを呼ぶため、Git操作禁止を守る目的でCLIのビルドも行わなかった。現在のコンパイラの振る舞いを実行確認したという主張はしない。

このファイルの Emela コードブロックはすべて **CLIで未整形・未実行** である。`?`を含むものは未実装の提案構文であり、現行CLIで整形できるとは扱わない。CLIが利用可能になったら、リポジトリ外の一時ディレクトリに空の `Pome.toml` と `src/main.emel` を置き、依存を取得せず既存構文を `emela check` と `emela fmt` で確認する。ローカル fn は実装状態を別に確認する。提案構文の正常系は機能実装後の受け入れテストである。

| 分類 | 具体的なケース | 期待結果（主案を実装した場合） |
|---|---|---|
| 正常系 | `fn foo() -> Int { 1 }` と `fn foo?() -> Bool { True }` を同じモジュールに置く | 共存し、直接呼び出し・関数値経由でそれぞれの結果を得る |
| 正常系 | 自作モジュールに `pub fn ready?(x: Item) -> Bool`、`Module.ready?(x)`、`x.ready?()`、`import Module.{ready?}` | 同じ定義IDに解決。直接参照 `f = Module.ready?` を呼べる |
| 正常系 | `List.filter([2, 3, 4], even?)`、`f = even?`、`f(4)` | 名前の接尾辞が関数型や引数名の規則を変更しない |
| 正常系 | Trait の `fn active?(self) -> Bool`、impl、`Trait.active?(x)` と `x.active?()` | 同じ署名・実装に解決。既定実装とジェネリック境界でも同様 |
| 正常系 | effect の `fn ready?() -> Bool`、handler の `fn ready?() { True }`、`cap.ready?()`、`f = cap.ready?` | 宣言と実装が一致し、関数値も効果を保持する |
| 正常系 | ローカル `fn even?(n: Int) -> Bool` と自己再帰・キャプチャ | 既存のスコープ・再帰・非汎化規則のまま動作 |
| 型方針依存 | `identity?[A](x: A) -> A`、推論された非Bool戻り値、Trait の戻り値省略 | 構文は受理。診断は未決のBool方針による。Trait省略は `()` |
| 正常系 | Bool戻り値に `fails Missing`、`uses { Probe }` を組み合わせる | E/Rが通常どおり伝播し、`escape` と handler が従来どおり処理 |
| 異常系 | `foo??`、`foo ? ()`、`?foo`、`Foo?`、`_?`、`if?` | 不正な `?` に E0105。接尾辞として救済しない |
| 異常系 | `x? = True`、`fn f(x?: Bool)`、`type Item(active?: Bool)`、`[x?] = xs`、`f(x?: True)` | predicate-name全体に新しい構文診断。通常の束縛・ラベルに広げない |
| 異常系 | `foo? + 1`、未定義 `missing?()`、二重 `fn foo?` | それぞれ型エラー、未定義、重複定義の既存診断 |
| 異常系 | Trait/effectで `f?` を要求し `f` だけ実装 | 実装不足を報告。`?`を取り除いて一致させない |
| 異常系 | `x.active?`、`Effect.ready?()` | 前者は束縛済みメソッド値を作らない。後者はeffect名からのoperation呼び出しを許さない |
| 境界 | `foo?` EOF、`foo?()`、`foo?bar`、`foo?.bar`、`foo?()`の結果に`.bar` | 最長一致、隣接トークン、関数値と呼び出し結果の区別を確認 |
| 境界 | `foo # comment` の次行に `?()`、`foo?` の次行に `()`、`foo?` の次行に `.bar` | 名前は連結しない。呼び出し改行とドット継続は既存規則どおり |
| 境界 | `foo_?`、`_foo?`、`fails?`、`uses?`、operation `init?` / `release?` | 有効な基部を許し、contextual keywordや特別なhandler itemに誤分類しない |
| 境界 | `foo`、`foo?`、`foo_` の直接・間接呼び出し、ジェネリック/suspendコピー | JS/C/WASMで別関数を呼ぶ。JS名衝突解消とCのID一意性を確認 |
| 境界 | `@external(c, "host_ready") fn ready?`、明示名のJS/C export、WASM user export | host名は変換しない。ターゲットの既存制約を維持 |
| 境界 | 有効なadapter `serve?` に対する `@entry(serve?)` と `@entry(Module.serve?)` | 同じadapterに解決し、既存の署名・境界検査とexport名を維持 |
| 境界 | completion、カーソルを`?`上に置くdefinition/references、`foo`↔`foo?` Rename | 全範囲が一つのシンボル。変数の`?`付きRenameは拒否。Trait/handler実装とimportも追従 |
| 回帰 | 既存API5種、関数値フィールド、メソッド候補衝突、通常の引数・パターン | 既存の有効なソース・候補選択・診断を維持 |
| 回帰 | 文字列/コメント内の`?`、`!=`、既存の `fails`/`escape`、fmtの再実行 | 内容と既存構文を維持。fmtは再解析でき、2回目で変化しない |

警告の追加テスト案（AとB1またはB2を採用した場合の条件付き期待。未採択）:

| 分類 | ケース | 期待候補 |
|---|---|---|
| 正常系 | `ready? -> Bool`、`has_key? -> Bool`、`has_children? -> Bool`、`is_ready? -> Bool` | A/Bなし。接頭辞削除を要求しない |
| 異常系 | `ready? -> String`、`has_key? -> Int` | Aだけを宣言名全体に報告 |
| 異常系 | `is_ready -> Bool`、`has_children -> Bool` | Bだけを報告し`?`追加を提案 |
| 境界 | `is_ready -> String`、`contains -> Bool` | 前者はA/B1/B2なし。後者はB1ではなし、B2なら接尾辞不足警告 |
| 型推論 | 明示`-> Bool`と本体からBool推論、明示/推論String | 同一の判定結果 |
| 型推論 | Never、unknown、未確定型、`identity?[A] -> A`、`Option[Bool]` | 未決の例外方針を個別に固定してからテスト。unknownで二次診断を増やさない案 |
| Trait | 非Boolの述語宣言・既定実装と複数impl、戻り値省略`()` | 宣言に一度だけの案。implごとに重複しない |
| Effect | `ready? -> String`と複数handler、`has_data -> Bool` | 宣言でA/B、handlerには重複なしの案。`fails`/`uses`から判定しない |
| 関数種別 | 外部fnの明示型、ローカルfnの推論型 | 型情報取得後に同じ判定基準を適用する案 |
| 回帰 | bundled core/std・依存を読むプロジェクト | 依存の既存`is_*`等から大量の警告を出さない案。利用者自身の定義は検査 |
| 将来移行 | 警告段階の非Bool述語を型エラー段階で再検査 | 将来の移行を採用した場合は拒否。警告と受理集合の変更を区別 |
| B1/B2比較 | `contains -> Bool`、`any -> Bool`、推論されたBool関数 | B1では警告なし、B2なら接尾辞不足。明示型と推論型を同じ基準で判定 |
| B2の範囲 | Trait/Effect/外部fn/ローカルfnのBool関数、演算子関連のTrait関数 | 採用後に対象・例外を固定してテスト。impl/handlerで同一宣言の警告を反復しない |
| 移行の回帰 | 現行APIを読む利用者とcore/std自体の開発 | 依存読込みと直接開発を区別し、API移行と有効化時期の方針に合わせる |

実装時には Lexer のトークン範囲、Parser/CST、解決ID、型/E/R、Formatter出力、LSP編集範囲、backend実行結果をそれぞれ対応する既存テスト群で検査する。今回これらの実装テストは追加・実行しない。2026-11-16の修正でもコード例のCLI整形・実行は行っていない。

### 追加の受け入れ条件とB2適合テスト

以下を実装・後続の命名Proposalのレビュー条件とする。未採択の例外を先に実装するための指示ではない。

- Q2は`?`付き関数の戻り値だけ、Q3はBool関数の`?`不足だけを扱い、警告Aの採否はQ2にのみ置く。B1/B2はQ3で代替として選び重複報告しない。
- 現行仕様と実装から特殊なBool関数を棚卸しし、Eq.eq・derive・intrinsic・組込みAPI・entry・外部名と全backendの依存を確認する。名前で認識する処理と単なるAPI参照、IRのBuiltin/ID参照を区別する。
- 推論後の型による全数調査で戻り値省略・private helper・ローカルfnも確認し、未確認項目を完了扱いしない。判明した追加項目も同じ分類表へ追記する。
- 例外を採る場合は維持すべき規約等の理由を示し、処理系を対応させて改名する案と比較する。特殊扱いだけを免除理由にしない。
- 標準ライブラリ統一を維持し、個別名・例外・移行は後続Proposalへ残す。Q2/Q3やB2の適用方針を採択済みと扱わない。

| 分類 | ケース | 条件付きの期待／確認事項 |
|---|---|---|
| Q2/Q3分離 | `ready? -> String`、`ready -> Bool` | 前者はQ2/Aだけ、後者はQ3/B2だけ。Q2規約のみとQ3/B2も併用可能 |
| 正常系 | `ready? -> Bool`、`has_key? -> Bool` | B1/B2の接尾辞不足なし。has_削除を求めない |
| 警告系 | `contains -> Bool`、`is_ready -> Bool` | B2なら両者に一度ずつ。B1なら後者のみ。Q3警告なしなら双方なし |
| 例外 | 組込みEq.eqと、利用者の通常の`eq -> Bool` | Eqを例外に選んだ場合のみ組込みの役割を除外。利用者の同名関数を文字列一致で免除しない |
| 例外候補 | Boolを返すmain、Bool.parse、明示host名を持つ外部fn | 原則適用/明示例外/段階的適用の決定ごとに期待を固定。例外未決の段階で免除テストを確定しない |
| derive回帰 | `derive Eq`と手書きimpl、`==`/`!=`、List/OptionのEq | 宣言と生成名・演算子解決が一致。派生実装へ重複警告なし。改名案なら全経路を同期 |
| intrinsic回帰 | contains/starts_with/ends_with、Deferred.complete | 新ソース名でも同じBuiltin/FiberOpへ到達し、JS/C/WASMで同じ結果。改名しない場合も既存経路を維持 |
| 外部境界回帰 | env_has、host_exists、host_is_directoryのソース名とhost名 | 明示host名とWASM std host表を区別し、対応変更後も同じ外部関数へ到達 |
| Trait/Effect/型推論 | 推論Bool、明示Bool、Trait/operation宣言と複数実装 | 同一の型基準、宣言単位の警告。Ord.compareの結果がOrderingであることも確認 |
| entry/test回帰 | mainの探索、任意名の@test、外部export | entry・test収集・host公開名を壊さず、改名によるテストフィルタの変化も検査 |

### 下書き作成後の批判的レビュー

2026-11-16の再レビューでは、確定した対象範囲をOpen questionsから切り分け、Bool制約なし・警告なしを暗黙に選ぶ旧記述を修正した。A/B1/B2の表・診断候補・テストはすべて条件付きであり、作者の警告への前向きな意向を採用決定に置き換えていない。今回の具体的な標準API変更と `has_` の機械的削除は含めていない。将来の標準ライブラリ統一自体は確定方針として反映し、B1/B2の選択は未決のままとした。

1. **Motivationは好みの範囲を出ない。** 計算能力は増えず、Bool制約を選べば名前と結果型の静的保証も得られる。既存の `is_` / `has_` に具体的な保守上の不利益がなければ、現状維持にはコスト面の利点があった。ただし現在の作者方針は導入後の標準ライブラリ統一であり、その実施方法を後続で具体化する。
2. **限定することで別の非対称性が生まれる。** 関数値を受け取る引数には `?` を付けられない一方で、非Boolを返す名前付きfnの扱いは未決である。前者は確定した範囲の帰結、後者はQ2の判断として区別する。
3. **一貫性を広げすぎない。** 既存の裸のメソッド値の禁止、effect operation参照の許可、Traitの戻り値省略が `()` になる規則を再確認した。接尾辞を理由にこれらを変更しない。
4. **ツールと将来構文のコストが主な負担。** 内部コード生成は既存のID・名前表を再利用できる見込みがあるが、CSTの名前アクセサ、Renameの字句クラス例外、補完中の不完全なソース、将来の後置`?`の制約は残る。新ABIは不要と見込む一方、Bool制約を選ぶ場合の型検査コストは独立に評価する。
5. **レビューで範囲外の拡張を除いた。** semantic tokensが現在はcapability変数だけを返す事実に合わせ、関数のsemantic token新設を含めないようにした。EBNFは宣言・import・式・`@entry`の裸/修飾参照を接続し、パターンや属性名を意図せず広げない形にした。

## Alternatives

### `lower_name` 全体に `?` を許す

文法差分は小さく、変数 `ready? = predicate`、引数 `f?`、フィールド `active?` などにも一貫した名前を使える。しかし識別子だけでは関数・Boolのどちらも保証されず、パターン、ラベル、属性の引数、handlerのフィールドまで受理範囲が広がる。フィールドとメソッド候補の衝突も拡大する。作者は今回この拡張を行わず、必要なら別Proposalで扱うと決定した。

### 宣言だけ許す、または呼び出しの直前だけ許す

`fn f?` を定義できても `f?` の値参照やimportを書けなければ通常の関数と扱いが分かれ、高階関数で不便になる。`f?()`だけ特別扱いして`?`を演算子にする案も、名前の同一性を保つ目的と異なる。主案は宣言と参照を接続する。

### 型エラーまたはLintでBoolを要求する

読み手への保証は強くなるが、命名が型推論に介入するか、警告だけで済ませるかは別の判断である。Typing rules の比較と Q2 を参照する。警告から将来の型制約へ進む段階的導入も候補であり、静的保証の利点と中程度の互換性リスクを比較する。

### 既存の `is_` / `has_` / 動詞による命名だけを使う

実装コストも将来の構文競合もなく、現在すでに十分使える、最も単純な代替案である。`contains`などにも一律の末尾目印を付ける利点と実装コストを比較するため、従来の代替案としてこの評価を残す。作者は導入後の標準ライブラリ統一を決めており、今回これを恒久据え置きの案として採用しない。

### 標準ライブラリを同時改名するか、後続で移行するか

同時改名は構文導入直後からAPIを揃えられるが、構文レビューと対象一覧・移行の判断が結合する。後続Proposalへ分離すれば、既存の `is_*` 規約をどう統合するか、改名と別名追加、非推奨化、移行時期・例外、Lintとの同期を個別に検討できる。作者は後続で具体化する方針を選んだ。統一しない案を選んだのではない。

### 属性、`predicate` 宣言、escaped identifier

属性は宣言箇所で意図を示せても呼び出しの綴りに目印を付けない。専用宣言やescaped identifierはより一般的な名前の仕組みを必要とし、この狭い用途には重い。将来一般化する明確な用途があるなら別途比較する。

### 後置演算子として `?` を予約する

将来の構文設計を優先して今回の機能を採らない案である。現時点の `[error.not]` は `?` 演算子を持たず、名前の拡張を排除する決定もない。演算子の具体的な必要性と今回の命名上の利点を作者が比較する（Q5）。

## Prior art

### RubyのAPIと外部Lintの区別

2026-11-16確認の [Ruby Hashリファレンス](https://docs.ruby-lang.org/ja/latest/class/Hash.html) は `has_key?` / `key?`（および`include?` / `member?`）を同じキー存在判定として、`has_value?` / `value?` を同じ値存在判定として掲載する。これらは同義のAPIであり、Ruby本体は`has_`を含む述語名を実際に提供している。

[RuboCop Naming/PredicatePrefix](https://docs.rubocop.org/rubocop/latest/cops_naming.html#namingpredicateprefix) はこれとは別の定義名の規則である。確認時の文書はRuboCop 1.92、Enabledであり、設定表の `NamePrefix` と `ForbiddenPrefixes` の既定値は `is_`、`has_`、`have_`、`does_`。禁止接頭辞は末尾`?`があっても報告対象となり、`AllowedMethods`（既定`is_a?`）で例外を認める。本文の短い既定値説明は3項目だが、ここでは4項目を列挙する設定表と区別して確認した。このcopによるユーザー定義の検査を、Ruby標準の `hash.has_key?(key)` の呼び出しが禁止されるという意味に読み替えない。

[RuboCop Naming/PredicateMethod](https://docs.rubocop.org/rubocop/latest/cops_naming.html#namingpredicatemethod) は戻り値推定と接尾辞を検査する別copである。確認時は **Pending**（追加1.76、変更1.78）であり、通常設定で必ず警告されるとはいえない。真偽値、リテラル、他の述語呼び出しなどから判定し、変数や非述語呼び出し等の未知の戻り値は判定しない。既定のconservativeは少なくとも一つBoolの返り値があれば`?`名を許し、aggressiveは既知の非Boolの返り値があれば報告する。

Ruby本体のAPI、RuboCopの既定規則、Emelaの警告候補は別である。Emelaは解決した静的結果型を使い、`has_`の包含関係の意味を保ったまま接尾辞不足を指摘できる。動的な戻り値推定のモードや禁止接頭辞をそのまま輸入する必要はない。

### その他の関連概念

ここでは概念と差異を比較し、他言語の規則をEmelaの定義として流用しない。

- **述語を示す名前の接尾辞:** [Ruby公式FAQ](https://www.ruby-lang.org/en/documentation/faq/6/) は `?`で終わるpredicate methodに真偽値を返すことを推奨する。これはEmelaでBoolを型エラーとして強制すべき根拠にはならない。
- **述語の命名規約と別用途の接頭辞:** [Elixir 1.12.3 Naming Conventions](https://elixir.hexdocs.pm/1.12.3/naming-conventions.html#trailing-question-mark-foo) は末尾`?`をBoolを返す関数の規約として説明する。一方、guardで使う関数には`is_`という別の規約がある。この区別からも、Emelaの既存`is_` APIを機械的に改名する結論は導けない。
- **値に適用する伝播演算子:** [Rust Reference: try propagation expression](https://doc.rust-lang.org/reference/expressions/operator-expr.html#the-try-propagation-expression) の`?`は値に作用し、正常値の取り出しと早期伝播を行う演算子である。本提案の名前の一部とは異なる。Emelaは`fails`によるEと`escape`で失敗を表し、呼び出しのEは通常の型規則で伝播する。名前の`?`にこの実行時意味を加える理由はない。

## Open questions

- Q1 — 関数名への限定は決定済み。現行の`fn_decl`・`local_fn`・`trait_item`・`impl_item`・`op_sig`・handlerのfn itemを含む構文上の詳細案を確認するか。`init`/`release`自体と変数・引数・フィールドの拡張は対象外。
- Q2 — **`?`付き関数の戻り値**。非Boolを許可（規約のみ）・非Boolなら警告A・非Boolなら型エラー・当初は警告Aで将来型制約へ移行、のどれにするか。ジェネリックな戻り値、Never、Trait/Effectの宣言と実装、型推論・明示戻り値型、推論への介入、警告Aの診断・抑制、移行時期をどう扱うか。接尾辞のないBool関数はこの問いに含めない。
- Q3 — **Bool関数の命名**。正常終了時の戻り値がBoolで`?`がない場合に、警告なし・B1（`is_`/`has_`限定）・B2（必要な例外付きでBool関数全般）のどれにするか。B1/B2は代替であり同一宣言に重複報告しない。警告Aの採否はここでは決めない。Lintを今回に含めるか別Proposalに分けるか。推論後の判定、宣言で一度だけ報告、利用者プロジェクトだけを対象とする案、未確定型の保留、汎化型の扱いをどうするか。無効化・抑制の必要性と方式、B1/B2のW03xxコードの割当、演算子関連・特殊な関数の例外を役割で定めるか、原則適用・明示例外・段階的適用のどれにするか、標準ライブラリ移行と警告有効化の同期も未決。
- Q4 — 基部の細部（予約語基部と `_?` の禁止、`_foo?`・`foo_?`・contextual keyword基部の許可）、専用トークン、構文診断、関数系に限る相互Renameを主案どおりにするか。新しい構文診断の番号は何にするか。
- Q5 — `foo?`を単一識別子とすることで将来の後置`?`、Optional Chaining、Null Coalescingの設計を制約する問題をどう評価するか。`fails`/`escape`で必要性が低くなるとしても、将来の演算子が不要とは決めない。

決定済み: 関数名への限定、採択後の標準ライブラリの`?`による命名統一、具体的な改名・移行を別Proposalで決めること、`has_*?`の許容と接頭辞を機械的に削除しないこと。後続Proposalで決める個別名・例外・非推奨化・時期・移行方法は今回確定しない。変数等への拡張、束縛済みメソッド値、ゼロ引数の`()`省略は今回の対象外である。

## Decision
