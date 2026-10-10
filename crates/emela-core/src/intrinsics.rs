//! 組み込み関数の表．
//!
//! Emela で書けない core の関数（文字列の操作，数値の変換，表示）と Prelude の関数を，
//! コンパイラが名前と型を知っている関数として持つ．名前解決と型推論は `Builtin::lookup` と
//! `BuiltinInfo::scheme` でこの表を引き，バックエンドは `BuiltinInfo::js` などで実装を選ぶ．
//!
//! `List.map` のように Emela で書ける関数はここに入れない（core を Emela で書く）．
//! 各行は同梱のソース（`lib/*.emel`）に `@intrinsic` の宣言を1つずつ持ち，
//! `Builtin::lookup_intrinsic` で宣言から行を引く．

use emela_types::{Scheme, Ty, TyConId, TyParam};

/// 組み込み関数．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    // String（16.2）．
    /// `String.length`．書記素クラスタの数．
    StringLength,
    /// `String.byte_size`．UTF-8 で符号化したときのバイト数．
    StringByteSize,
    StringConcat,
    StringContains,
    StringStartsWith,
    StringEndsWith,
    /// `String.split(s, sep)`．空の区切りでは書記素ごとに分ける．
    StringSplit,
    /// `String.chars`．書記素クラスタごとの文字列のリスト．
    StringChars,
    /// `String.join(parts, sep)`．
    StringJoin,
    /// `String.trim`．前後の Unicode の White_Space を取り除く．
    StringTrim,
    StringFromInt,
    /// `String.code_points`．コードポイントの値のリスト．
    StringCodePoints,
    /// `String.from_code_point`．Unicode のスカラー値でなければ `None`．
    StringFromCodePoint,

    // Int．
    IntToString,
    IntToFloat,
    IntToInt64,
    /// `Int.checked_add`．あふれたら `None`．
    IntCheckedAdd,
    IntBitAnd,
    IntBitOr,
    IntBitXor,
    IntBitNot,
    /// `Int.shift_left(n, by)`．シフト量の扱いは `NOTES.md`．
    IntShiftLeft,
    /// `Int.shift_right(n, by)`．符号を保つ（算術シフト）．
    IntShiftRight,
    /// `Int.shift_right_unsigned(n, by)`．上位を 0 で埋める（論理シフト）．
    IntShiftRightUnsigned,

    // Int64．
    Int64FromInt,
    Int64ToString,
    /// `Int64.to_int`．下位 32bit に巻き戻す．
    Int64ToInt,
    Int64BitAnd,
    Int64BitOr,
    Int64BitXor,
    Int64BitNot,
    Int64ShiftLeft,
    Int64ShiftRight,
    Int64ShiftRightUnsigned,

    // Float．Int への変換は，結果が Int の範囲外か NaN なら defect．
    FloatToString,
    FloatFloor,
    FloatCeil,
    /// 0.5 は 0 から遠い方へ丸める．
    FloatRound,
    FloatTruncate,

    // Prelude（15.1）．
    /// `panic(message)`．defect を起こす．
    Panic,
    /// `todo()`．`not yet implemented` の defect を起こす．
    Todo,
    /// `dbg(value)`．値の表示を標準エラーに1行出して，値をそのまま返す．
    Dbg,
}

/// 組み込み関数の型の簡単な表現．`Sig::to_ty` で型検査の `Ty` に変換する．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sig {
    Int,
    Int64,
    Float,
    Bool,
    String,
    Unit,
    Never,
    List(&'static Sig),
    /// Prelude の `Option`．
    Option(&'static Sig),
    /// 関数の型引数．`Ty::Bound(i)` になる．
    Param(u32),
}

impl Sig {
    /// 型検査の型にする．`option` は Prelude の `Option` の `TyConId`．
    pub fn to_ty(self, option: TyConId) -> Ty {
        match self {
            Sig::Int => Ty::INT,
            Sig::Int64 => Ty::INT64,
            Sig::Float => Ty::FLOAT,
            Sig::Bool => Ty::BOOL,
            Sig::String => Ty::STRING,
            Sig::Unit => Ty::UNIT,
            Sig::Never => Ty::Never,
            Sig::List(t) => Ty::list(t.to_ty(option)),
            Sig::Option(t) => Ty::named(option, [t.to_ty(option)]),
            Sig::Param(i) => Ty::Bound(i),
        }
    }
}

/// 表の1行．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinInfo {
    /// 修飾に使うモジュール名．Prelude の関数は `None`（修飾せずに呼ぶ）．
    pub module: Option<&'static str>,
    pub name: &'static str,
    /// 型引数の数．`params` と `ret` の `Sig::Param(i)` が指す．
    pub ty_params: u32,
    pub params: &'static [Sig],
    pub ret: Sig,
    /// 純粋か．Prelude には純粋なものしか入れない（15.1）ので，今の表はすべて `true`．
    /// `dbg` は標準エラーに書くが，デバッグ用の例外として純粋に数える．
    pub pure: bool,
    /// JS ランタイム（`emela-codegen-js` の `runtime.mjs`）での実装名．
    pub js: &'static str,
}

