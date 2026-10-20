//! The builtin function table.
//!
//! Holds core functions that cannot be written in Emela (string operations, numeric conversion,
//! showing) and the Prelude functions, as functions whose names and types the compiler knows.
//! Name resolution and type inference look them up via `Builtin::lookup` and
//! `BuiltinInfo::scheme`; each backend maps `Builtin` to its own implementation.
//!
//! Functions writable in Emela, such as `List.map`, do not belong here (core is written in
//! Emela). Each row has exactly one `@intrinsic` declaration in the bundled sources (`lib/*.emel`),
//! and `Builtin::lookup_intrinsic` finds the row from the declaration.

use emela_types::{Scheme, Ty, TyConId, TyParam};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    // String (spec 16.2).
    /// `String.length`: the number of grapheme clusters.
    StringLength,
    /// `String.byte_size`: the number of bytes in UTF-8.
    StringByteSize,
    /// `String.append(a, b)`．
    StringAppend,
    StringContains,
    StringStartsWith,
    StringEndsWith,
    /// `String.split(s, sep)`. An empty separator splits into graphemes.
    StringSplit,
    /// `String.chars`: one string per grapheme cluster.
    StringChars,
    /// `String.join(parts, sep)`．
    StringJoin,
    /// `String.trim`: removes leading and trailing Unicode White_Space.
    StringTrim,
    /// `String.code_points`.
    StringCodePoints,
    /// `String.from_code_point`. `None` if not a Unicode scalar value.
    StringFromCodePoint,
    /// `String.show_nested` (`@internal`): the Show of a value inside another value (spec 10.6),
    /// for the Show impls in core. Monomorphization replaces it with [`Builtin::ShowString`] for a
    /// String and with a call of `Show.show` for any other type, so backends never see it.
    ShowNested,

    // Int.
    IntToString,
    /// The body of `Int.parse`. Reads only decimal digits with an optional leading sign; anything
    /// else, or out of range, is `None`.
    IntParse,
    /// `Int.checked_add`. `None` on overflow.
    IntCheckedAdd,
    IntBitAnd,
    IntBitOr,
    IntBitXor,
    IntBitNot,
    /// `Int.shift_left(n, by)`. See `NOTES.md` for how shift amounts are handled.
    IntShiftLeft,
    /// `Int.shift_right(n, by)`. Keeps the sign (arithmetic shift).
    IntShiftRight,
    /// `Int.shift_right_unsigned(n, by)`. Fills the high bits with 0 (logical shift).
    IntShiftRightUnsigned,

    // Int64.
    Int64FromInt,
    Int64ToString,
    /// `Int64.to_int`. Keeps the low 32 bits.
    Int64ToInt,
    Int64BitAnd,
    Int64BitOr,
    Int64BitXor,
    Int64BitNot,
    Int64ShiftLeft,
    Int64ShiftRight,
    Int64ShiftRightUnsigned,

    // Float. Converting to Int is a defect if the result is NaN or outside the range of Int.
    FloatToString,
    /// `Float.from_int`. Every Int is exactly representable as a Float.
    FloatFromInt,
    /// The body of `Float.parse`. Reads only the forms `to_string` writes; anything else is
    /// `None`.
    FloatParse,
    FloatFloor,
    FloatCeil,
    /// Ties round away from zero.
    FloatRound,
    FloatTruncate,

    // Prelude (spec 15.1).
    /// `panic(message)`.
    Panic,
    /// `todo()`. Raises a `not yet implemented` defect.
    Todo,
    /// `dbg(value)`. Prints the debug display of the value to stderr on one line and returns the
    /// value (spec 10.6). Takes no trait; what reaches a backend is shown by its structure
    /// ([`crate::mono`] rewrites the rest).
    Dbg,
    /// `inspect(value)`: the debug display of a value with Show (spec 10.6), a String quoted.
    /// What reaches a backend is shown by its structure, which is its Show.
    Inspect,

    // Internal builtins used only by lowering. Not in `ALL`, so sources cannot call them.
    /// Makes a cell holding one value (the state of `State.run`, spec 8.8).
    CellNew,
    CellGet,
    /// Replaces the cell's contents. The value is `()`.
    CellSet,
    /// A String shown inside another value: quoted and escaped (spec 10.6). What
    /// [`Builtin::ShowNested`] becomes for a String, and what derived Show uses for a String field.
    ShowString,
    /// Prints the String as it is on one line to stderr. What `dbg` becomes for a value shown
    /// through its Show impl (spec 10.6). The value is `()`.
    DebugPrint,
}

