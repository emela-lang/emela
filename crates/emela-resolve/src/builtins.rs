//! Prelude（15.1）と，組み込みのモジュールを外から差し込む口．
//!
//! Prelude の型，構成子，Trait，関数は [`crate::hir::Program`] の定義として最初に作る．
//! `Bool`，`Option`，`Ordering` は普通の enum として，バリアントと型引数を持つ．
//!
//! 組み込みのモジュール（`String`，`List` など）の中身は emela-core の組み込み関数の表が持つ．
//! resolve は core に依存できないので，[`BuiltinModules`] で引く．

use indexmap::IndexMap;
use la_arena::Idx;
use smol_str::SmolStr;

use crate::hir::{
    CtorItem, DefData, DefId, DefKind, DefModule, EffectItem, EnumItem, Fields, Item, Program, Res,
    TraitItem, TypeKind, TypeParam, TypeRef,
};

/// 組み込みのモジュールの項目の種類．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinKind {
    /// 関数．`List.map` のように修飾して呼ぶ．
    Fn,
    /// 型．`Time.Instant` のように修飾して書く．
    Type,
}

/// 組み込みのモジュールの表．名前で引く．
///
/// モジュールは import なしで `.` の左に書ける（`String.contains(s, "@")`，4.4 の例）．
pub trait BuiltinModules {
    /// モジュールの名前の一覧．
    fn modules(&self) -> Vec<SmolStr>;
    /// モジュールの項目を名前で引く．
    fn lookup(&self, module: &str, name: &str) -> Option<BuiltinKind>;
    /// モジュールの項目の名前の一覧．「もしかして」の候補に使う．
    fn names(&self, module: &str) -> Vec<SmolStr> {
        let _ = module;
        Vec::new()
    }
}

/// 組み込みのモジュールがない．
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

/// 表で持つ組み込みのモジュール．テストと，core の表が入るまでの仮の表に使う．
#[derive(Debug, Default, Clone)]
pub struct BuiltinTable {
    modules: IndexMap<SmolStr, IndexMap<SmolStr, BuiltinKind>>,
}

impl BuiltinTable {
    pub fn new() -> Self {
        BuiltinTable::default()
    }

    /// モジュールを足す．同じ名前なら項目を足す．
    pub fn module(mut self, name: &str, items: &[(&str, BuiltinKind)]) -> Self {
        let module = self.modules.entry(name.into()).or_default();
        for &(item, kind) in items {
            module.insert(item.into(), kind);
        }
        self
    }

    /// 関数だけのモジュールを足す．
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

/// 15.1 の型のうち，enum でないもの．
const PRELUDE_TYPES: &[&str] = &["Int", "Int64", "Float", "String", "Never", "List"];
/// 15.1 の Trait と，組み込みの制約 `Immediate`（9.5，10.6）．関数の名前を添える．
/// 関数のシグネチャは型推論の段が持つ．
const PRELUDE_TRAITS: &[(&str, &[&str])] = &[
    ("Eq", &["eq"]),
    ("Ord", &["compare"]),
    ("Show", &["show"]),
    ("Hash", &["hash"]),
    ("Immediate", &[]),
];
/// 15.1 の関数．
const PRELUDE_FNS: &[&str] = &["panic", "todo", "dbg"];
/// 組み込みのエフェクト（8.7）．
const PRELUDE_EFFECTS: &[&str] = &["Async"];

/// Prelude の定義を作る．`Program` が空のときに1度だけ呼ぶ．
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
        program
            .items
            .insert(def, Item::Effect(EffectItem { ops: Vec::new() }));
    }
    for name in PRELUDE_FNS {
        add(program, name, DefKind::Fn, None);
    }
    program.prelude.defs = defs;
}

/// enum の中身と，中身のないバリアントの構成子を登録する．
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
