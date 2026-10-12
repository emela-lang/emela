//! 型推論の診断．単一化の失敗（[`crate::TypeError`]）には位置を付けてここに入れる．
//!
//! 文面とエラーコードは driver が決める．ここでは中身だけを持つ．

use emela_resolve::hir::Span;
use smol_str::SmolStr;

use crate::error::TypeError;
use crate::ty::Ty;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Prelude の宣言の中の型はソースがないので `None`．
    pub span: Option<Span>,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// 型引数の数が合わない（E0304）．`List[Int, Int]`，型引数を書いていない `Option`．
    WrongTypeArgCount {
        name: SmolStr,
        expected: usize,
        found: usize,
    },
    /// alpha.1 ではまだ扱えない構文．`what` は構文の名前（`Self` など）．
    Unsupported { what: &'static str },
    /// 単一化の失敗（E0301 の型が合わない，E0302 の無限型）．
    Mismatch(TypeError),
    /// 呼び出しの引数の数が合わない（E0305）．`callee` は呼んだ関数か構成子の名前．
    /// 名前で指していない値（局所変数，ラムダ）を呼んだときは `None`．
    WrongArgCount {
        callee: Option<SmolStr>,
        expected: usize,
        found: usize,
    },
    /// そのラベルの引数がない（E0306）．
    UnknownLabel { callee: SmolStr, label: SmolStr },
    /// 同じ引数を2回渡した（E0307）．位置引数とラベルで同じ引数を指したときも含む．
    DuplicateArg { label: SmolStr },
    /// ラベルを持たない値（局所変数，ラムダ）の呼び出しで名前付き引数を書いた（E0306）．
    LabelOnUnnamedCallee { label: SmolStr },
    /// 構成子のパターンのフィールドの数が合わない（E0309）．多すぎる（`Rect(w, h, d)`），
    /// または位置だけのフィールドの構成子で `..` なしに足りない（`Rect(w)`）．
    PatternFieldCount {
        ctor: SmolStr,
        expected: usize,
        found: usize,
    },
    /// 名前付きフィールドの構成子のパターンで，`..` を書かずにフィールドを省いた（E0308，6.5）．
    /// `fields` は省いたフィールドの全部（宣言の順）．
    MissingPatternFields { ctor: SmolStr, fields: Vec<SmolStr> },
    /// 数の型でない（E0310）．`integer` なら Int か Int64 が要る（整数リテラル），
    /// そうでなければ Int，Int64，Float のどれかが要る（算術演算子，6.6）．
    NotNumeric { found: Ty, integer: bool },
    /// 整数リテラルが型の範囲を超える（E0311，2.4）．
    IntLiteralOutOfRange { literal: SmolStr, ty: Ty },
    /// そのフィールドがない（E0312）．フィールドの参照と部分更新で使う．
    NoField { ty: Ty, field: SmolStr },
    /// フィールドを持たない型のフィールドを引いた（E0313）．enum，基本型，タプルなど．
    NoFields { ty: Ty, field: SmolStr },
    /// 型がまだわからない値のフィールドを引いた（E0314）．型注釈が要る．
    FieldOnUnknownType { field: SmolStr },
    /// opaque な型のフィールドを，定義モジュールの外で引いた（E0315，4.4）．
    OpaqueField { ty: Ty, field: SmolStr },
    /// トップレベルの fn の引数に型を書いていない（E0316，6.1）．
    MissingParamType { param: SmolStr },
    /// match が網羅していない（E0303，6.5）．`missing` は足りない値の例（`Rect(_, _)`）．
    NonExhaustiveMatch { missing: Vec<SmolStr> },
    /// 束縛のパターンが失敗しうる（E0317，6.3）．`missing` は合わない値の例．
    RefutableBinding { missing: Vec<SmolStr> },
    /// それより上の腕で全部覆われている腕（W0301，6.5）．警告．
    UnreachableArm,
    /// 本体のエラーが宣言の E に含まれない（E0318，7.4）．`errors` は含まれないエラー
    /// （タグか集合の型引数の名前）．`declared` は fails 節を書いたか（書いていなければ
    /// エラーなしの宣言で，未処理のエラー）．const の値も失敗できない（5.6．`declared` は偽）．
    UnhandledErrors {
        errors: Vec<SmolStr>,
        declared: bool,
    },
    /// エラーの型でない値を `fail` に渡した，または1つのエラーの型の型引数をエラーでない型で
    /// 具体化した（E0319，7.2）．
    NotAnError { found: Ty },
    /// escape の腕のパターンがエラーの構成子でない（E0320，7.3）．
    EscapeArmNotError,
    /// 関数の値の E が，期待する関数型の E に含まれない（E0321，7.4，9.2）．
    /// `expected` と `found` は集合の表示（`Timeout | E`，空なら `{}`）．
    ErrorSetMismatch { expected: SmolStr, found: SmolStr },
    /// escape の腕が受けるエラーが，元の式から起きない（W0302，7.3）．警告．
    UnraisedError { error: SmolStr },
    /// 能力の値のエフェクトに，その名前の操作がない（E0401，8.2）．
    UnknownOperation { effect: SmolStr, op: SmolStr },
    /// 本体の R が，宣言や文脈の許す R に含まれない（E0402，8.3，9.3，13.3）．`effects` は含まれない
    /// エフェクト（タグか集合の型引数の名前）．
    UnhandledEffects {
        effects: Vec<SmolStr>,
        context: EffectContext,
    },
    /// 関数の値の R が，期待する関数型の R に含まれない（E0403，9.2）．
    /// `expected` と `found` は効果の集合の表示（`{ Logger, Clock }`，空なら `{}`）．
    EffectSetMismatch { expected: SmolStr, found: SmolStr },
    /// handler がエフェクトの操作を全部は実装していない（E0404，8.4）．`ops` は足りない操作（宣言の順）．
    MissingHandlerOps { handler: SmolStr, ops: Vec<SmolStr> },
    /// handler が同じ操作を2回実装した（E0405，8.4）．
    DuplicateHandlerOp { op: SmolStr },
    /// handler の操作の引数の数が，エフェクトの操作の宣言と違う（E0406，8.4）．
    HandlerOpArgCount {
        op: SmolStr,
        expected: usize,
        found: usize,
    },
    /// `use` を省略した `@external` の fn か，`@external suspend fn` を，ハンドラの操作の本体の外で
    /// 使った（E0407，12.2）．`suspend` は後者か（注記を変える）．
    ExternalOutsideHandler { name: SmolStr, suspend: bool },
    /// 能力の値を関数の引数に渡した（E0408，8.2）．
    CapabilityArgument,
    /// 能力の値をデータ構造（リスト，タプル，構成子）に入れた（E0409，8.2）．
    CapabilityInData,
    /// 能力の値を関数から返した（E0410，8.2）．
    CapabilityReturned,
    /// 同じ with か layer に並べたハンドラが，互いの効果に依存して循環している（E0411，8.3，8.5，
    /// 14.2）．`cycle` は循環をたどるハンドラの名前（最初のハンドラを最後にもう一度入れる）．
    HandlerCycle { cycle: Vec<SmolStr> },
    /// ハンドラの値の構成子を，そのハンドラの init の外で使った（E0418，8.4）．
    HandlerCtorOutsideInit { handler: SmolStr },
    /// フィールドのあるハンドラに init がない（E0419，8.4）．
    MissingInit { handler: SmolStr },
    /// release が失敗しうる（E0420，8.4）．`errors` はその式から起きうるエラー．
    ReleaseFails {
        handler: SmolStr,
        errors: Vec<SmolStr>,
    },
    /// layer の中で同じエフェクトを2つのハンドラが供給する（E0421，8.5）．`first` が先に
    /// 並べたハンドラ．同じハンドラを2回並べたときは `first` と `second` が同じ．
    DuplicateLayerEffect {
        layer: SmolStr,
        effect: SmolStr,
        first: SmolStr,
        second: SmolStr,
    },
    /// layer が互いを含んで循環している（E0422，8.5）．`cycle` は循環をたどる layer の名前
    /// （最初の layer を最後にもう一度入れる）．
    LayerCycle { cycle: Vec<SmolStr> },
    /// 組み込みのエフェクト Async のハンドラを書いた（E0413，8.7）．
    AsyncHandler { handler: SmolStr },
    /// `use Async` を書いた（E0414，8.7）．Async は操作を持たず，供給するのはランタイムだけ．
    UseAsync,
    /// handler の操作の `suspend` の有無が，エフェクトの操作の宣言と違う（E0415，8.1，8.4）．
    /// `suspend` はエフェクトの宣言の側が `suspend fn` か．
    SuspendMismatch {
        op: SmolStr,
        effect: SmolStr,
        suspend: bool,
    },
    /// suspend fn でない操作の実装の本体が，中断しうる呼び出しを含む（E0417，8.1）．`op` は
    /// 操作の実装の名前，`call` はその呼び出しが中断しうる理由．呼び出しごとに出す．
    SuspendingFnOp { op: SmolStr, call: SuspendingCall },
    /// `Immediate` を求める効果の集合に，中断しうるエフェクトか，Immediate と分からない集合が
    /// 入る（E0416，9.5）．`effects` は中断しうるエフェクト（suspend fn を持つか Async），
    /// `sets` は `Immediate` の制約のない集合の型引数の名前．`required` は何が求めたか．
    NotImmediate {
        effects: Vec<SmolStr>,
        sets: Vec<SmolStr>,
        required: ImmediateSource,
    },
    /// `Immediate` を，効果の集合でない型引数に付けた（E0322，9.5）．
    ImmediateOnNonEffect { param: SmolStr },
    /// 集合でない型引数の位置に集合（`{ Db }`，`A | B`）を書いた（E0324，9.2）．
    SetAsType,
    /// 集合でない型引数の位置にエフェクトの名前を書いた（E0221，8.2）．名前解決は型引数の位置の
    /// エフェクトの名前を通すので，型推論が確かめる．
    EffectAsType { name: SmolStr },
    /// 型引数のあるエフェクトの型引数を書いていない（E0423，8.1）．`use State` と，`implements State`．
    /// `handler` は implements に書いたときのハンドラの名前．`params` はエフェクトの型引数の名前．
    MissingEffectTypeArgs {
        effect: SmolStr,
        params: Vec<SmolStr>,
        handler: Option<SmolStr>,
    },
    /// エフェクトの型引数を集合として使った（E0424，8.1）．エフェクトの型引数はふつうの型だけ．
    EffectParamNotType { effect: SmolStr, param: SmolStr },
    /// 名前付き型の集合の型引数（`Handler[E, R]` の `E`）に，その種類のタグか集合の型引数でない
    /// ものを書いた（E0323，9.2）．`effects` は効果の集合の位置か．`found` は書いたものの説明．
    SetArgMismatch {
        param: SmolStr,
        ty: SmolStr,
        effects: bool,
        found: SmolStr,
    },
}

