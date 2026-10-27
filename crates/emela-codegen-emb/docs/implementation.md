# 組み込みC backendの実装範囲と対応状況

`emela-codegen-emb` はFlow IRから、固定容量のメモリと必要なruntimeを使うC11コードを生成する。この文書は現在のコンパイラーライブラリの契約・実装箇所・対応する試験をまとめる。表の試験名はリポジトリにある回帰試験への参照であり、今回の実行結果を表すものではない。

## 1. 実装境界と入力

中心となるAPIは `compile_flow(&emela_lir::Module, &Options)` である。入力は `Stage::Flow` の構造体であり、dump文字列を解析しない。型・制御・失敗辺を検証し、closure変換、値レイアウト、所有権、suspend継続、必要なruntime生成をこのcrateで行う。`emela_lir::to_heap` およびその後のHeap IRパスは呼ばない。

型やvariantの定義は [LIR](../../emela-lir/src/ir.rs)、[Coreのbuiltin](../../emela-core/src/intrinsics.rs) を参照する。標準moduleはCoreの同梱ソースへ登録するが、Core IR・syntax・LIRの定義やlowering処理は変更しない。通常のホスト向けC backendのABIも変更しない。

| API | 入力・役割 | 現在の境界 |
| --- | --- | --- |
| `compile_flow` | Flowを正規化・検証してCを生成 | 未解決の型・effect集合、不整合なABI、Heap IR専用要素は拒否する。外部C境界とISRには後述の制約がある。 |
| `verify_flow` | Flowの型・stage・制御辺を検証 | 共通LIR verifierに加え、Heap専用RuntimeOpと未lowerのsource builtinを拒否する。資源設定・C ABIの検査は出力段でも行う。 |
| `flow_from_core` | 既存のCore単相化と `emela_lir::from_core` を使う便宜API | Core入力・既存lowering経路の制約を受ける。正常Flowを受け取るAPIの範囲と同一ではない。 |
| `compile` | `flow_from_core` と `compile_flow` を順に呼ぶ | Core便宜APIと同じ制約を受ける。Emelaソースを解析するAPIではない。 |
| `derived_binding` | 閉じたFlow型の構造比較・構造表示のbindingを作る | `DerivedOperation::Eq` / `Show` を指定し、producerがFlowへ追加して呼ぶ。sourceのtrait実装を選択・書き換える機能ではない。 |

### Core便宜APIとsourceの制約

正常なFlow入力への対応から、任意のsourceが既存frontend経由でそのFlowになることは導けない。特に次の境界がある。

- **成長する再帰型**：`Grow[A]` のフィールドに `Grow[List[A]]` が現れるような型族を含むCore moduleは、未使用の定義でも `flow_from_core` / `compile` が `Unsupported` として拒否する。[`derived::check_core_input`](../src/derived.rs) と [`type_expansion`](../src/type_expansion.rs) による入口検査である。Flow producerが既存IRで有限の入力を作り、必要な構造操作を `derived_binding` で表せば、`compile_flow` の対応対象になる。sourceからそのFlowを自動生成する接続はこのPRに含まない。
- **直接の関数値const**：現在の本体 [`emela-core/src/lower.rs`](../../emela-core/src/lower.rs) の `signature` は、constが返すfn型を通常関数の宣言と同じように扱うケースがあり、`const APPLY = fn(x: Int) { x + 1 }` を正しいCoreへlowerできない。値として返す関数のeffectをconst initializerのevidence引数にしない処理も必要になる。sourceの回帰試験では関数値を `(関数, ())` のtupleへ格納して呼び出す。backendのconst cache・closure対応は有効なCore/Flow入力に対するものであり、このsource loweringの修正は含まない。
- **汎化effectのAsyncへの具体化**：現在の本体 [`emela-core/src/mono.rs`](../../emela-core/src/mono.rs) には、未確定effect集合をAsyncへ具体化した際にLambdaと間接Callの `suspends` の印を更新できないケースがある。例は `const APPLY = fn(callback) { callback() }` を `Fiber.join` するcallbackへ適用する場合である。Flow producerは確定した型・suspend/failure/evidence ABIを与える必要がある。このPRのsource汎化const試験は同期・failure・通常effectを扱い、このAsync具体化の成功を保証しない。

`derived_binding` は既存のexternal metadataを使うbackend専用契約である。型はclosedかつ有効で、Unit/Neverは引数表現がないため対象外。Eqは関数値を比較せず、Showは関数値を `<fn>` と表示する。sourceのユーザー定義Eq/Showを使う場合は、その呼出しをproducer側で残す。

### 状態の読み方

| 状態 | 意味 |
| --- | --- |
| 対応 | 正常なFlowに対する生成処理がある。共通の容量・実行文脈・外部ABI契約は適用される。任意のsourceや任意のメモリ量への対応を意味しない。 |
| 制約付き | 記載した入力・実行構成に限定する。条件外は診断する。 |
| Flow禁止 | Heap IR専用、またはFlowへ変換する前に置換される要素。Flow入口で `InvalidFlowIr` にする。source機能そのものが未実装という意味ではない。 |

## 2. 値表現・メモリ・所有権

| 基盤 | 実装される値・操作 | 資源と実行文脈 |
| --- | --- | --- |
| 静的なC値と定数 | scalar、固定レイアウトのtuple/enum、String literal/view、静的に解決できるclosure/handler | 使用する値・helperだけを出力する。大きなinline値や同期再帰もC stackを使用する。 |
| 固定heap | List、再帰型の箱、escapeしたString、動的closure/handler/evidence、Cell、Anyのbox | moduleごとに上限を確保し、システムの `malloc` を呼ばない。main側で参照管理・解放を行う。 |
| 機能別runtimeとpool | Fiber、suspend frame、task/event、Deferred、resume、失敗・取消の継続 | 到達可能な機能に応じて出力する。heapとは別の固定領域も持ち、各poolを独立に設定する。 |

- `Int` / `Bool` はCの32 bit整数、`Int64` は64 bit整数、`Float` はbinary64の `double`。Unit/Neverには通常のC値を割り当てない。
- nullary enumは整数、payloadを持つenumとtupleはinlineの構造体を基本とする。List等の参照フィールドがあるだけでは構造体全体をheapへ移さない。再帰する辺に箱を置き、成長する型族の辺では型付きの動的表現を使う。
- 所有参照の追加・解放はbackendが挿入する。借用したフィールドの読取り、分岐の結果、並列loop代入、capture、失敗値、suspend保持も対象になる。結果を使わない操作でも必要な副作用・容量検査・所有権解放を維持する。
- 最後の参照からの深い値の解放は、解放待ちリストを使い反復する。Cell・closure・Any等によるcycleを扱う経路では固定heap内のcycle回収も行う。回収やデータ量に比例する操作を含むため、固定容量であることから一定の実行時間は保証しない。
- `@external type` は不透明なhandleとして保持する。`Any` / `EmelaError` はタグ付きdescriptorを使い、一般値のboxとerror payloadを区別する。Cellは共有するtyped contentを所有し、creatorのstack frameから独立した寿命を持つ。source capability制約をheap化によって緩めない。

### 設定値

| `Options` の設定方法 | 既定値 | 意味 |
| --- | --- | --- |
| `with_heap_bytes` | 0 byte | heapを使用する入力には正の容量が必要。0ではheapが必要な経路を診断する。 |
| `with_peripheral_buffer_capacity` | 256 word | 標準UART/I²C/SPI bridgeの1方向の転送buffer。1〜65,535 wordで、1 wordはuint16_t。C stack上の配列を使う。 |
| `with_string_capacity` | 256 byte | 動的String生成の上限。設定範囲は1〜512 byte。 |
| `with_const_diagnostic_bytes` | 1024 byte | runtime constの保存するdefect message領域。必要な場合だけ出力し、heapからは確保しない。 |
| `with_fiber_capacity` | 8 | 実行中または完了後に未解放のFiber handle数。 |
| `with_frame_capacity` | 32 | 同時に生存するsuspend frame数。 |
| `with_frame_bytes` | 256 byte | headerを含む1 frameの上限。target依存の型付きframeサイズを生成Cのstatic assertで検査する。 |
| `with_task_capacity` | 8 | 外部task数。 |
| `with_event_capacity` | 8 | 完了event queueの長さ。 |
| `with_deferred_capacity` | 8 | 生存するDeferred数。 |

設定範囲外は `InvalidOptions`、実行中の容量不足はdefectとする。heap容量には管理情報・alignmentも含まれ、指定byte数すべてがpayloadに使えるわけではない。stack総量の証明やtargetのlink map評価はこのライブラリでは行わない。

### String・数値・Unicode

`EmelaString` はデータpointer・uint16のbyte長・backing ownerを持つdescriptorであり、各変数に256 byte配列を埋め込まない。NUL終端を必要とせず、埋込みNULを保持する。literalは読み取り専用領域を参照する。

`StringBufferPlan` は生成箇所ごとの容量と生存期間を計画し、連結の中間bufferを抑え、短命なscratchを再利用する。String戻り値には必要に応じcallerが渡す出力bufferを使う。長寿命の値・capture・suspend保持では、元のstackを参照し続けず所有backingまたはframeへ保持する。容量が足りなければ切り詰めずdefectにする。任意の制御フローで最小のscratch量になる保証はない。

