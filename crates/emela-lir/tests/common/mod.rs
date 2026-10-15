//! A Core module with the definitions the hand-built LIR tests refer to.

#![allow(dead_code)]

use emela_core::{
    CtorRef, EffectDef, EffectId, EnumDef, EnumId, FnType, OpDef, TypeId, VariantDef,
};
use emela_lir::{Const, Operand};

pub struct Fixture {
    pub core: emela_core::Module,
    /// `enum Shape { Circle(Float), Rect(width: Float, height: Float) }`
    pub shape: EnumId,
    pub shape_ty: TypeId,
    /// `enum Color { Red, Green, Blue }`
    pub color: EnumId,
    pub color_ty: TypeId,
    /// `error ParseError(input: String)`
    pub parse_error: EnumId,
    pub parse_error_ty: TypeId,
    /// `effect Console { print_line, print_error_line }`
    pub console: EffectId,
    pub console_ty: TypeId,
    pub ints: TypeId,
    pub pair: TypeId,
    /// `fn(String) -> ()`
    pub string_to_unit: TypeId,
    /// `fn() -> Int`
    pub thunk: TypeId,
    /// `fn(Int) -> Int fails`
    pub int_to_int_fails: TypeId,
    /// `fn(Int, ()) -> Int ev`
    pub with_unit_ev: TypeId,
    /// `Option[Int]`
    pub option_int: TypeId,
    pub some: CtorRef,
    pub none: CtorRef,
}

impl Fixture {
    pub fn new() -> Self {
        let mut core = emela_core::Module::new();
        let shape = core.add_enum(EnumDef {
            name: "Shape".into(),
            params: 0,
            variants: vec![
                VariantDef::positional("Circle", vec![TypeId::FLOAT]),
                VariantDef::named(
                    "Rect",
                    vec![("width", TypeId::FLOAT), ("height", TypeId::FLOAT)],
                ),
            ],
            error: false,
        });
        let color = core.add_enum(EnumDef {
            name: "Color".into(),
            params: 0,
            variants: vec![
                VariantDef::unit("Red"),
                VariantDef::unit("Green"),
                VariantDef::unit("Blue"),
            ],
            error: false,
        });
        let parse_error = core.add_enum(EnumDef {
            name: "ParseError".into(),
            params: 0,
            variants: vec![VariantDef::named(
                "ParseError",
                vec![("input", TypeId::STRING)],
            )],
            error: true,
        });
        let console = core.add_effect(EffectDef {
            name: "Console".into(),
            ops: vec![OpDef::new("print_line"), OpDef::new("print_error_line")],
        });
        let shape_ty = core.enum_type(shape, vec![]);
        let color_ty = core.enum_type(color, vec![]);
        let parse_error_ty = core.enum_type(parse_error, vec![]);
        let console_ty = core.handler(console);
        let ints = core.list(TypeId::INT);
        let pair = core.tuple(vec![TypeId::INT, TypeId::STRING]);
        let string_to_unit = core.func(FnType {
            params: vec![TypeId::STRING],
            ret: TypeId::UNIT,
            fails: false,
            suspends: false,
            ev: false,
        });
        let thunk = core.func(FnType {
            params: vec![],
            ret: TypeId::INT,
            fails: false,
            suspends: false,
            ev: false,
        });
        let int_to_int_fails = core.func(FnType {
            params: vec![TypeId::INT],
            ret: TypeId::INT,
            fails: true,
            suspends: false,
            ev: false,
        });
        let with_unit_ev = core.func(FnType {
            params: vec![TypeId::INT, TypeId::UNIT],
            ret: TypeId::INT,
            fails: false,
            suspends: false,
            ev: true,
        });
        let some = core.some_ctor();
        let none = core.none_ctor();
        let option_int = core.enum_type(some.enum_id, vec![TypeId::INT]);
        Fixture {
            core,
            shape,
            shape_ty,
            color,
            color_ty,
            parse_error,
            parse_error_ty,
            console,
            console_ty,
            ints,
            pair,
            string_to_unit,
            thunk,
            int_to_int_fails,
            with_unit_ev,
            option_int,
            some,
            none,
        }
    }

    pub fn ctor(&self, enum_id: EnumId, variant: usize) -> CtorRef {
        CtorRef { enum_id, variant }
    }
}

pub fn int(n: i32) -> Operand {
    Operand::Const(Const::Int(n))
}

pub fn string(s: &str) -> Operand {
    Operand::Const(Const::String(s.to_owned()))
}