/// `@intrinsic` declarations (module name, function name) that lowering maps as runtime handlers.
/// They are not in the builtin table: `State.run` calls its body and extends the evidence table,
/// so it is not a single function's implementation.
pub const RUNTIME_HANDLERS: &[(&str, &str)] = &[("Std.State", "run")];

pub fn is_runtime_handler(module: &str, name: &str) -> bool {
    RUNTIME_HANDLERS.contains(&(module, name))
}

/// A simple representation of builtin types, converted to the type checker's `Ty` by `Sig::to_ty`.
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
    Option(&'static Sig),
    /// A type parameter of the function; becomes `Ty::Bound(i)`.
    Param(u32),
}

impl Sig {
    /// `option` is the `TyConId` of Prelude's `Option`.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinInfo {
    /// The module name used to qualify. `None` for Prelude functions (called unqualified).
    pub module: Option<&'static str>,
    pub name: &'static str,
    /// The number of type parameters, referred to by `Sig::Param(i)` in `params` and `ret`.
    pub ty_params: u32,
    pub params: &'static [Sig],
    pub ret: Sig,
    /// The Prelude only holds pure functions (spec 15.1), so every row is `true` today. `dbg`
    /// writes to stderr but counts as pure as a debugging exception.
    pub pure: bool,
}

impl BuiltinInfo {
    /// A qualified name like `String.length`; just the name for Prelude functions.
    pub fn path(&self) -> String {
        match self.module {
            Some(m) => format!("{m}.{}", self.name),
            None => self.name.to_owned(),
        }
    }