Int / Int64の十進出力は最大11 / 20 byte、動的Float出力は最大25 byteをproducerの容量計画へ登録する。Floatの短い往復可能な表記、±0、非有限値、subnormal、固定/指数表記の切替を実装する。`-0.0000012345678901234567` は25 byteを使う。Float.parseはpointer+長さで文法を検査し、固定作業領域でbinary64へ丸める。不正形式はNone、overflow/underflowは符号付きInfinity/0をSomeで返す。動的変換はhost libcの文字列変換や動的確保に依存しない。targetのdoubleにはIEEE 754 binary64を要求する。

書記素処理は同梱するUnicode 18.0.0のtableとextended grapheme clusterの規則を使う。データ出典は [`unicode_grapheme.rs`](../src/unicode_grapheme.rs)、公式vectorは [`GraphemeBreakTest-18.0.0.txt`](../tests/data/GraphemeBreakTest-18.0.0.txt) にある。String長、chars、空separatorでのsplitはこの境界を使い、byte長・部分文字列検索・code point操作とは区別する。

### const

静的に評価できるscalar/String等は定数へ畳む。評価のfuel・深さ・出力サイズに上限を設け、畳めない場合はruntimeの初回参照時に評価してcacheする。再帰helper、外部呼出し、closureやaggregateを返すconstも、有効な入力IRと必要な資源があればこの経路で扱う。通常のdefectはcontext付きでcacheし、自己参照は専用の診断を行う。const初期化とその診断に必要な記憶領域も固定容量である。

汎化したconstは `const_origins` が既存Core単相化APIのoriginを追跡し、既存Flowのprivate metadataへcanonical値と型別specializationの対応を保存する。`const_bridge` が型別レイアウトとcallableの橋渡しを生成し、source上の初期化を型ごとに繰り返さない。origin metadataのない直接Flow入力は、異なるconst IDを独立したcacheとして扱う。source/Coreの関数値constとAsync具体化には第1節の制約がある。

## 3. 失敗・defect・suspendの契約

`fails` は回復可能な失敗辺として扱い、所有するerrorをcatch・上位関数・FiberのCauseへ渡す。通常のreturn/fail/break、取消、scopeの終了では必要なProtect cleanupを実行する。cleanup自身がsuspendする場合も、その完了を待って元の結果や失敗を配送する。永久に完了しないcleanupを時間だけで強制終了する契約はない。

**defectは即時停止契約である。** kind・messageのpointer・byte長を `_Noreturn emela_board_defect` へ渡す。Protect cleanupや参照解放を行わず、Fiber.attemptでもDieとして捕捉しない。通常のEmelaのdefect unwindと異なるtarget固有の意味論であり、hookが安全な出力状態への移行・停止/resetを担当する。公開入口まで届いた未処理のfailsは `EMELA_DEFECT_UNHANDLED_ERROR` と静的message `unhandled error` にする。

suspendは固定pool上の継続と協調schedulerで実装する。Scope/Fork/Join/Await/Interrupt、Deferred、Cause、suspend closure、handlerのresume、managed値の保持・解放を扱う。schedulerはmain文脈で呼ぶ。`emela_scheduler_run(budget)` はdispatchまたはeventの処理数を制限するが、同期の同一fiber内resumeや次のsuspendまでの処理時間を分割する上限ではない。WCET・任意の応答時間・全stack使用量の保証は提供しない。

## 4. 外部C・ISR・ボードとの境界

基本の生成ファイルは `emela_embedded.h`、`emela_board.h`、`emela_embedded.c` の3つ。標準handlerを使う場合は、到達する機能に応じてGPIO/UART/I²C/SPI/Clock/ConsoleのC headerと共通 `emela_peripheral.h` も出力する。必要なhookとtask ABIだけをheaderに宣言する。C側の宣言は生成headerを正とし、ホスト向けC backendの型定義やABIを流用しない。

| 境界 | 契約 |
| --- | --- |
| scalarの同期external | 通常の `@external(c, ...)` bindingを利用する。非scalar、fails付きの同期externalには明示的な `with_external_adapter` を使う。 |
| `ExternalResult::Borrowed` | adapterの戻り値をEmela側でownしてから返る。C側は取得完了まで値を生存させる。C calleeのstackへのpointerは返さない。 |
| `ExternalResult::Owned` | 全managed fieldを含む所有値をEmelaへ移す。ownerは当該moduleの固定heapに属し、mallocやC stack由来ではない。 |
| `ExternalResult::StringBuffer` | callerから追加で渡すbufferとbyte容量へ書き、ownerなしStringを返す。String戻り値専用。書込み前に容量を検査する。 |
| 同期adapterの引数 | 呼出し中だけの借用。保持するなら独立した所有参照を取得する。ownerなしStringのbyte列はcopyする。 |
| suspend external | `with_task_adapter` でC symbolを明示mappingする。開始時に `EmelaTask` を渡し、完了を通知してmain側で継続を再開する。開始引数を後で使うならC側でsnapshot/retainする。 |
| managed task結果 | 型付きproducer/context leaseを使う。producerとrelease callbackはmain側・event lock外で呼び、ISRでは呼ばない。scheduler・Emela入口へ再入しない。 |
| C callback | Emela関数値を通常のC関数pointerへ自動変換するthunkは提供しない。runtime capabilityを含む外部署名にも制約があり、条件外は診断する。 |
| moduleの実行文脈 | scheduler・heap・生成Emela入口はmain側で扱う。複数thread/coreからの同時呼出し・再入を自動で同期するAPIではない。 |

| 生成hook/API | C側の責任 |
| --- | --- |
| `emela_board_defect` | 使用するプログラムで要求される場合に実装する。戻らず、messageは長さ付きの借用として扱う。ISRから到達する場合も安全に停止する。 |
| `emela_board_debug` | DebugPrintが使う長さ付きbyte列を呼出し中に読み切るかcopyする。行末の出力を担当する。 |
| `emela_board_init` / `emela_board_idle` | main起動wrapperを生成する場合の初期化・idle処理を担当する。 |
| `emela_board_event_lock` / `emela_board_event_unlock` | task/event経路で必要な割込み状態の保存・復元と排他を実装する。 |
| `emela_task_complete_*` | 固定event queueへ通知するAPI。ISRから呼べるが、その場でEmelaを再開しない。stale/重複通知は拒否し、queue不足はdefectにする。 |

ISRとして公開するEmela関数は `with_isr_entry` で指定する。到達先にheap値・参照管理・再帰・suspend・runtime const初期化・動的call・Hold/Unholdがある場合は拒否する。外部C symbolは `with_isr_safe_external` で明示し、非scalar adapter等の追加制約も満たす必要がある。この検査は外部関数の実装や実行時間を証明しない。ISRのstack総量は算出しない。

標準 `Std.Embedded` effect・共有handlerとC bridgeは[共通インターフェース](embedded-interfaces.md)と[ABI契約](embedded-peripherals.md)で提供する。これらの標準bindingでは汎用adapterの明示mappingは不要である。[ボードpackage](boards.md)はSTM32F4とESP32-C3のstartup/runtime、standalone build/flash script、Rustのboard probeとFlash起動の観測runnerを提供する。`Output.exports` から公開C symbolとsuspend入口かどうかを取得できる。CLIのtarget/board選択、TOML設定、ツールチェーン取得や利用例の統合は後続の範囲である。BSPはGPIO・Console・Clock等の対応部分を実装し、未提供のdriverは利用側が補う。

## 5. APIの最小利用例とhostでの確認

既にFlow producerを持つ場合は `compile_flow` へ渡す。最小のFlowを直接作る例を示す。

```rust
use emela_codegen_emb::{Options, compile_flow};
use emela_core::TypeId;
use emela_lir::{Const, FnBuilder, Module, Operand, Term};

let mut flow = Module::new(emela_core::Module::new());
let mut function = FnBuilder::new(&flow.core, "answer");
function.set_ret(TypeId::INT).set_exported();
function.term(Term::Return(Some(Operand::Const(Const::Int(42)))));
flow.add(function.finish());
let output = compile_flow(&flow, &Options::default()).expect("compile Flow IR");
let directory = std::path::Path::new("generated");
std::fs::create_dir_all(directory).expect("create output directory");
for file in output.files {
    std::fs::write(directory.join(file.path), file.contents).expect("write generated C");
}
```

利用側のCプログラムは生成headerをincludeして公開入口を呼び、要求されたhookを実装して生成Cとlinkする。入口名・引数・String出力buffer・所有権操作は生成headerに従う。heapを使うプログラムでは、例えば `Options::default().with_heap_bytes(4096)` とする。これは4096 byteで任意のプログラムが動くという推奨値ではない。

