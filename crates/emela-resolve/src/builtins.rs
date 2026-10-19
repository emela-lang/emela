//! The Prelude (spec 15.1), and the hook for plugging in built-in modules from outside.
//!
//! Prelude types, constructors, traits and functions are created first, as definitions in
//! [`crate::hir::Program`]. `Bool`, `Option` and `Ordering` are ordinary enums with variants and
//! type parameters.
//!
//! The contents of built-in modules (`String`, `List`, ...) live in emela-core's table of
//! built-in functions. resolve cannot depend on core, so it looks them up through
//! [`BuiltinModules`].

use indexmap::IndexMap;
use la_arena::Idx;
use smol_str::SmolStr;

use crate::hir::{
    CtorItem, DefData, DefId, DefKind, DefModule, EffectItem, EnumItem, Fields, Item, Program, Res,
    TraitItem, TypeKind, TypeParam, TypeRef,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinKind {
    /// Called qualified, as in `List.map`.
    Fn,
    /// Written qualified, as in `Time.Instant`.
    Type,
}

/// Modules can be written left of `.` without an import (`String.contains(s, "@")`, the example
/// in spec 4.4).
pub trait BuiltinModules {
    fn modules(&self) -> Vec<SmolStr>;
    fn lookup(&self, module: &str, name: &str) -> Option<BuiltinKind>;
    /// Used for "did you mean" suggestions.
    fn names(&self, module: &str) -> Vec<SmolStr> {
        let _ = module;
        Vec::new()
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoBuiltins;

impl BuiltinModules for NoBuiltins {
    fn modules(&self) -> Vec<SmolStr> {
        Vec::new()
    }

    fn lookup(&self, _: &str, _: &str) -> Option<BuiltinKind> {
        None
    }
}

/// Used by tests, and as a stand-in until the core table lands.
#[derive(Debug, Default, Clone)]
pub struct BuiltinTable {
    modules: IndexMap<SmolStr, IndexMap<SmolStr, BuiltinKind>>,
}

impl BuiltinTable {
    pub fn new() -> Self {
        BuiltinTable::default()
    }

    /// Adds the items to an existing module of the same name.
    pub fn module(mut self, name: &str, items: &[(&str, BuiltinKind)]) -> Self {
        let module = self.modules.entry(name.into()).or_default();
        for &(item, kind) in items {
            module.insert(item.into(), kind);
        }
        self
    }

    pub fn functions(self, name: &str, fns: &[&str]) -> Self {
        let items: Vec<_> = fns.iter().map(|f| (*f, BuiltinKind::Fn)).collect();
        self.module(name, &items)
    }
}

impl BuiltinModules for BuiltinTable {
    fn modules(&self) -> Vec<SmolStr> {
        self.modules.keys().cloned().collect()
    }

    fn lookup(&self, module: &str, name: &str) -> Option<BuiltinKind> {
        self.modules.get(module)?.get(name).copied()
    }

    fn names(&self, module: &str) -> Vec<SmolStr> {
        self.modules
            .get(module)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// The types of spec 15.1 that are not enums.
const PRELUDE_TYPES: &[&str] = &["Int", "Int64", "Float", "String", "Never", "List"];
/// The traits of spec 15.1 and the built-in constraint `Immediate` (spec 9.5, 10.6), with their
/// function names. The function signatures belong to type inference.
const PRELUDE_TRAITS: &[(&str, &[&str])] = &[
    ("Eq", &["eq"]),
    ("Ord", &["compare"]),
    ("Show", &["show"]),
    ("Immediate", &[]),
];
/// The functions of spec 15.1.
const PRELUDE_FNS: &[&str] = &["panic", "todo", "dbg", "inspect"];
/// Built-in effects (spec 8.7).
const PRELUDE_EFFECTS: &[&str] = &["Async"];

/// Called once, while `Program` is empty.
pub(crate) fn build_prelude(program: &mut Program) {
    let mut defs = Vec::new();
    let mut add = |program: &mut Program, name: &str, kind: DefKind, parent: Option<DefId>| {
        let def = program.defs.alloc(DefData {
            name: name.into(),
            kind,
            module: DefModule::Prelude,
            parent,
            span: None,
            is_pub: true,
            is_opaque: false,
        });
        if parent.is_none() || kind == DefKind::Ctor {
            defs.push(def);
        }
        def
    };

    for name in PRELUDE_TYPES {
        add(program, name, DefKind::BuiltinType, None);
    }

    // enum Bool { True, False }
    let bool_ = add(program, "Bool", DefKind::Enum, None);
    let variants = ["True", "False"]
        .map(|v| add(program, v, DefKind::Ctor, Some(bool_)))
        .to_vec();
    enum_item(program, bool_, Vec::new(), variants);

    // enum Option[A] { Some(A), None }
    let option = add(program, "Option", DefKind::Enum, None);
    let a = add(program, "A", DefKind::TypeParam, Some(option));
    let some = add(program, "Some", DefKind::Ctor, Some(option));
    let none = add(program, "None", DefKind::Ctor, Some(option));
    let a_ty = program.types.alloc(TypeRef {
        kind: TypeKind::Path {
            res: Res::Def(a),
            args: Vec::new(),
        },
        span: None,
    });
    enum_item(
        program,
        option,
        vec![TypeParam {
            def: a,
            bounds: Vec::new(),
        }],
        vec![some, none],
    );
    program.items.insert(
        some,
        Item::Ctor(CtorItem {
            owner: option,
            fields: Fields::Positional(vec![a_ty]),
        }),
    );

    // enum Ordering { Less, Equal, Greater }
    let ordering = add(program, "Ordering", DefKind::Enum, None);
    let variants = ["Less", "Equal", "Greater"]
        .map(|v| add(program, v, DefKind::Ctor, Some(ordering)))
        .to_vec();
    enum_item(program, ordering, Vec::new(), variants);

    for (name, fns) in PRELUDE_TRAITS {
        let def = add(program, name, DefKind::Trait, None);
        let fns = fns
            .iter()
            .map(|f| add(program, f, DefKind::TraitFn, Some(def)))
            .collect();
        program.items.insert(
            def,
            Item::Trait(TraitItem {
                supertraits: Vec::new(),
                fns,
            }),
        );
    }
    for name in PRELUDE_EFFECTS {
        let def = add(program, name, DefKind::Effect, None);
        program.items.insert(
            def,
            Item::Effect(EffectItem {
                type_params: Vec::new(),
                ops: Vec::new(),
                is_intrinsic: false,
                label: None,
            }),
        );
    }
    for name in PRELUDE_FNS {
        add(program, name, DefKind::Fn, None);
    }
    program.prelude.defs = defs;
}

/// Registers the enum's contents and the constructors of its variants without payload.
fn enum_item(
    program: &mut Program,
    def: DefId,
    type_params: Vec<TypeParam>,
    variants: Vec<Idx<DefData>>,
) {
    for &variant in &variants {
        program.items.insert(
            variant,
            Item::Ctor(CtorItem {
                owner: def,
                fields: Fields::None,
            }),
        );
    }
    program.items.insert(
        def,
        Item::Enum(EnumItem {
            type_params,
            variants,
            derives: Vec::new(),
        }),
    );
}