/// fn の操作の実装の本体の呼び出しが中断しうる理由（[`DiagnosticKind::SuspendingFnOp`]，8.1）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuspendingCall {
    /// suspend fn の操作の呼び出し．`effect` はエフェクトの名前，`op` は操作の名前．
    Op { effect: SmolStr, op: SmolStr },
    /// `@external suspend fn` の呼び出し．
    External { callee: SmolStr },
    /// 型の R が Immediate でない関数か関数の値の呼び出し．`callee` は名前で呼んだ関数か局所変数の名前，
    /// `effects` は R の中断しうるエフェクト（suspend fn を持つか Async）．
    Fn {
        callee: Option<SmolStr>,
        effects: Vec<SmolStr>,
    },
}

/// `Immediate` を求めたもの（[`DiagnosticKind::NotImmediate`]）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImmediateSource {
    /// 型引数の制約 `R: Immediate`．`param` は型引数の名前，`callee` はそれを持つ関数．
    Bound { param: SmolStr, callee: SmolStr },
    /// 外部関数に渡すコールバック（9.5）．`callee` は外部関数の名前．
    ExternalCallback { callee: SmolStr },
}

/// R が含まれるべき集合がどこから決まったか（[`DiagnosticKind::UnhandledEffects`]）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectContext {
    /// fn に書いた `use` 節（9.3）．
    Declared,
    /// const の値．効果を持てない（5.6）．
    Const,
    /// main．R は空でなければならない（8.3）．
    Main,
    /// テスト関数．R は空でなければならない（13.3）．`name` は関数の名前．
    Test { name: SmolStr },
}

impl DiagnosticKind {
    /// 警告か．警告でなければエラー．
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            DiagnosticKind::UnreachableArm | DiagnosticKind::UnraisedError { .. }
        )
    }
}