workspace rootで実行する確認コマンドは次のとおり。host C実行試験にはC11に対応する `cc` と数学ライブラリが必要で、cross compilerやemulatorは使わない。workspace全体の確認には既存の他backendが要求するNode/wasmtime等も必要になる。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p emela-codegen-emb --locked
cargo run -p emela-codegen-emb --example check_flow_coverage --locked
cargo nextest run --workspace --locked --profile ci
```

host試験は生成Cの値・制御・所有権・容量defectを確認する。特定MCU上の割込みtiming、link配置、最大stack/heap、電気的な周辺機器動作の検証にはならない。以下の対応表はvariantの実装と試験の結び付きを示し、全入力・全型・全制御配置の組合せを網羅する宣言ではない。

## 6. Flow IR対応表

実装名は特記がなければ [`emit.rs`](../src/emit.rs) 内の関数を指す。所有権・資源・ABIの共通制約は第2〜4節のとおり。source builtinは既存lower先と、Flow入力としての可否を分けて記す。variantが増えたときはこの文書・実装・試験を合わせて更新する。

<!-- flow-audit:Value Const Prim Call Builtin Make Field Tag Tuple TupleGet List ListIsEmpty ListHead ListTail FnRef Closure MakeClosure EnvGet EvLookup EvExtend Handler Fiber IsError AsError Runtime Coerce -->
<!-- flow-audit:Const Int Int64 Float Bool String Unit -->
<!-- flow-audit:PrimOp Add Sub Mul Div Rem Neg Not Eq Ne Lt Le Gt Ge -->
<!-- flow-audit:PrimTy I32 I64 F64 String -->
<!-- flow-audit:Callee Direct Indirect Op -->
<!-- flow-audit:Operand Var Const -->
<!-- flow-audit:Stmt Let If Switch Loop Block Protect Dup Drop -->
<!-- flow-audit:Term Yield Return Fail Break Recur Defect Unwind Unreachable -->
<!-- flow-audit:FiberOp Scope Fork Deferred Join Await Interrupt Attempt Complete DeferredAwait Poll CauseKind CauseToString CauseRaise -->
<!-- flow-audit:OnFail Break Propagate Handle -->
<!-- flow-audit:SwitchOn Tag Lit -->
<!-- flow-audit:CaseKey Variant Lit -->
<!-- flow-audit:Stage Flow ClosuresConverted DefectsExplicit StateMachines RefCounted -->
<!-- flow-audit:Builtin StringLength StringByteSize StringAppend StringContains StringStartsWith StringEndsWith StringSplit StringChars StringJoin StringTrim StringCodePoints StringFromCodePoint IntToString IntParse IntCheckedAdd IntBitAnd IntBitOr IntBitXor IntBitNot IntShiftLeft IntShiftRight IntShiftRightUnsigned Int64FromInt Int64ToString Int64ToInt Int64BitAnd Int64BitOr Int64BitXor Int64BitNot Int64ShiftLeft Int64ShiftRight Int64ShiftRightUnsigned FloatToString FloatFromInt FloatParse FloatFloor FloatCeil FloatRound FloatTruncate Panic Todo Dbg Inspect ShowNested CellNew CellGet CellSet ShowString DebugPrint AssertFail -->
<!-- flow-audit:RuntimeOp ShowString DebugPrint Retire Defect DefectAt DefectTake DefectResume Hold Unhold Yield YieldPerform -->

### Value

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `Value::Const` | 定数値を読む | 対応 | `emit.rs::constant_expression`。6種をC literal/viewへ出す。Unitは変数を持たない。 | `emits_c_for_a_function_returning_an_int_constant`、`compiles_core_bool_and_unit_constants`、`emits_and_runs_int64_constants_at_the_signed_boundaries`、`emits_and_runs_float_constants_including_non_finite_values`、`emits_utf8_and_embedded_nul_string_constants_as_views` | Stringは読み取り専用view。UnitにはC変数を作らない。 |
| `Value::Prim` | 型別の基本演算を行う | 対応 | `primitive_expression`と型別helper。整数wrapと除算defect、IEEE Float、String内容比較。 | `emits_and_runs_int32_primitive_operations`、`emits_and_runs_int64_primitive_operations_with_wrapping_semantics`、`emits_and_runs_float_primitive_operations`、`string_comparison::source_comparisons_are_heapless_and_allow_isr_and_constant_space_tail_loops` | 有効な型と演算の組合せは次表。 |
| `Value::Call` | 関数・effect操作を呼ぶ | 対応 | `emit_statement_inner`のDirect、`emit_indirect_call`、`emit_handler_operation_call`、`suspend.rs`。結果の所有権取得と捨てた結果のreleaseを行う。 | `compiles_direct_calls_to_internal_and_external_c_functions`、`propagates_catches_and_reports_fallible_calls`、`passes_any_values_through_function_value_parameters_and_results`、`suspend::suspend_closure_keeps_captures_after_the_original_function_value_is_released` | 外部Cは第4節の明示契約に従う。 |
| `Value::Builtin` | 組込み演算を行う | 対応 | Builtinの各生成処理。 | `int_bitwise_builtins_match_signed_32_bit_semantics`、`cell::discarded_cell_construction_and_reads_release_their_owned_contents` | 下のBuiltin表でFlowへの出現可否を区別する。 |
| `Value::Make` | enum値を構築する | 対応 | `emit_make`、`emit_aggregate_field_store`、`TypeLayouts::collect`。inline enumと再帰する辺の箱、参照保持。 | `constructs_payload_and_unit_enum_variants_as_inline_values`、`boxes_recursive_enum_fields_and_releases_deep_values_iteratively`、`boxes_recursive_fields_nested_inside_tuples` | Core便宜APIの成長型制約は第1節。 |
| `Value::Field` | enumのフィールドを読む | 対応 | `emit_field`。recursive boxを参照解除し、managed結果を保持する。 | `reads_generic_enum_fields_from_inline_values`、`stores_and_releases_list_references_inside_enum_fields`、`owned::recursive_heap_payload_promotion_and_destruction_do_not_follow_value_depth_on_stack` | managed結果には独立した所有参照を持たせる。 |
| `Value::Tag` | enumの構成子番号を読む | 対応 | `emit_tag`。nullary enumは整数、payload enumはtag field。 | `reads_tags_from_payload_and_nullary_enums`、`emits_and_runs_switches_on_nullary_enum_representations` | 確保自体を行う操作ではない。 |
| `Value::Tuple` | tupleを構築する | 対応 | `emit_tuple`。Unit位置を保ったinline structと、recursive edgeの箱。 | `constructs_inline_tuples_and_keeps_unit_field_positions`、`stores_and_releases_list_references_inside_tuples_and_list_elements`、`owned::heap_payload_promotion_owns_inline_strings_and_shares_existing_backing` | Unit位置とmanaged fieldの所有を保持する。 |
| `Value::TupleGet` | tupleの要素を読む | 対応 | `emit_tuple_get`。inline fieldまたはrecursive boxから取得しmanaged結果を保持。 | `reads_tuple_fields_and_ignores_unit_projections`、`stores_and_releases_list_references_inside_tuples_and_list_elements` | Unit射影には通常のC値を作らない。 |
| `Value::List` | Listを構築する | 対応 | `emit_list_value`とtyped node。tail・headを保持し反復destroyで解放。 | `allocates_and_releases_lists_in_the_fixed_heap`、`reads_list_heads_and_keeps_shared_tails_alive`、`stores_and_releases_nested_lists`、`owned::returned_list_owns_formatted_strings_after_the_producer_stack_returns` | 空以外の実行時nodeには固定heapを使う。 |
| `Value::ListIsEmpty` | Listの空判定を行う | 対応 | NULL比較。 | `emits_empty_list_and_checks_list_emptiness`、`discarded_list_emptiness_is_heapless` | NULLとの比較。 |
| `Value::ListHead` | Listの先頭値を読む | 対応 | `emit_list_head`。非空条件は入力の制御フローの前提。 | `reads_list_heads_and_keeps_shared_tails_alive` | 非空であることを入力の制御フローが保証する。 |
| `Value::ListTail` | Listの残りを読む | 対応 | `emit_list_tail`。使う結果は独立retain、捨てる結果は参照追加なし。 | `reads_list_heads_and_keeps_shared_tails_alive`、`discarded_list_tail_preserves_the_source_owner` | 元Listと結果の寿命を分ける。 |
| `Value::FnRef` | 関数を値として参照する | 対応 | `validate_function_refs`、`function_ref_adapter`、lifted closureの環境構築。 | `adapts_a_function_value_to_effect_evidence_and_failure_call_abis`、`suspend::suspend_fnrefs_dispatch_to_external_tasks_and_internal_frames` | C callbackの自動thunkは含まない。 |
| `Value::Closure` | captureを持つ関数値を作る | 対応 | `static_closure.rs`でbackend内lift。静的captureまたはtyped heap環境。 | `captures_runtime_values_in_a_fixed_heap_closure_environment`、`lifts_nested_closures_that_capture_outer_closure_values`、`cell::cycles_through_captured_cells_preserve_live_callbacks_and_reclaim_the_last_root` | 動的captureは固定heapを使う。 |
| `Value::MakeClosure` | Heap IRのclosure表現を作る | Flow禁止 | Flow対象外。 | `flow::rejects_heap_only_values_and_reference_count_statements_at_flow_stage` | Heap IR専用。 |
| `Value::EnvGet` | Heap IRの環境を読む | Flow禁止 | Flow対象外。 | `flow::rejects_heap_only_values_and_reference_count_statements_at_flow_stage` | Heap IR専用。 |
| `Value::EvLookup` | effectの実装を検索する | 対応 | `emit_evidence_lookup`。後から追加したbindingを優先し、欠落はdefect。 | `dynamically_dispatches_through_a_fixed_heap_evidence_table`、`passes_dynamic_effect_evidence_to_a_captured_closure` | 後から追加したbindingが優先し、欠落はdefect。 |
| `Value::EvExtend` | effectの実装表を拡張する | 対応 | `emit_evidence_extend`。nodeがhandlerとparentを所有、空bindingはalias。 | `dynamically_dispatches_through_a_fixed_heap_evidence_table`、`passes_dynamic_effect_evidence_to_a_captured_closure` | nodeはparentとhandlerを所有する。 |
| `Value::Handler` | handler値を作る | 対応 | `emit_handler_value`。active flagと各operation closureを保持。 | `passes_dynamic_effect_evidence_to_a_captured_closure`、`suspend::duplicate_resume_reports_the_operation_and_handler` | Retire後の操作とresumeの多重使用を検査する。 |
| `Value::Fiber` | Fiber関連操作を行う | 対応 | suspend、child、deferred、causeと生成runtime。 | `fiber::integration::shared_state_child_reclamation_deferreds_and_external_failure_share_owned_edges_with_and_without_gc` | 下のFiberOp表と第3節。 |
| `Value::IsError` | errorの型tagを判定する | 対応 | Any/EmelaErrorの正tagをenum IDと比較。 | `propagates_catches_and_reports_fallible_calls` | 一般Any boxとerror payloadを区別する。 |
| `Value::AsError` | error payloadを取り出す | 対応 | matched error payloadからtyped enumを読む。managed結果はretain。 | `cell::cell_references_in_native_error_payloads_outlive_the_failing_frame` | 対応するerror判定後のpayloadを扱う。 |
| `Value::Runtime` | runtime用操作を行う | 対応 | emit_runtimeと必要な生成runtime。 | `debug_print_calls_the_board_hook_with_the_string_view`、`flow::rejects_heap_only_runtime_operations_at_flow_stage` | 下のRuntimeOp表を参照。 |
| `Value::Coerce` | 値表現・関数ABIを変換する | 対応 | `emit_coerce`、`function_coerce.rs`、`owned.rs`。Fn ABI wrapper、typed Any box、runtime handle、errorAny所有。 | `adapts_a_function_value_to_effect_evidence_and_failure_call_abis`、`passes_any_values_through_function_value_parameters_and_results`、`any_data::tagged_error_any_values_remain_normal_data_through_child_and_deferred_lifetimes`、`suspend::coerce_adapts_sync_and_suspend_function_values_to_a_suspend_evidence_abi` | 未解決の集合paramは受け付けない。 |

### PrimOp / PrimTy

| 演算 | 簡単な意味 | 状態 | `PrimTy::I32` | `PrimTy::I64` | `PrimTy::F64` | `PrimTy::String` |
| --- | --- | --- | --- | --- | --- | --- |
| `PrimOp::Add` | 加算 | 対応（有効な型） | 32 bit wrap | 64 bit wrap | double加算 | verifier拒否 |
| `PrimOp::Sub` | 減算 | 対応（有効な型） | 32 bit wrap | 64 bit wrap | double減算 | verifier拒否 |
| `PrimOp::Mul` | 乗算 | 対応（有効な型） | 32 bit wrap | 64 bit wrap | double乗算 | verifier拒否 |
| `PrimOp::Div` | 除算 | 対応（有効な型） | 0はdefect、MIN/-1はMIN | 同左 | IEEE除算 | verifier拒否 |
| `PrimOp::Rem` | 剰余 | 対応（有効な型） | 0はdefect、MIN%-1は0 | 同左 | fmod | verifier拒否 |
| `PrimOp::Neg` | 符号反転 | 対応（有効な型） | MIN 32 bit wrap | MIN 64 bit wrap | double符号反転 | verifier拒否 |
| `PrimOp::Not` | 論理反転 | 対応（有効な型） | 0/非0から1/0 | verifier拒否 | verifier拒否 | verifier拒否 |
| `PrimOp::Eq` | 等しい | 対応（有効な型） | 32 bit整数比較 | 64 bit整数比較 | IEEE比較（NaNを含む） | byte列の辞書順比較 |
| `PrimOp::Ne` | 等しくない | 対応（有効な型） | 32 bit整数比較 | 64 bit整数比較 | IEEE比較（NaNを含む） | byte列の辞書順比較 |
| `PrimOp::Lt` | 小さい | 対応（有効な型） | 32 bit整数比較 | 64 bit整数比較 | IEEE比較（NaNを含む） | byte列の辞書順比較 |
| `PrimOp::Le` | 以下 | 対応（有効な型） | 32 bit整数比較 | 64 bit整数比較 | IEEE比較（NaNを含む） | byte列の辞書順比較 |
| `PrimOp::Gt` | 大きい | 対応（有効な型） | 32 bit整数比較 | 64 bit整数比較 | IEEE比較（NaNを含む） | byte列の辞書順比較 |
| `PrimOp::Ge` | 以上 | 対応（有効な型） | 32 bit整数比較 | 64 bit整数比較 | IEEE比較（NaNを含む） | byte列の辞書順比較 |

「verifier拒否」は不正な型と演算の組合せであり、正常Flowの機能を制限する項目ではない。整数・Floatの演算は `primitive_expression` と型別helperが実装する。参照試験は `emits_and_runs_int32_primitive_operations`、`emits_and_runs_int64_primitive_operations_with_wrapping_semantics`、`emits_and_runs_float_primitive_operations`。String比較はpointer/長さで内容を比較し、heap確保を行わない。

### Const

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `Const::Int` | 32 bit整数 | 対応 | `constant_expression`。INT32 literal。 | `emits_c_for_a_function_returning_an_int_constant` | 対応する固定幅型を使う。 |
| `Const::Int64` | 64 bit整数 | 対応 | `constant_expression`。INT64 literal。 | `emits_and_runs_int64_constants_at_the_signed_boundaries` | 対応する固定幅型を使う。 |
| `Const::Float` | binary64値 | 対応 | `constant_expression`。double / NaN / Infinity。 | `emits_and_runs_float_constants_including_non_finite_values` | 対応する固定幅型を使う。 |
| `Const::Bool` | 真偽値 | 対応 | `constant_expression`。0/1。 | `compiles_core_bool_and_unit_constants` | 対応する固定幅型を使う。 |
| `Const::String` | UTF-8 byte列 | 対応 | `constant_expression`。pointer/uint16 byte長、読み取り専用領域、owner NULL。 | `emits_utf8_and_embedded_nul_string_constants_as_views` | 元のUTF-8 byte列とbyte長を保持する。 |
| `Const::Unit` | 値を持たない単位 | 対応 | `constant_expression`。C値・変数なし。 | `compiles_core_bool_and_unit_constants` | tuple等では位置を保持する。 |

### Callee

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `Callee::Direct` | 関数IDを直接呼ぶ | 対応 | targetのret/fails/suspendsからABIを決める。 | `compiles_direct_calls_to_internal_and_external_c_functions`、`propagates_catches_and_reports_fallible_calls`、`suspend::suspend_recursion_resumes_into_caller_storage_and_reuses_frames` | ret/fails/suspends/evidenceに一致するABIを使う。 |
| `Callee::Indirect` | 関数値を介して呼ぶ | 対応 | typed function recordのcall/env。 | `passes_any_values_through_function_value_parameters_and_results`、`propagates_failure_from_a_captured_closure_through_its_indirect_call`、`suspend::suspend_closure_keeps_captures_after_the_original_function_value_is_released` | ret/fails/suspends/evidenceに一致するABIを使う。 |
| `Callee::Op` | handlerの操作を呼ぶ | 対応 | handlerのactive flag・operation pointerを確認。 | `builtin_audit::retire::retired_handler_aliases_keep_the_active_check_in_native_and_suspend_callers`、`passes_dynamic_effect_evidence_to_a_captured_closure`、`suspend::suspend_handler_source_waits_and_resumes_into_the_callers_result` | 外部Cは第4節。Retireし得るeffectはalias越しの失効検査を維持する。 |

### Operand

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `Operand::Var` | 関数内の変数を読む | 対応 | `operand_expression` がVarIdをC名へ対応付ける。型はFunction.varsから取得。 | `emits_and_runs_recur_with_multiple_loop_variables` | 値移送時の所有権は利用する演算が管理する。 |
| `Operand::Const` | literalを直接読む | 対応 | `constant_expression`。上のConst表に対応。 | `emits_utf8_and_embedded_nul_string_constants_as_views`、`compiles_core_bool_and_unit_constants` | Unitは通常のC値を持たない。 |

### Stmt

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `Stmt::Let` | 値を評価して束縛する | 対応 | `emit_statement_inner` とsuspend Graph。managed destinationの旧値解放、borrowed projectorのretain、所有flagを扱う。FlowのlandingはNone | `emit::compiles_core_int_main_through_flow_ir_and_runs_generated_c`、`suspend::suspend_frames_own_string_inputs_and_return_owned_results_after_reuse`、`flow::rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage` | 値ごとのdiscard監査はValue/Builtin表を参照。 |
| `Stmt::If` | 真偽値で分岐する | 対応 | 同期C分岐、suspend `Node::Branch`と各armの`Node::Assign`。managed arm値はdestinationへ保持 | `emit::compiles_if_statements_and_boolean_short_circuiting`、`fiber::aggregate::string_aggregates_own_borrowed_inputs_and_native_results_across_suspend_and_cancellation` | 共通の資源・所有権契約に従う。 |
| `Stmt::Switch` | 構成子・定数で分岐する | 対応 | `emit_switch`、suspend `Node::Switch`。tag/Int32/Int64とdefaultを扱い、defaultなしtagの不正値はdefect | `emit::emits_and_runs_int32_and_int64_literal_switches`、`emit::emits_and_runs_exhaustive_payload_enum_switch_and_defects_on_invalid_tag`、`suspend::suspend_switch_handles_payload_and_nullary_tags` | defaultなしliteralはFlow不正。 |
| `Stmt::Loop` | loopを開始する | 対応 | `emit_loop`と`emit_recur`、suspend `LoopTarget` / `Node::Assign`。全引数を一度読む→retain→旧値release→更新の並列代入 | `emit::emits_and_runs_recur_with_multiple_loop_variables`、`suspend::suspend_parallel_recur_retains_managed_values_before_replacing_them`、`fiber::aggregate::parallel_recur_and_discarded_native_calls_reclaim_string_tuple_results` | local全体で最小の記憶領域を選ぶ保証はない。 |
| `Stmt::Block` | 脱出先を持つblockを作る | 対応 | 同期`BlockTarget`とlabel、suspend `Graph::blocks`。`Break`/`OnFail::Break`で値・errorを保持して脱出 | `emit::emits_and_runs_break_to_an_enclosing_block`、`suspend::suspend_loop_preserves_parallel_recur_and_block_result`、`fiber::payload::native_failure_channels_own_formatted_parse_errors_and_list_payloads` | errorのAny descriptorと通常boxを同じものと扱わない。 |
| `Stmt::Protect` | 本体とcleanupを定義する | 対応 | `emit_protect` / `emit_pending_cleanups`、suspend `Node::Protect/Leave/CleanupEnd`と`runtime/unwind.c`。cleanupのerror slotを本体と分離 | `emit::runs_protect_cleanup_on_yield_break_and_return`、`fiber::protect::suspend_protect_runs_cleanup_on_yield_return_and_break_without_heap`、`fiber::failure::caught_failure_inside_suspending_release_does_not_replace_the_original_error`、`fiber::protect::cancellation_preserves_a_suspending_release_and_its_pending_task` | defect即時停止はターゲット契約。cleanupの永久待ちを無条件に打ち切らない。 |
| `Stmt::Dup` | Heap IRの参照を増やす | Flow禁止 | Flowでは明示RCを受け取らず、`verify_flow`→共有verifierで拒否 | `flow::rejects_heap_only_values_and_reference_count_statements_at_flow_stage` | backendが必要なretainを挿入。 |
| `Stmt::Drop` | Heap IRの参照を減らす | Flow禁止 | Flowでは明示RCを受け取らず、共有verifierのstage検査で拒否する。 | `flow::rejects_heap_only_values_and_reference_count_statements_at_flow_stage` | backendが必要なreleaseを挿入。 |

### Term

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `Term::Yield` | compoundの結果を返す | 対応 | `emit_compound_sequence`、suspend `Graph::assign`。値を保持してcompound destinationへ渡す。cleanupはvalueless Yield | `emit::emits_and_runs_a_block_that_yields_a_value`、`fiber::protect::suspend_protect_runs_cleanup_on_yield_return_and_break_without_heap` | 関数/closure本体の最終YieldはFlow不正。 |
| `Term::Return` | 関数から戻る | 対応 | `emit_return`、suspend `Node::Return`。caller storageへの移送/保持とlocal/frame破棄。途中のProtectを先に実行 | `emit::returns_dynamic_string_append_into_the_callers_buffer`、`fiber::aggregate_abi::scope_returns_string_enum_exits_after_success_or_interruption_and_frame_reuse`、`fiber::protect::suspend_cleanup_waits_for_an_external_task_before_publishing_the_result` | Unit/NeverはC値なし。公開suspendのNULL出力も試験。 |
| `Term::Fail` | 回復可能な失敗を送る | 対応 | `emit_error_assignment` / `emit_failure_return`、suspend `Node::SetError/Fail`。errorを所有しcleanup後に配送 | `emit::propagates_catches_and_reports_fallible_calls`、`fiber::failure::suspend_failures_cross_calls_and_preserve_cleanup_before_escape`、`fiber::payload::list_error_payloads_survive_suspending_cleanup_and_are_dropped_on_cancellation` | fails=falseの伝播は拒否。 |
| `Term::Break` | blockから脱出する | 対応 | 同期labelとprotect depth、suspend `Graph::leave`。block結果の保持後に必要なcleanupを実行 | `emit::emits_and_runs_break_to_an_enclosing_block`、`fiber::protect::suspend_protect_runs_cleanup_on_yield_return_and_break_without_heap`、`fiber::function_aggregate::any_fields_nested_in_error_payloads_use_owned_boxes_and_indirect_failure_channels` | cleanupを越えて外へbreakするFlowはverifierが拒否。 |
| `Term::Recur` | loop引数を更新して繰り返す | 対応 | `emit_recur`、suspend `Node::Assign`。innermost loop引数の並列置換 | `emit::emits_and_runs_recur_with_multiple_loop_variables`、`suspend::suspend_parallel_recur_retains_managed_values_before_replacing_them`、`flow::rejects_recur_crossing_protect_and_return_leaving_cleanup` | Protectを越えるrecurは正常Flowでは禁止。Protect内のloopを回るrecurとは異なる。 |
| `Term::Defect` | defectで即時停止する | 対応 | `emit_defect` / suspend `Node::Defect`。kind、pointerとbyte長をboard hookへ渡す | `emit::sends_dynamic_defect_string_views_to_the_board_hook`、`emit::classifies_a_static_todo_defect`、`fiber::protect::defects_in_suspend_protect_keep_the_configured_immediate_stop_policy` | cleanup・解放・Attempt捕捉を行わない既定契約。 |
| `Term::Unwind` | Heap IRの保留defectを伝播する | Flow禁止 | 共有verifierのstage検査で拒否 | `flow::rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage` | 保留defectを持つHeap IR専用。 |
| `Term::Unreachable` | 到達不能な終端を表す | 対応 | 同期/Graphとも到達時はASSERT defect。通常の非到達枝としても許可 | `emit::synchronous_exhaustive_matches_and_list_loops_accept_unreachable_terminators`、`fiber::attempt::attempt_keeps_the_embedded_policy_of_stopping_immediately_on_defect` | 任意の未初期化値を返して継続しない。 |

### FiberOp

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `FiberOp::Scope` | 子Fiberの生存範囲を作る | 対応 | `suspend::emit_function`のScope分岐、`runtime/scope.c` | `fiber::scope_starts_children_at_real_suspension_and_reclaims_waiting_tasks`、`fiber::failure::failing_and_infallible_scopes_share_the_fork_operation_abi`、`fiber::aggregate_abi::scope_returns_string_enum_exits_after_success_or_interruption_and_frame_reuse` | fails混在、結果NULL、兄弟cleanup待ち、managed成功/失敗を確認。 |
| `FiberOp::Fork` | 子Fiberを作る | 対応 | `suspend::emit_scope_runtime` / `emit_fiber_value`、`emela_child_begin` | `fiber::child_reclaim::long_lived_scope_reclaims_completed_children_with_copied_handles_and_repeated_join_await`、`fiber::child_reclaim::discarded_completed_children_release_results_before_scope_end_and_survive_cancellation` | 実行/handle/waiterが所有しScope一覧はweak。未読のhandleをframeへ残さない。 |
| `FiberOp::Deferred` | 一度だけ完成する値を作る | 対応 | `suspend::emit_fiber_value`、`runtime/deferred.c` | `deferred::discarded_deferreds_reuse_one_slot_within_a_long_lived_scope`、`deferred::deferred_pool_exhaustion_uses_the_defined_defect` | handle寿命とslot generationを分ける。 |
| `FiberOp::Join` | 子の成功値を待つ | 対応 | `child::emit_wait` / `emit_runtime`、`runtime/child_wait.c` | `fiber::join::join_reuses_completed_results_without_yielding_and_waits_for_discarded_unit`、`fiber::join::join_results_own_strings_and_lists_after_scope_and_frame_reuse`、`fiber::interrupt::join_of_an_interrupted_child_is_the_defined_panic_defect` | 完了結果は反復読取可能。Unit/discardでも待機し、中断済み子のJoinはpanic。 |
| `FiberOp::Await` | 子のExitを待つ | 対応 | `child::emit_wait` / `emit_runtime` | `fiber::await_::await_returns_success_and_can_mix_with_repeated_join_without_yielding`、`fiber::interrupt::await_of_interrupted_children_returns_owned_failures_repeatedly_and_for_unit` | Exitの成功値/Causeを所有し、NULL結果は破棄。error/notesはScopeとも共有。 |
| `FiberOp::Interrupt` | Fiberの取消とcleanupを待つ | 対応 | `child::emit_wait`、`runtime/child_wait.c` / `unwind.c` | `fiber::interrupt::interrupt_waits_for_suspending_cleanup_and_ignores_old_task_notifications`、`fiber::interrupt::self_interrupt_unwinds_the_body_and_is_deferred_inside_cleanup` | cleanup完了まで待つ。hardwareのcancelそのものではない。 |
| `FiberOp::Attempt` | 失敗をCauseとして捕捉する | 対応 | `cause::emit_attempts` / `emit_attempt_call`、同期/非同期callback | `fiber::attempt::attempt_captures_sync_failure_and_returns_owned_cause`、`fiber::attempt::attempt_owns_managed_success_and_releases_discarded_results`、`fiber::attempt::attempt_waits_for_async_cleanup_before_catching_failure` | Fail捕捉・Interrupt非捕捉。Dieは既定即時停止契約により捕捉しない。 |
| `FiberOp::Complete` | Deferredを完成させる | 対応 | `deferred::emit_complete` / `emit_runtime`、`emela_deferred_prepare_complete` | `deferred::complete_stores_once_and_scope_end_breaks_cycles_through_captured_deferreds`、`deferred::complete_copies_borrowed_string_and_poll_retains_it_past_scope_end` | 最初の値だけ保存。borrowed Stringをsnapshotし、複数回のcompleteで旧値を失わない。 |
| `FiberOp::DeferredAwait` | Deferredの完成を待つ | 対応 | `deferred::emit_await`、`runtime/deferred_await.c` | `deferred::await_resumes_all_waiters_in_fifo_order_after_completer_finishes`、`deferred::await_preserves_string_and_list_results_past_frame_and_scope_reuse` | NULL/Unitも待機し、配送前取消ではwaiterとresult所有を解放。 |
| `FiberOp::Poll` | Deferredの現在値を読む | 対応 | `deferred::emit_poll` / `emit_runtime` | `deferred::poll_observes_empty_and_first_value_for_scalar_and_unit_without_yielding`、`fiber::aggregate::deferred_complete_poll_and_await_share_owned_string_aggregates_past_scope_end` | Optionを保持。dstなしは値をcopyせずhandle/Scope有効性を検査。 |
| `FiberOp::CauseKind` | Causeの分類を読む | 対応 | `cause::emit_kind` / `emela_cause_checked` | `fiber::cause::cause_kind_reads_failure_without_consuming_the_cause` | 観測のみでCauseを消費しない。 |
| `FiberOp::CauseToString` | Causeを表示する | 対応 | `cause::emit_to_string`、`StringBufferPlan` | `fiber::cause::cause_to_string_uses_error_show_and_caller_output_buffer`、`fiber::notes::note_display_checks_the_callers_remaining_capacity` | 出力容量を検査。dstなしでも表示評価と容量defectを維持する。 |
| `FiberOp::CauseRaise` | Causeを制御経路として再送する | 対応 | `emit_statement_inner`のCauseRaise分岐、`cause::emit_infallible_raise` | `fiber::cause::cause_raise_preserves_the_error_and_runs_native_cleanup_before_escape`、`fiber::cause::raised_interruption_crosses_sync_calls_and_attempt_without_becoming_a_value`、`fiber::cause::failed_empty_cause_raised_in_cleanup_returns_to_its_failing_scope` | Neverで後続を実行しない。Fail/Interruptを分離し、空集合Causeのsource正規化は`flow_from_core`内だけ。 |

### 失敗辺・分岐・stage

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `OnFail::Break` | 失敗をblockへ送る | 対応 | `emit_call_failure_edge`、`Graph::failure`。errorを所有してlabelのdestinationへ渡し、protect depthに応じcleanup | `emit::propagates_catches_and_reports_fallible_calls`、`fiber::failure::suspend_failures_cross_calls_and_preserve_cleanup_before_escape` | 所有するerrorを配送し、必要なcleanupを実行する。 |
| `OnFail::Propagate` | 失敗を呼出し元へ送る | 対応 | 上記と`emit_failure_return` / `Node::Fail`。fails=falseへの伝播は拒否 | `fiber::failure::uncaught_suspend_failure_stops_at_public_boundary_after_release`、`fiber::function_aggregate::failing_indirect_aggregate_calls_release_discarded_successes_and_preserve_string_errors` | fails=falseの関数への伝播は拒否する。 |
| `OnFail::Handle` | Heap IRのlandingへ送る | Flow禁止 | Heap IRの明示landing。Flowでは共有verifierが拒否 | `flow::rejects_heap_only_failure_handle_at_flow_stage` | 明示landingを持たない。 |
| `on_defect` / `on_suspend` | defect/suspendのlanding辺 | Flow禁止（Someの場合） | Flow入力では両方None。共有verifierのstage検査。backendのsuspend Graphは独自に作る | `flow::rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage` | 両方Noneを要求する。 |
| `SwitchOn::Tag` / `CaseKey::Variant` | enum構成子で分岐する | 対応 | enumのnullary/inline/recursive representationに応じtagを読む | `emit::emits_and_runs_switches_on_nullary_enum_representations`、`suspend::suspend_switch_handles_payload_and_nullary_tags` | 格納表現に応じてtagを読む。 |
| `SwitchOn::Lit` / `CaseKey::Lit` | 整数literalで分岐する | 対応 | Int32/Int64の定数case、default必須 | `emit::emits_and_runs_int32_and_int64_literal_switches` | Int32/Int64、default必須。 |
| `Stage::Flow` | Flow段階の入力を示す | 対応 | `compile_flow` はclosedなfunction setsをABIへ正規化後、`verify_flow` を呼ぶ。 | `flow::lowers_and_verifies_core_as_flow_ir` | 正規化後に型・stageを検証する。 |
| `Stage::ClosuresConverted` / `Stage::DefectsExplicit` / `Stage::StateMachines` / `Stage::RefCounted` | Heap IR以降の段階を示す | Flow禁止 | stageそのものを入口で拒否し、Heap passを呼ばない | `flow::rejects_every_heap_stage_before_codegen` | 入口で拒否し、Heapパスを呼ばない。 |

### Builtin

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `Builtin::StringLength` | 書記素数を数える | 対応 | `emit_builtin` → `emit_string_length`、`unicode_grapheme::emit_runtime` | `string_length_uses_unicode_18_extended_grapheme_clusters` | Unicode 18の表を使用。公式vectorはUnicode helperのunit testでも確認。実行時間は入力長に依存する。 |
| `Builtin::StringByteSize` | UTF-8のbyte長を読む | 対応 | `emit_builtin` | `string_byte_size_counts_utf8_bytes_from_a_view` | 長さ付きviewを直接読む。NUL終端には依存しない。 |
| `Builtin::StringAppend` | Stringを連結する | 対応 | `emit_builtin`、`StringBufferPlan::new`、`emit_string_append_helper` | `appends_dynamic_strings_into_one_inferred_stack_buffer`、`string_append_capacity_overflow_uses_the_board_defect_hook`、`string_append::literal_prefixes_are_copied_without_treating_readonly_bytes_as_writable_output`、`builtin_audit::discarded_string_producers_share_scratch_and_preserve_numeric_bounds` | 容量と生存期間を計画しreadonly literalへ書き込まない。未使用結果の直後suspendも確認。任意CFGの最小scratch量は保証しない。 |
| `Builtin::StringContains` | 部分文字列を検索する | 対応 | `emit_builtin`、`emit_string_contains_helper` | `string_search_predicates_match_utf8_substrings_without_copying` | byte列の検索。空String/UTF-8を含む。 |
| `Builtin::StringStartsWith` | prefixを判定する | 対応 | `emit_builtin`、`emit_string_starts_with_helper` | `string_search_predicates_match_utf8_substrings_without_copying` | 同じ試験内でprefix境界を確認。 |
| `Builtin::StringEndsWith` | suffixを判定する | 対応 | `emit_builtin`、`emit_string_ends_with_helper` | `string_search_predicates_match_utf8_substrings_without_copying` | 同じ試験内でsuffix境界を確認。 |
| `Builtin::StringSplit` | 区切りごとのListを作る | 対応 | `emit_builtin` → `emit_string_split` / `emit_string_split_push` | `string_split_keeps_views_alive_after_copying_the_source` | 入力を所有し、各要素はそのview。空separatorは書記素境界。 |
| `Builtin::StringChars` | 書記素ごとのListを作る | 対応 | `emit_builtin` → `emit_string_chars` | `string_chars_handles_unicode_graphemes`、`string_chars_matches_the_unicode_18_grapheme_break_test`、`builtin_audit::discarded_string_chars_still_builds_and_releases_its_owned_list` | 公式Unicode vector、所有参照、捨てた結果の構築・即時解放を扱う試験がある。 |
| `Builtin::StringJoin` | StringのListを結合する | 対応 | `emit_builtin` → `emit_string_join`、`emit_string_join_helper` | `string_join_reads_split_views_into_a_bounded_stack_buffer` | List要素のviewを容量付き出力へ結合する。作業量はList長と要素byte長に依存する。 |
| `Builtin::StringTrim` | 前後のUnicode空白を除く | 対応 | `emit_builtin`、`emit_string_trim_helper`、`emit_statement`のretain | `string_trim_returns_a_unicode_whitespace_view`、`preserves_a_string_scratch_buffer_while_a_trim_view_is_live` | ownerを維持するviewとして返す。scratchの生存期間も確認。 |
| `Builtin::StringCodePoints` | code pointのListを作る | 対応 | `emit_builtin` → `emit_string_code_points` | `string_code_points_builds_and_releases_a_list_in_code_point_order` | 順序・NUL・UTF-8と全解放を確認。 |
| `Builtin::StringFromCodePoint` | Unicode scalarをStringへ変換する | 対応 | `emit_builtin`、`StringBufferPlan::new` | `string_from_code_point_encodes_valid_scalars_and_rejects_invalid_ones`、`dynamic_string_from_code_point_checks_capacity_before_writing`、`builtin_audit::discarded_option_builtins_infer_their_result_layout_without_a_source_option_type` | 不正scalarはNone、動的出力は最大4 byte。Option型が未生成のFlowで未使用結果を確認。 |
| `Builtin::IntToString` | Intを十進Stringにする | 対応 | `emit_builtin`、`emit_int_to_string_helper`、`StringBufferPlan::new` | `int_to_string_uses_small_scratch_and_matches_signed_decimal_output`、`int_to_string_capacity_overflow_uses_the_board_defect_hook`、`builtin_audit::discarded_string_producers_share_scratch_and_preserve_numeric_bounds` | 符号を含む最大11 byte。最小Intと容量defectを確認。 |
| `Builtin::IntParse` | 十進StringをIntへ変換する | 対応 | `emit_builtin`、`emit_int_parse_helper` | `int_parse_accepts_decimal_signs_and_rejects_invalid_or_overflowing_text`、`builtin_audit::discarded_option_builtins_infer_their_result_layout_without_a_source_option_type` | pointer+長さで読み、文法/overflowをNoneへ変換。未使用Option結果も確認。 |
| `Builtin::IntCheckedAdd` | overflow付き加算をOptionで返す | 対応 | `emit_builtin` | `int_checked_add_returns_some_or_none_at_int_boundaries` | int64へ拡張して境界を確認し、Int範囲を越えたらNone。 |
| `Builtin::IntBitAnd` | 32 bitのAND | 対応 | `emit_builtin` | `int_bitwise_builtins_match_signed_32_bit_semantics` | 符号付き境界を含む4演算の共通試験。 |
| `Builtin::IntBitOr` | 32 bitのOR | 対応 | `emit_builtin` | `int_bitwise_builtins_match_signed_32_bit_semantics` | 同じ試験内でOR結果を確認。 |
| `Builtin::IntBitXor` | 32 bitのXOR | 対応 | `emit_builtin` | `int_bitwise_builtins_match_signed_32_bit_semantics` | 同じ試験内でXOR結果を確認。 |
| `Builtin::IntBitNot` | 32 bitの反転 | 対応 | `emit_builtin` | `int_bitwise_builtins_match_signed_32_bit_semantics` | 同じ試験内でNOT結果を確認。 |
| `Builtin::IntShiftLeft` | 32 bitの左shift | 対応 | `emit_builtin`、`emit_i32_shift_helpers` | `int_shift_builtins_handle_width_and_negative_amount_boundaries` | 負量はdefect、幅以上も分岐してCの不正shiftを避ける。 |
| `Builtin::IntShiftRight` | 32 bitの算術右shift | 対応 | `emit_builtin`、`emit_i32_shift_helpers` | `int_shift_builtins_handle_width_and_negative_amount_boundaries` | 符号保持と0/幅以上を確認。 |
| `Builtin::IntShiftRightUnsigned` | 32 bitの論理右shift | 対応 | `emit_builtin`、`emit_i32_shift_helpers` | `int_shift_builtins_handle_width_and_negative_amount_boundaries` | unsignedへ変換してshiftし、bit列を戻す。 |
| `Builtin::Int64FromInt` | Intを符号拡張する | 対応 | `emit_builtin` | `int64_conversions_preserve_sign_and_keep_the_low_32_bits` | 符号付き32 bitを64 bitへ変換。 |
| `Builtin::Int64ToString` | Int64を十進Stringにする | 対応 | `emit_builtin`、`emit_int_to_string_helper`、`StringBufferPlan::new` | `int64_to_string_uses_twenty_byte_scratch_and_matches_decimal_output` | 符号を含む最大20 byte。最小/最大/0を確認。 |
| `Builtin::Int64ToInt` | 下位32 bitをIntとして返す | 対応 | `emit_builtin`、`emela_i32_from_bits`を生成するwrap helper | `int64_conversions_preserve_sign_and_keep_the_low_32_bits` | 範囲外のsigned castへ依存せず下位bitを復元する。 |
| `Builtin::Int64BitAnd` | 64 bitのAND | 対応 | `emit_builtin` | `int64_bitwise_builtins_match_signed_64_bit_semantics` | 64 bit符号境界と4演算を確認。 |
| `Builtin::Int64BitOr` | 64 bitのOR | 対応 | `emit_builtin` | `int64_bitwise_builtins_match_signed_64_bit_semantics` | 同じ試験内でOR結果を確認。 |
| `Builtin::Int64BitXor` | 64 bitのXOR | 対応 | `emit_builtin` | `int64_bitwise_builtins_match_signed_64_bit_semantics` | 同じ試験内でXOR結果を確認。 |
| `Builtin::Int64BitNot` | 64 bitの反転 | 対応 | `emit_builtin` | `int64_bitwise_builtins_match_signed_64_bit_semantics` | 同じ試験内でNOT結果を確認。 |
| `Builtin::Int64ShiftLeft` | 64 bitの左shift | 対応 | `emit_builtin`、`emit_i64_shift_helpers` | `int64_shift_builtins_handle_width_and_negative_amount_boundaries` | amountはInt。負量はdefect、64以上も明示分岐。 |
| `Builtin::Int64ShiftRight` | 64 bitの算術右shift | 対応 | `emit_builtin`、`emit_i64_shift_helpers` | `int64_shift_builtins_handle_width_and_negative_amount_boundaries` | 符号保持と幅境界を確認。 |
| `Builtin::Int64ShiftRightUnsigned` | 64 bitの論理右shift | 対応 | `emit_builtin`、`emit_i64_shift_helpers` | `int64_shift_builtins_handle_width_and_negative_amount_boundaries` | unsignedでshiftしbit列を復元。 |
| `Builtin::FloatToString` | binary64をshortest Stringにする | 対応 | `emit_builtin`、`float_to_string::emit_runtime`、`StringBufferPlan::new` | `dynamic_float_to_string_matches_const_formatting_in_caller_owned_buffers` | 参照試験は±0、非有限値、subnormal、負の25 byte例、const照合と容量defectを扱う。 |
| `Builtin::FloatFromInt` | Intを正確なFloatへ変換する | 対応 | `emit_builtin` | `float_from_int_converts_the_full_signed_range_exactly` | 32 bitの符号付き整数はbinary64へ正確に入る。hostで代表境界を確認。 |
| `Builtin::FloatParse` | Stringをbinary64へ変換する | 対応 | `emit_builtin`、`float_parse::emit_runtime` | `dynamic_float_formatting_uses_a_bounded_runtime_and_float_parse_uses_the_bounded_parser`、`builtin_audit::discarded_option_builtins_infer_their_result_layout_without_a_source_option_type` | 文法・NUL・丸め・overflow/underflow・固定作業領域を確認。未使用Option結果も確認。 |
| `Builtin::FloatFloor` | 負方向へ丸めてIntにする | 対応 | `emit_builtin`、`emit_float_to_int_helper` | `float_rounding_builtins_match_spec_and_defect_out_of_range`、`builtin_audit::discarded_float_rounding_preserves_the_out_of_range_defect` | 非有限/範囲外を変換前に検査。未使用でもNaN defectを維持する試験がある。 |
| `Builtin::FloatCeil` | 正方向へ丸めてIntにする | 対応 | `emit_builtin`、`emit_float_to_int_helper` | `float_rounding_builtins_match_spec_and_defect_out_of_range` | 共通helperのoperation1。 |
| `Builtin::FloatRound` | tieを0から遠ざけてIntにする | 対応 | `emit_builtin`、`emit_float_to_int_helper` | `float_rounding_builtins_match_spec_and_defect_out_of_range` | 正負tie、Int境界とNaNを確認。 |
| `Builtin::FloatTruncate` | 小数部を捨ててIntにする | 対応 | `emit_builtin`、`emit_float_to_int_helper` | `float_rounding_builtins_match_spec_and_defect_out_of_range` | 共通helperのoperation3。 |
| `Builtin::Panic` | message付きdefectを発生させる | Flow禁止（lower先に対応） | `from_core::Lower::builtin`、`emit_return`のDefect分岐、`lib::verify_flow`の入口guard | `sends_dynamic_defect_string_views_to_the_board_hook`、`rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | sourceのlower先は実装済み。生BuiltinはInvalidFlowIr。組み込み契約に従いdefect時はcleanupを省略する。 |
| `Builtin::Todo` | 未実装defectを発生させる | Flow禁止（lower先に対応） | `from_core::Lower::builtin`、`emit_return`のDefect分岐、入口guard | `classifies_a_static_todo_defect`、`rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | 同じlower経路でTODO kindに分類。生BuiltinはInvalidFlowIr。defectの即時停止契約を維持。 |
| `Builtin::AssertFail` | 失敗したassertのdefectを場所つきで発生させる | Flow禁止（lower先に対応） | `from_core::Lower::builtin`、`emit_return`のDefect分岐、入口guard | `rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | `Term::Defect`の場所はboard hookに渡さず、messageだけを使う。生BuiltinはInvalidFlowIr。 |
| `Builtin::Dbg` | show出力後に元の値を返す | Flow禁止（lower先に対応） | `from_core::Lower::builtin`、`emit_runtime` | `debug_print_calls_the_board_hook_with_the_string_view`、`debug_print_can_consume_a_runtime_show_view_before_return`、`rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | 生BuiltinはInvalidFlowIr。debug hookの改行/出力先はboard契約。 |
| `Builtin::Inspect` | Showによるdebug文字列を返す | Flow禁止（lower先に対応） | `mono::debug`、`from_core::Lower::builtin` | `release_0_24::derived_and_written_traits_keep_static_dispatch_and_escaped_display`、`rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | 型はShowを満たす必要がある。Stringは最上位でもquoteする。生BuiltinはInvalidFlowIr。 |
| `Builtin::ShowNested` | List・Option等の要素をShow表示する | Flow禁止（lower先に対応） | `mono::expr`、`mono::derive` | `release_0_24::derived_and_written_traits_keep_static_dispatch_and_escaped_display`、`rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | 通常のFlowには現れない。String以外もユーザー定義Showへ静的dispatchする。 |
| `Builtin::ShowString` | Stringをquote・escapeする | Flow禁止（lower先に対応） | `from_core::Lower::builtin`、`emit_runtime` | `release_0_24::derived_and_written_traits_keep_static_dispatch_and_escaped_display`、`rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | lowering内部用でALLには含まれない。長さ付きviewのNULを保持し、出力容量を検査する。 |
| `Builtin::DebugPrint` | 表示済みStringを1行debug出力する | Flow禁止（lower先に対応） | `from_core::Lower::builtin`、`emit_runtime` | `release_0_24::debug_and_error_display_respect_written_derived_and_missing_show`、`rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | lowering内部用でALLには含まれない。board hookは長さ付きStringを借用し、生BuiltinはInvalidFlowIr。 |
| `Builtin::CellNew` | 共有Cellを作る | 対応 | `emit_cell_builtin` | `state_run_shares_owned_cell_storage_with_its_handler`、`cell::discarded_cell_construction_and_reads_release_their_owned_contents`、`cell::cycles_through_cell_any_boxes_are_reclaimed_without_losing_external_roots` | 固定heap、String snapshot、cycle回収を確認。source capability制約とISR非対応契約を維持する。 |
| `Builtin::CellGet` | 共有Cellの値を読む | 対応 | `emit_cell_builtin` | `cell::shared_cells_keep_string_updates_alive_across_capture_suspend_and_cancellation`、`cell::unit_cells_keep_a_shared_heap_identity_without_reading_a_void_payload` | capture/suspend/cancel/Unitの所有を確認。 |
| `Builtin::CellSet` | 共有Cellの値を置き換える | 対応 | `emit_cell_builtin`、`owned::TypeLayouts::emit_own_value_in_place` | `cell::shared_cells_keep_string_updates_alive_across_capture_suspend_and_cancellation`、`cell::nested_cells_and_cells_in_enum_loop_values_keep_shared_identity` | 新値をownしてpublish後に旧値release。 |

### RuntimeOp

| variant | 簡単な意味 | 状態 | 実装 | 試験の参照 | 制約・契約 |
| --- | --- | --- | --- | --- | --- |
| `RuntimeOp::ShowString` | 引用符とescapeを含む表示用Stringを作る | 対応 | `emit_runtime`、`emit_string_show_helpers`、`StringBufferPlan::new` | `shows_dynamic_strings_into_a_bounded_scratch_buffer`、`runtime_show_string_capacity_overflow_uses_the_board_defect_hook`、`builtin_audit::discarded_runtime_show_string_keeps_its_capacity_check` | pointer+長さでescapeする。未使用結果でも容量defectを維持する試験がある。全入力Stringの網羅ではない。 |
| `RuntimeOp::DebugPrint` | 表示用Stringをboardへ渡す | 対応 | `emit_runtime` | `debug_print_calls_the_board_hook_with_the_string_view`、`debug_print_can_consume_a_runtime_show_view_before_return` | board hookへdata/lengthを渡す。boardは呼出し中に読み切るかcopyし、ISRから到達する場合はISR安全に実装する。 |
| `RuntimeOp::Retire` | 解放済みhandlerを無効にする | 対応 | `emit_runtime`、`emit_handler_operation_call`、`suspend::emit_operation`、`static_handler::lower` | `builtin_audit::retire::retired_handler_aliases_keep_the_active_check_in_native_and_suspend_callers`、`builtin_audit::retire::unretired_static_handlers_keep_the_heapless_dispatch_path` | alias経由の操作も失効検査する。Retireしない静的effectはheapなしで特殊化できる。 |
| `RuntimeOp::Defect` | 明示unwind用のdefectを設定する | Flow禁止 | `lib::verify_flow`の入口guard | `rejects_heap_only_runtime_operations_at_flow_stage` | Flow入口でInvalidFlowIr。共通LIR verifierに加えてcodegen側でstage検査する。実行する対象ではない。 |
| `RuntimeOp::DefectAt` | 失敗したassertの場所つきでdefectを設定する | Flow禁止 | `lib::verify_flow`の入口guard | `rejects_heap_only_runtime_operations_at_flow_stage` | 同じ入口guardで拒否。Heap IR runtimeは実装しない。 |
| `RuntimeOp::DefectTake` | unwind中のdefectを取り出す | Flow禁止 | `lib::verify_flow`の入口guard | `rejects_heap_only_runtime_operations_at_flow_stage` | 同じ入口guardで拒否。Heap IR runtimeは実装しない。 |
| `RuntimeOp::DefectResume` | 保存したdefectのunwindを再開する | Flow禁止 | `lib::verify_flow`の入口guard | `rejects_heap_only_runtime_operations_at_flow_stage` | 同じ入口guardで拒否。Heap IR runtimeは実装しない。 |
| `RuntimeOp::Hold` | fiberのinterrupt/cancelを延期する | 対応 | `emit_runtime`、runtimeの`emela_interrupt_hold` | `fiber::protect::cancellation_preserves_a_suspending_release_and_its_pending_task`、`fiber::interrupt::operation_tails_do_not_keep_a_copy_of_an_expired_cleanup_mask` | release中の取消とsuspendを間接検証。`flow::isr_entries_reject_reachable_hold_and_unhold_in_direct_and_nested_calls`でISRからの到達は拒否。hardware割り込みmaskではない。無制限のhold nestingは保証しない。 |
| `RuntimeOp::Unhold` | 延期したfiberのinterrupt/cancelを再開する | 対応 | `emit_runtime`、runtimeの`emela_interrupt_unhold` | `fiber::protect::cancellation_preserves_a_suspending_release_and_its_pending_task`、`fiber::interrupt::operation_tails_do_not_keep_a_copy_of_an_expired_cleanup_mask` | source loweringのHold/Retire/Unhold列を確認。ISR到達の試験で直接/callee経由とも拒否し、通常処理の実行は維持。 |
| `RuntimeOp::Yield` | state machineの公平性のためにyieldする | Flow禁止 | `lib::verify_flow`の入口guardと共通verifierのstage検査 | `rejects_heap_only_runtime_operations_at_flow_stage` | Flow入口で拒否。組み込みschedulerが行うyieldとは別のIR operation。 |
| `RuntimeOp::YieldPerform` | effect境界の公平性のためにyieldする | Flow禁止 | `lib::verify_flow`の入口guardと共通verifierのstage検査 | `rejects_heap_only_runtime_operations_at_flow_stage` | 同じ入口guardで拒否。組み込みschedulerの実装にこのHeap IR operationは要求しない。 |

## 7. 対応表の監査と確認範囲

[`coverage_audit.rs`](../tests/coverage_audit.rs) と [`flow_audit_inventory.rs`](../examples/support/flow_audit_inventory.rs) は、本体のenum定義と上記15種類のinventory marker、表示されたvariant名、試験列に書かれた実在の試験名を照合する。この監査は行の欠落や参照切れを見つけるもので、試験の実行結果や意味論の完全性を代わりに証明するものではない。

| 確認対象 | 試験の参照 | 主な境界 |
| --- | --- | --- |
| 入力stageと不正Flow | `flow::rejects_every_heap_stage_before_codegen`、`flow::rejects_source_builtins_that_must_lower_to_flow_control_or_runtime` | source builtinのlower先と、Heap専用variantの拒否を区別する。 |
| constの初期化・所有・診断 | `const_runtime::verified_flow_runtime_scalar_const_uses_no_heap_and_caches_discarded_first_reference`、`const_runtime::persistent_const_roots_survive_cycle_collection_and_caller_release` | 正常Core/Flowのcache契約。source loweringの制約は第1節。 |
| 成長型と構造操作 | `completion_types::growing_enum_opaque_edges_trace_cycles_through_cell_and_any` | 成長型はFlow producer経路。Core便宜APIからの自動生成を含まない。 |
| 通常のnested enumのerror表示 | `completion_types::derived::nested_generic_error_payloads_have_finite_display_functions` | 本試験は通常のWrap型を扱い、成長型のsource loweringを検証しない。 |
| 外部Cの所有契約 | `sync_external::source_string_adapters_snapshot_borrowed_results_and_board_held_arguments`、`const_runtime::external::pure_external_aggregate_const_owns_string_and_list_fields_after_board_release` | ABI mapping、借用・所有、caller buffer、managed fieldの寿命。 |
| schedulerの固定poolと通知 | `fixed_scheduler_bounds_work_reuses_pools_and_rejects_stale_handles` | main側の処理数・pool再利用とstale検査。実割込みtimingやWCETの測定ではない。 |

表の「対応」をもって、全Emelaソースが任意の小容量ボードで動作する、CLIからfirmwareを書き込める、すべての組合せを実行検証した、という意味にはしない。後続のfrontend/CLI接続やボードdriverの拡張では、ここで定義したIR・所有権・資源・外部Cの契約を保ち、対象ごとの生成物と実行結果を確認する。