impl BuiltinInfo {
    /// `String.length` のような修飾した名前．Prelude の関数は名前だけ．
    pub fn path(&self) -> String {
        match self.module {
            Some(m) => format!("{m}.{}", self.name),
            None => self.name.to_owned(),
        }
    }

    /// 型推論に渡す型スキーム．型引数は `A`，`B`，… と名付ける．
    pub fn scheme(&self, option: TyConId) -> Scheme {
        let params = (0..self.ty_params)
            .map(|i| TyParam::new(char::from(b'A' + i as u8).to_string()))
            .collect();
        let ty = Ty::func(
            self.params.iter().map(|p| p.to_ty(option)),
            self.ret.to_ty(option),
        );
        Scheme::new(params, ty)
    }
}

const fn info(
    module: Option<&'static str>,
    name: &'static str,
    params: &'static [Sig],
    ret: Sig,
    js: &'static str,
) -> BuiltinInfo {
    BuiltinInfo {
        module,
        name,
        ty_params: 0,
        params,
        ret,
        pure: true,
        js,
    }
}

const S: Option<&str> = Some("String");
const I: Option<&str> = Some("Int");
const L: Option<&str> = Some("Int64");
const F: Option<&str> = Some("Float");

impl Builtin {
    pub const ALL: &'static [Builtin] = &[
        Builtin::StringLength,
        Builtin::StringByteSize,
        Builtin::StringConcat,
        Builtin::StringContains,
        Builtin::StringStartsWith,
        Builtin::StringEndsWith,
        Builtin::StringSplit,
        Builtin::StringChars,
        Builtin::StringJoin,
        Builtin::StringTrim,
        Builtin::StringFromInt,
        Builtin::StringCodePoints,
        Builtin::StringFromCodePoint,
        Builtin::IntToString,
        Builtin::IntToFloat,
        Builtin::IntToInt64,
        Builtin::IntCheckedAdd,
        Builtin::IntBitAnd,
        Builtin::IntBitOr,
        Builtin::IntBitXor,
        Builtin::IntBitNot,
        Builtin::IntShiftLeft,
        Builtin::IntShiftRight,
        Builtin::IntShiftRightUnsigned,
        Builtin::Int64FromInt,
        Builtin::Int64ToString,
        Builtin::Int64ToInt,
        Builtin::Int64BitAnd,
        Builtin::Int64BitOr,
        Builtin::Int64BitXor,
        Builtin::Int64BitNot,
        Builtin::Int64ShiftLeft,
        Builtin::Int64ShiftRight,
        Builtin::Int64ShiftRightUnsigned,
        Builtin::FloatToString,
        Builtin::FloatFloor,
        Builtin::FloatCeil,
        Builtin::FloatRound,
        Builtin::FloatTruncate,
        Builtin::Panic,
        Builtin::Todo,
        Builtin::Dbg,
    ];

    pub const fn info(self) -> BuiltinInfo {
        use Sig::*;
        const STRS: Sig = List(&String);
        match self {
            Builtin::StringLength => info(S, "length", &[String], Int, "$strlen"),
            Builtin::StringByteSize => info(S, "byte_size", &[String], Int, "$strbytes"),
            Builtin::StringConcat => info(S, "concat", &[String, String], String, "$strcat"),
            Builtin::StringContains => info(S, "contains", &[String, String], Bool, "$strhas"),
            Builtin::StringStartsWith => {
                info(S, "starts_with", &[String, String], Bool, "$strstarts")
            }
            Builtin::StringEndsWith => info(S, "ends_with", &[String, String], Bool, "$strends"),
            Builtin::StringSplit => info(S, "split", &[String, String], STRS, "$strsplit"),
            Builtin::StringChars => info(S, "chars", &[String], STRS, "$strchars"),
            Builtin::StringJoin => info(S, "join", &[STRS, String], String, "$strjoin"),
            Builtin::StringTrim => info(S, "trim", &[String], String, "$strtrim"),
            Builtin::StringFromInt => info(S, "from_int", &[Int], String, "$showInt"),
            Builtin::StringCodePoints => {
                info(S, "code_points", &[String], List(&Int), "$strcodepoints")
            }
            Builtin::StringFromCodePoint => info(
                S,
                "from_code_point",
                &[Int],
                Option(&String),
                "$strfromcodepoint",
            ),
            Builtin::IntToString => info(I, "to_string", &[Int], String, "$showInt"),
            Builtin::IntToFloat => info(I, "to_float", &[Int], Float, "$itof"),
            Builtin::IntToInt64 => info(I, "to_int64", &[Int], Int64, "$itol"),
            Builtin::IntCheckedAdd => {
                info(I, "checked_add", &[Int, Int], Option(&Int), "$icheckedAdd")
            }
            Builtin::IntBitAnd => info(I, "bit_and", &[Int, Int], Int, "$iand"),
            Builtin::IntBitOr => info(I, "bit_or", &[Int, Int], Int, "$ior"),
            Builtin::IntBitXor => info(I, "bit_xor", &[Int, Int], Int, "$ixor"),
            Builtin::IntBitNot => info(I, "bit_not", &[Int], Int, "$inot"),
            Builtin::IntShiftLeft => info(I, "shift_left", &[Int, Int], Int, "$ishl"),
            Builtin::IntShiftRight => info(I, "shift_right", &[Int, Int], Int, "$ishr"),
            Builtin::IntShiftRightUnsigned => {
                info(I, "shift_right_unsigned", &[Int, Int], Int, "$iushr")
            }
            Builtin::Int64FromInt => info(L, "from_int", &[Int], Int64, "$itol"),
            Builtin::Int64ToString => info(L, "to_string", &[Int64], String, "$showInt64"),
            Builtin::Int64ToInt => info(L, "to_int", &[Int64], Int, "$ltoi"),
            Builtin::Int64BitAnd => info(L, "bit_and", &[Int64, Int64], Int64, "$land"),
            Builtin::Int64BitOr => info(L, "bit_or", &[Int64, Int64], Int64, "$lor"),
            Builtin::Int64BitXor => info(L, "bit_xor", &[Int64, Int64], Int64, "$lxor"),
            Builtin::Int64BitNot => info(L, "bit_not", &[Int64], Int64, "$lnot"),
            Builtin::Int64ShiftLeft => info(L, "shift_left", &[Int64, Int], Int64, "$lshl"),
            Builtin::Int64ShiftRight => info(L, "shift_right", &[Int64, Int], Int64, "$lshr"),
            Builtin::Int64ShiftRightUnsigned => {
                info(L, "shift_right_unsigned", &[Int64, Int], Int64, "$lushr")
            }
            Builtin::FloatToString => info(F, "to_string", &[Float], String, "$showFloat"),
            Builtin::FloatFloor => info(F, "floor", &[Float], Int, "$ffloor"),
            Builtin::FloatCeil => info(F, "ceil", &[Float], Int, "$fceil"),
            Builtin::FloatRound => info(F, "round", &[Float], Int, "$fround"),
            Builtin::FloatTruncate => info(F, "truncate", &[Float], Int, "$ftrunc"),
            Builtin::Panic => info(None, "panic", &[String], Never, "$panic"),
            Builtin::Todo => info(None, "todo", &[], Never, "$todo"),
            Builtin::Dbg => BuiltinInfo {
                ty_params: 1,
                ..info(None, "dbg", &[Param(0)], Param(0), "$dbg")
            },
        }
    }

    pub fn arity(self) -> usize {
        self.info().params.len()
    }

    /// 名前で引く．Prelude の関数は `module` を `None` にする．
    pub fn lookup(module: Option<&str>, name: &str) -> Option<Builtin> {
        Builtin::ALL.iter().copied().find(|b| {
            let i = b.info();
            i.module == module && i.name == name
        })
    }

    /// 同梱のソースの `@intrinsic` の宣言から引く．`module` は `crate::core_sources` の
    /// モジュール名で，Prelude のソース（`crate::PRELUDE`）の宣言は `module` が `None` の行に当たる．
    pub fn lookup_intrinsic(module: &str, name: &str) -> Option<Builtin> {
        let module = (module != crate::PRELUDE).then_some(module);
        Builtin::lookup(module, name)
    }
}