    /// The type scheme for type inference, with type parameters named `A`, `B`, ….
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
) -> BuiltinInfo {
    BuiltinInfo {
        module,
        name,
        ty_params: 0,
        params,
        ret,
        pure: true,
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
        Builtin::StringAppend,
        Builtin::StringContains,
        Builtin::StringStartsWith,
        Builtin::StringEndsWith,
        Builtin::StringSplit,
        Builtin::StringChars,
        Builtin::StringJoin,
        Builtin::StringTrim,
        Builtin::StringCodePoints,
        Builtin::StringFromCodePoint,
        Builtin::ShowNested,
        Builtin::IntToString,
        Builtin::IntParse,
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
        Builtin::FloatFromInt,
        Builtin::FloatParse,
        Builtin::FloatFloor,
        Builtin::FloatCeil,
        Builtin::FloatRound,
        Builtin::FloatTruncate,
        Builtin::Panic,
        Builtin::Todo,
        Builtin::Dbg,
        Builtin::Inspect,
    ];

    pub const fn info(self) -> BuiltinInfo {
        use Sig::*;
        const STRS: Sig = List(&String);
        match self {
            Builtin::StringLength => info(S, "length", &[String], Int),
            Builtin::StringByteSize => info(S, "byte_size", &[String], Int),
            Builtin::StringAppend => info(S, "append", &[String, String], String),
            Builtin::StringContains => info(S, "contains", &[String, String], Bool),
            Builtin::StringStartsWith => info(S, "starts_with", &[String, String], Bool),
            Builtin::StringEndsWith => info(S, "ends_with", &[String, String], Bool),
            Builtin::StringSplit => info(S, "split", &[String, String], STRS),
            Builtin::StringChars => info(S, "chars", &[String], STRS),
            Builtin::StringJoin => info(S, "join", &[STRS, String], String),
            Builtin::StringTrim => info(S, "trim", &[String], String),
            Builtin::StringCodePoints => info(S, "code_points", &[String], List(&Int)),
            Builtin::StringFromCodePoint => info(S, "from_code_point", &[Int], Option(&String)),
            Builtin::ShowNested => BuiltinInfo {
                ty_params: 1,
                ..info(S, "show_nested", &[Param(0)], String)
            },
            Builtin::IntToString => info(I, "to_string", &[Int], String),
            Builtin::IntParse => info(I, "parse_or_none", &[String], Option(&Int)),
            Builtin::IntCheckedAdd => info(I, "checked_add", &[Int, Int], Option(&Int)),
            Builtin::IntBitAnd => info(I, "bit_and", &[Int, Int], Int),
            Builtin::IntBitOr => info(I, "bit_or", &[Int, Int], Int),
            Builtin::IntBitXor => info(I, "bit_xor", &[Int, Int], Int),
            Builtin::IntBitNot => info(I, "bit_not", &[Int], Int),
            Builtin::IntShiftLeft => info(I, "shift_left", &[Int, Int], Int),
            Builtin::IntShiftRight => info(I, "shift_right", &[Int, Int], Int),
            Builtin::IntShiftRightUnsigned => info(I, "shift_right_unsigned", &[Int, Int], Int),
            Builtin::Int64FromInt => info(L, "from_int", &[Int], Int64),
            Builtin::Int64ToString => info(L, "to_string", &[Int64], String),
            Builtin::Int64ToInt => info(L, "to_int", &[Int64], Int),
            Builtin::Int64BitAnd => info(L, "bit_and", &[Int64, Int64], Int64),
            Builtin::Int64BitOr => info(L, "bit_or", &[Int64, Int64], Int64),
            Builtin::Int64BitXor => info(L, "bit_xor", &[Int64, Int64], Int64),
            Builtin::Int64BitNot => info(L, "bit_not", &[Int64], Int64),
            Builtin::Int64ShiftLeft => info(L, "shift_left", &[Int64, Int], Int64),
            Builtin::Int64ShiftRight => info(L, "shift_right", &[Int64, Int], Int64),
            Builtin::Int64ShiftRightUnsigned => {
                info(L, "shift_right_unsigned", &[Int64, Int], Int64)
            }
            Builtin::FloatToString => info(F, "to_string", &[Float], String),
            Builtin::FloatFromInt => info(F, "from_int", &[Int], Float),
            Builtin::FloatParse => info(F, "parse_or_none", &[String], Option(&Float)),
            Builtin::FloatFloor => info(F, "floor", &[Float], Int),
            Builtin::FloatCeil => info(F, "ceil", &[Float], Int),
            Builtin::FloatRound => info(F, "round", &[Float], Int),
            Builtin::FloatTruncate => info(F, "truncate", &[Float], Int),
            Builtin::Panic => info(None, "panic", &[String], Never),
            Builtin::Todo => info(None, "todo", &[], Never),
            Builtin::Dbg => BuiltinInfo {
                ty_params: 1,
                ..info(None, "dbg", &[Param(0)], Param(0))
            },
            Builtin::Inspect => BuiltinInfo {
                ty_params: 1,
                ..info(None, "inspect", &[Param(0)], String)
            },
            // A cell's type cannot be written as a `Sig`, so the content type `A` stands in for
            // `Cell[A]` (it is never passed to type inference). The type argument is the content
            // type: `new(A) -> Cell[A]`, `get(Cell[A]) -> A`, `set(Cell[A], A) -> ()`.
            Builtin::CellNew => BuiltinInfo {
                pure: false,
                ty_params: 1,
                ..info(Some("$Cell"), "new", &[Param(0)], Param(0))
            },
            Builtin::CellGet => BuiltinInfo {
                pure: false,
                ty_params: 1,
                ..info(Some("$Cell"), "get", &[Param(0)], Param(0))
            },
            Builtin::CellSet => BuiltinInfo {
                pure: false,
                ty_params: 1,
                ..info(Some("$Cell"), "set", &[Param(0), Param(0)], Unit)
            },
            Builtin::ShowString => info(Some("$Show"), "string", &[String], String),
            Builtin::DebugPrint => BuiltinInfo {
                pure: false,
                ..info(Some("$Debug"), "print", &[String], Unit)
            },
        }
    }

    pub fn arity(self) -> usize {
        self.info().params.len()
    }

    /// Prelude functions use `None` for `module`.
    pub fn lookup(module: Option<&str>, name: &str) -> Option<Builtin> {
        Builtin::ALL.iter().copied().find(|b| {
            let i = b.info();
            i.module == module && i.name == name
        })
    }

    /// Looks up from an `@intrinsic` declaration in the bundled sources. `module` is a module name
    /// from `crate::core_sources`; declarations in the Prelude source (`crate::PRELUDE`) match rows
    /// whose `module` is `None`.
    pub fn lookup_intrinsic(module: &str, name: &str) -> Option<Builtin> {
        let module = (module != crate::PRELUDE).then_some(module);
        Builtin::lookup(module, name)
    }
}
