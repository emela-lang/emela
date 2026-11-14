//! The derived impls of Encode, Decode and ObjectKey (`Std.Dynamic`) and Shaped (`Std.Shape`)
//! (`[trait.derive]`).
//!
//! Unlike the derived Eq, Ord and Show, which monomorphization generates per type
//! (`mono/derive.rs`), these are generic IR functions made here and put in `Module::dispatch`
//! like written impls: `impl Tree[A: Encode]: Encode` becomes a function with one type parameter,
//! and monomorphization copies it per type. Their bodies only construct and take apart values of
//! the type, compare strings, and call functions: the trait functions for the fields
//! (`Encode.to_dynamic[A]`), and functions of std whose names start with `derived_`
//! (`Std.Dynamic.derived_field`). Those take no functions, so no function value, set type
//! argument or evidence table appears here; what each derivation does to a value is written in
//! Emela in std, and only the walk over the fields, the keys and the tags is the compiler's.
//!
//! - Encode of a type is `object` of its fields by key; of an enum, the variant with its tag and
//!   contents (an object, Items, or none).
//! - Decode reads the fields in declaration order. `from_dynamic` stops at the first failure.
//!   `from_dynamic_all` reads each part into an `Attempt`, fails with the failures of all of them
//!   if any failed, and otherwise constructs the value. An enum first reads its tag, and a
//!   variant with positional contents first checks the number of items.
//! - ObjectKey of an enum whose variants have no contents writes and reads the tags; of a type
//!   with one field, it goes through the ObjectKey of that field. The type checker rejects other
//!   forms (E0339).
//! - Shaped is `Ref(name)` if `name` is in `visiting`, and otherwise a Record or Variants whose
//!   fields are read with `name` added to `visiting`: what `Std.Shape.visit` gives.

use emela_resolve::hir::{DefId, DefKind, DefModule, Fields, Item};
use emela_types::Ty;

use super::Lowerer;
use crate::ir::{Arm, BinOp, Callee, CtorRef, Expr, FnId, Lit, Local, OpTy, Pat, Type, TypeId};

const DYNAMIC: &str = "Std.Dynamic";
const SHAPE: &str = "Std.Shape";

/// A constructor of a deriving type: its fields with their types (the type parameters of the
/// type as `Type::Param`) and keys, and its tag.
struct Ctor {
    ctor: CtorRef,
    tag: String,
    fields: Vec<(String, TypeId)>,
    /// Named fields (or a type's), as opposed to positional contents.
    named: bool,
}

/// The type a derived impl is for.
struct Deriving {
    /// `Self`, with the type's parameters as `Type::Param`.
    ty: TypeId,
    /// The name of the type (`Tree`).
    name: String,
    /// The number of type parameters.
    params: usize,
    /// An enum, as opposed to a type or an error (a type with one constructor).
    is_enum: bool,
    ctors: Vec<Ctor>,
}

impl Lowerer<'_> {
    /// The derived impls of the trait function `def` of the derivable trait `trait_def` of std
    /// (`Encode.to_dynamic` and the others), as `Dispatch::impls` entries. Empty for a trait
    /// function the derivations leave to its default (`Shaped.shape`) and for other traits.
    pub(super) fn std_derived_impls(
        &mut self,
        def: DefId,
        trait_def: DefId,
    ) -> Vec<(TypeId, FnId)> {
        let hir = self.hir;
        let types = self.types;
        let Some(trait_name) = emela_types::derivable(hir, trait_def) else {
            return Vec::new();
        };
        let fn_name = hir.defs[def].name.clone();
        let op = match (trait_name, fn_name.as_str()) {
            ("Encode", "to_dynamic") => Op::Encode,
            ("Decode", "from_dynamic") => Op::Decode,
            ("Decode", "from_dynamic_all") => Op::DecodeAll,
            ("ObjectKey", "to_key") => Op::ToKey,
            ("ObjectKey", "from_key") => Op::FromKey,
            ("Shaped", "shape_in") => Op::Shape,
            _ => return Vec::new(),
        };
        let mut out = Vec::new();
        let mut tys = types.traits.derived_of(trait_def);
        tys.sort_by_key(|d| d.into_raw());
        for ty_def in tys {
            let Some(deriving) = self.deriving(trait_def, ty_def) else {
                continue;
            };
            let f = self.derived_fn(op, def, trait_name, &fn_name, &deriving);
            out.push((deriving.ty, f));
        }
        out
    }

    /// The type `ty_def` with its constructors, keys and tags.
    fn deriving(&mut self, trait_def: DefId, ty_def: DefId) -> Option<Deriving> {
        let hir = self.hir;
        let types = self.types;
        let head = &types.traits.derived(trait_def, ty_def)?.head;
        let ty = self.ty(&head.self_ty);
        let (ctors, tags, is_enum): (Vec<DefId>, Vec<Option<String>>, bool) =
            match hir.item(ty_def)? {
                Item::Type(t) => (t.ctor.into_iter().collect(), Vec::new(), false),
                Item::Enum(e) => (
                    e.variants.clone(),
                    e.tags
                        .iter()
                        .map(|t| t.as_ref().map(|t| t.to_string()))
                        .collect(),
                    true,
                ),
                Item::Error(_) => (vec![ty_def], Vec::new(), false),
                _ => return None,
            };
        let mut out = Vec::with_capacity(ctors.len());
        for (i, ctor) in ctors.into_iter().enumerate() {
            let fields = match hir.item(ctor) {
                Some(Item::Ctor(c)) => &c.fields,
                Some(Item::Error(e)) => &e.fields,
                _ => &Fields::None,
            };
            let field_tys: Vec<TypeId> = match types.sigs.get(ctor).map(|s| &s.scheme.ty) {
                Some(Ty::Fn(f)) => f.params.iter().map(|t| self.ty(t)).collect(),
                _ => Vec::new(),
            };
            let (named, keys): (bool, Vec<String>) = match fields {
                Fields::None => (true, Vec::new()),
                Fields::Named(fs) => (
                    true,
                    fs.iter()
                        .map(|f| f.key.as_ref().unwrap_or(&f.name).to_string())
                        .collect(),
                ),
                Fields::Positional(ts) => (false, (0..ts.len()).map(|i| i.to_string()).collect()),
            };
            let tag = tags
                .get(i)
                .cloned()
                .flatten()
                .unwrap_or_else(|| hir.defs[ctor].name.to_string());
            out.push(Ctor {
                ctor: *self.ctors.get(ctor)?,
                tag,
                fields: keys.into_iter().zip(field_tys).collect(),
                named: named || !is_enum,
            });
        }
        Some(Deriving {
            ty,
            name: hir.defs[ty_def].name.to_string(),
            params: head.params.len(),
            is_enum,
            ctors: out,
        })
    }

    /// Declares and defines the derived impl function of `op` for `d`.
    fn derived_fn(
        &mut self,
        op: Op,
        trait_fn: DefId,
        trait_name: &str,
        fn_name: &str,
        d: &Deriving,
    ) -> FnId {
        let base = format!("{}: {trait_name}.{fn_name}", d.name);
        let mut name = base.clone();
        let mut i = 2;
        while self.module.functions.iter().any(|(_, f)| f.name == name) {
            name = format!("{base}#{i}");
            i += 1;
        }
        let f = self.module.declare(&name, false);
        let dynamic = self.std_type(DYNAMIC, "Dynamic", Vec::new());
        let (params, ret, body) = match op {
            Op::Encode => {
                let x = self.module.local("self", d.ty);
                let body = self.encode(trait_fn, d, x);
                (vec![x], dynamic, body)
            }
            Op::Decode | Op::DecodeAll => {
                let v = self.module.local("d", dynamic);
                let body = if op == Op::Decode {
                    self.decode(d, v)
                } else {
                    self.decode_all(d, v)
                };
                (vec![v], d.ty, body)
            }
            Op::ToKey => {
                let x = self.module.local("self", d.ty);
                let body = self.derive_to_key(trait_fn, d, x);
                (vec![x], TypeId::STRING, body)
            }
            Op::FromKey => {
                let k = self.module.local("key", TypeId::STRING);
                let body = self.derive_from_key(trait_fn, d, k);
                (vec![k], d.ty, body)
            }
            Op::Shape => {
                let option = self.module.option_enum();
                let proxy_ty = self.module.intern(Type::Enum(option, vec![d.ty]));
                let strings = self.module.list(TypeId::STRING);
                let proxy = self.module.local("proxy", proxy_ty);
                let visiting = self.module.local("visiting", strings);
                let body = self.shape(trait_fn, d, visiting);
                let shape = self.std_type(SHAPE, "Shape", Vec::new());
                (vec![proxy, visiting], shape, body)
            }
        };
        self.module.define(f, params, ret, body);
        let func = &mut self.module.functions[f];
        func.type_params = d.params;
        func.fails = matches!(op, Op::Decode | Op::DecodeAll | Op::FromKey);
        f
    }

    // ---- Encode ----

    fn encode(&mut self, to_dynamic: DefId, d: &Deriving, x: Local) -> Expr {
        let dynamic = self.std_type(DYNAMIC, "Dynamic", Vec::new());
        let pair = self.module.tuple(vec![TypeId::STRING, dynamic]);
        self.match_ctors(d, x, |cx, c, xs| {
            let values: Vec<Expr> = c
                .fields
                .iter()
                .zip(xs)
                .map(|((_, t), &x)| cx.trait_call(to_dynamic, *t, vec![Expr::Var(x)]))
                .collect();
            let contents = if c.named {
                let pairs = c
                    .fields
                    .iter()
                    .zip(values)
                    .map(|((key, _), v)| Expr::Tuple(vec![string(key), v]))
                    .collect();
                let list = Expr::List {
                    elem: pair,
                    elems: pairs,
                    tail: None,
                };
                cx.std_call(DYNAMIC, "object", Vec::new(), vec![list])
            } else {
                let list = Expr::List {
                    elem: dynamic,
                    elems: values,
                    tail: None,
                };
                cx.std_call(DYNAMIC, "derived_items", Vec::new(), vec![list])
            };
            if !d.is_enum {
                contents
            } else if c.fields.is_empty() {
                cx.std_call(
                    DYNAMIC,
                    "derived_unit_variant",
                    Vec::new(),
                    vec![string(&c.tag)],
                )
            } else {
                cx.std_call(
                    DYNAMIC,
                    "derived_variant",
                    Vec::new(),
                    vec![string(&c.tag), contents],
                )
            }
        })
    }

    // ---- Decode ----

    fn decode(&mut self, d: &Deriving, v: Local) -> Expr {
        if !d.is_enum {
            let c = &d.ctors[0];
            let args = c
                .fields
                .iter()
                .map(|(key, t)| {
                    self.std_call(
                        DYNAMIC,
                        "derived_field",
                        vec![*t],
                        vec![var(v), string(key)],
                    )
                })
                .collect();
            return self.construct(d, c, args);
        }
        let tag = self.std_call(DYNAMIC, "tag", Vec::new(), vec![var(v)]);
        self.by_tag(d, tag, "derived_unknown_tag", |cx, c| {
            if c.named {
                let args = c
                    .fields
                    .iter()
                    .map(|(key, t)| {
                        cx.std_call(
                            DYNAMIC,
                            "derived_contents_field",
                            vec![*t],
                            vec![var(v), string(key)],
                        )
                    })
                    .collect();
                return cx.construct(d, c, args);
            }
            let n = int(c.fields.len());
            let check = cx.std_call(
                DYNAMIC,
                "derived_contents_items",
                Vec::new(),
                vec![var(v), n],
            );
            let args = c
                .fields
                .iter()
                .enumerate()
                .map(|(i, (_, t))| {
                    cx.std_call(
                        DYNAMIC,
                        "derived_contents_index",
                        vec![*t],
                        vec![var(v), int(i)],
                    )
                })
                .collect();
            let value = cx.construct(d, c, args);
            cx.then(check, value)
        })
    }

    fn decode_all(&mut self, d: &Deriving, v: Local) -> Expr {
        if !d.is_enum {
            let c = &d.ctors[0];
            let parts = c
                .fields
                .iter()
                .map(|(key, t)| {
                    let read = self.std_call(
                        DYNAMIC,
                        "derived_field_all",
                        vec![*t],
                        vec![var(v), string(key)],
                    );
                    (*t, read)
                })
                .collect();
            return self.all_or_fail(d, c, parts);
        }
        let tag = self.std_call(DYNAMIC, "derived_tag_all", Vec::new(), vec![var(v)]);
        self.by_tag(d, tag, "derived_unknown_tag_all", |cx, c| {
            if c.named {
                let parts = c
                    .fields
                    .iter()
                    .map(|(key, t)| {
                        let read = cx.std_call(
                            DYNAMIC,
                            "derived_contents_field_all",
                            vec![*t],
                            vec![var(v), string(key)],
                        );
                        (*t, read)
                    })
                    .collect();
                return cx.all_or_fail(d, c, parts);
            }
            let n = int(c.fields.len());
            let check = cx.std_call(
                DYNAMIC,
                "derived_contents_items_all",
                Vec::new(),
                vec![var(v), n],
            );
            let parts = c
                .fields
                .iter()
                .enumerate()
                .map(|(i, (_, t))| {
                    let read = cx.std_call(
                        DYNAMIC,
                        "derived_contents_index_all",
                        vec![*t],
                        vec![var(v), int(i)],
                    );
                    (*t, read)
                })
                .collect();
            let value = cx.all_or_fail(d, c, parts);
            cx.then(check, value)
        })
    }

    /// Reads every part into an `Attempt` in order, fails with all their failures if any failed
    /// (`derived_check`), and otherwise constructs `c` from their values.
    fn all_or_fail(&mut self, d: &Deriving, c: &Ctor, parts: Vec<(TypeId, Expr)>) -> Expr {
        if parts.is_empty() {
            return self.construct(d, c, Vec::new());
        }
        let mut binds = Vec::with_capacity(parts.len());
        for (t, read) in parts {
            let attempt = self.std_type(DYNAMIC, "Attempt", vec![t]);
            let a = self.module.local("a", attempt);
            binds.push((a, t, read));
        }
        let error = self.std_type(DYNAMIC, "DecodeError", Vec::new());
        let errors_ty = self.module.list(error);
        let errors = binds
            .iter()
            .map(|&(a, t, _)| self.std_call(DYNAMIC, "errors_of", vec![t], vec![var(a)]))
            .collect();
        let list = Expr::List {
            elem: errors_ty,
            elems: errors,
            tail: None,
        };
        let check = self.std_call(DYNAMIC, "derived_check", Vec::new(), vec![list]);
        let args = binds
            .iter()
            .map(|&(a, t, _)| self.std_call(DYNAMIC, "derived_decoded", vec![t], vec![var(a)]))
            .collect();
        let value = self.construct(d, c, args);
        let mut body = self.then(check, value);
        for (a, _, read) in binds.into_iter().rev() {
            body = Expr::Let {
                var: a,
                value: Box::new(read),
                body: Box::new(body),
            };
        }
        body
    }

    // ---- ObjectKey ----

    fn derive_to_key(&mut self, to_key: DefId, d: &Deriving, x: Local) -> Expr {
        self.match_ctors(d, x, |cx, c, xs| match (c.fields.first(), xs.first()) {
            // A type with one field (E0339 rejects the other forms).
            (Some(&(_, t)), Some(&x)) if !d.is_enum => cx.trait_call(to_key, t, vec![Expr::Var(x)]),
            _ => string(&c.tag),
        })
    }

    fn derive_from_key(&mut self, from_key: DefId, d: &Deriving, k: Local) -> Expr {
        if !d.is_enum {
            let c = &d.ctors[0];
            let args = c
                .fields
                .iter()
                .map(|&(_, t)| self.trait_call(from_key, t, vec![var(k)]))
                .collect();
            return self.construct(d, c, args);
        }
        self.by_tag(d, var(k), "derived_unknown_key", |cx, c| {
            cx.construct(d, c, Vec::new())
        })
    }

    // ---- Shaped ----

    fn shape(&mut self, shape_in: DefId, d: &Deriving, visiting: Local) -> Expr {
        let name_ty = TypeId::STRING;
        let name = if d.params == 0 {
            string(&d.name)
        } else {
            let shape = self.std_type(SHAPE, "Shape", Vec::new());
            let args = (0..d.params)
                .map(|i| {
                    let p = self.module.type_param(i);
                    self.field_shape(shape_in, p, visiting)
                })
                .collect();
            let list = Expr::List {
                elem: shape,
                elems: args,
                tail: None,
            };
            self.std_call(
                SHAPE,
                "generic_name",
                Vec::new(),
                vec![string(&d.name), list],
            )
        };
        let n = self.module.local("name", name_ty);
        let strings = self.module.list(TypeId::STRING);
        let inner = self.module.local("visiting", strings);
        let build = if d.is_enum {
            let variant = self.std_type(SHAPE, "Variant", Vec::new());
            let variants = d
                .ctors
                .iter()
                .map(|c| {
                    if c.fields.is_empty() {
                        return self.std_call(
                            SHAPE,
                            "derived_no_fields",
                            Vec::new(),
                            vec![string(&c.tag)],
                        );
                    }
                    if c.named {
                        let fields = self.shape_fields(shape_in, c, inner);
                        self.std_call(
                            SHAPE,
                            "derived_named_fields",
                            Vec::new(),
                            vec![string(&c.tag), fields],
                        )
                    } else {
                        let shape = self.std_type(SHAPE, "Shape", Vec::new());
                        let shapes = c
                            .fields
                            .iter()
                            .map(|&(_, t)| self.field_shape(shape_in, t, inner))
                            .collect();
                        let list = Expr::List {
                            elem: shape,
                            elems: shapes,
                            tail: None,
                        };
                        self.std_call(
                            SHAPE,
                            "derived_positional_fields",
                            Vec::new(),
                            vec![string(&c.tag), list],
                        )
                    }
                })
                .collect();
            let list = Expr::List {
                elem: variant,
                elems: variants,
                tail: None,
            };
            self.std_call(SHAPE, "derived_variants", Vec::new(), vec![var(n), list])
        } else {
            let fields = self.shape_fields(shape_in, &d.ctors[0], inner);
            self.std_call(SHAPE, "derived_record", Vec::new(), vec![var(n), fields])
        };
        let enter = self.std_call(
            SHAPE,
            "derived_enter",
            Vec::new(),
            vec![var(n), var(visiting)],
        );
        let visited = self.std_call(
            SHAPE,
            "derived_visited",
            Vec::new(),
            vec![var(n), var(visiting)],
        );
        let reference = self.std_call(SHAPE, "derived_ref", Vec::new(), vec![var(n)]);
        Expr::Let {
            var: n,
            value: Box::new(name),
            body: Box::new(Expr::If {
                cond: Box::new(visited),
                then: Box::new(reference),
                else_: Box::new(Expr::Let {
                    var: inner,
                    value: Box::new(enter),
                    body: Box::new(build),
                }),
            }),
        }
    }

    /// `[Field(key, shape), ...]` of the named fields of `c`.
    fn shape_fields(&mut self, shape_in: DefId, c: &Ctor, visiting: Local) -> Expr {
        let field = self.std_type(SHAPE, "Field", Vec::new());
        let fields = c
            .fields
            .iter()
            .map(|(key, t)| {
                let shape = self.field_shape(shape_in, *t, visiting);
                self.std_call(SHAPE, "derived_field", Vec::new(), vec![string(key), shape])
            })
            .collect();
        Expr::List {
            elem: field,
            elems: fields,
            tail: None,
        }
    }

    /// `Shaped.shape_in(None: Option[t], visiting)`.
    fn field_shape(&mut self, shape_in: DefId, t: TypeId, visiting: Local) -> Expr {
        let proxy = self.std_call(SHAPE, "derived_proxy", vec![t], Vec::new());
        self.trait_call(shape_in, t, vec![proxy, var(visiting)])
    }

    // ---- Parts ----

    /// `match x { C(xs..) -> body(c, xs) ... }` over the constructors of `d`.
    fn match_ctors(
        &mut self,
        d: &Deriving,
        x: Local,
        mut body: impl FnMut(&mut Self, &Ctor, &[Local]) -> Expr,
    ) -> Expr {
        let arms = d
            .ctors
            .iter()
            .map(|c| {
                let xs: Vec<Local> = c
                    .fields
                    .iter()
                    .map(|&(_, t)| self.module.local("x", t))
                    .collect();
                let body = body(self, c, &xs);
                Arm {
                    pat: Pat::Ctor {
                        ctor: c.ctor,
                        fields: xs.iter().map(|&l| Pat::Bind(l)).collect(),
                    },
                    guard: None,
                    body,
                }
            })
            .collect();
        Expr::Match {
            scrutinee: Box::new(Expr::Var(x)),
            arms,
        }
    }

    /// `if tag == "A" { a } else if ... else unknown[Self]("one of A, B", tag)`, with `tag`
    /// evaluated once. `unknown` is the function of `Std.Dynamic` that fails for a tag that names
    /// no variant.
    fn by_tag(
        &mut self,
        d: &Deriving,
        tag: Expr,
        unknown: &str,
        mut variant: impl FnMut(&mut Self, &Ctor) -> Expr,
    ) -> Expr {
        let t = self.module.local("tag", TypeId::STRING);
        let tags: Vec<&str> = d.ctors.iter().map(|c| c.tag.as_str()).collect();
        let expected = format!("one of {}", tags.join(", "));
        let mut body = self.std_call(
            DYNAMIC,
            unknown,
            vec![d.ty],
            vec![string(&expected), var(t)],
        );
        for c in d.ctors.iter().rev() {
            let then = variant(self, c);
            body = Expr::If {
                cond: Box::new(Expr::Binary {
                    op: BinOp::Eq,
                    ty: OpTy::String,
                    lhs: Box::new(var(t)),
                    rhs: Box::new(string(&c.tag)),
                }),
                then: Box::new(then),
                else_: Box::new(body),
            };
        }
        Expr::Let {
            var: t,
            value: Box::new(tag),
            body: Box::new(body),
        }
    }

    /// The value of `c` of `d` from its fields.
    fn construct(&mut self, d: &Deriving, c: &Ctor, args: Vec<Expr>) -> Expr {
        let ty_args = (0..d.params).map(|i| self.module.type_param(i)).collect();
        Expr::Ctor {
            ctor: c.ctor,
            ty_args,
            args,
        }
    }

    /// `first; then`, with `first` of type `()`.
    fn then(&mut self, first: Expr, then: Expr) -> Expr {
        let unit = self.module.local("checked", TypeId::UNIT);
        Expr::Let {
            var: unit,
            value: Box::new(first),
            body: Box::new(then),
        }
    }

    /// A call of the trait function `def` for `Self` = `ty`.
    fn trait_call(&mut self, def: DefId, ty: TypeId, args: Vec<Expr>) -> Expr {
        self.derived_call(def, vec![ty], args)
    }

    /// A call of the function `name` of the std module `module`.
    fn std_call(
        &mut self,
        module: &str,
        name: &str,
        ty_args: Vec<TypeId>,
        args: Vec<Expr>,
    ) -> Expr {
        let def = self
            .std_def(module, name, DefKind::Fn)
            .unwrap_or_else(|| panic!("`{module}.{name}`, which derived impls call, is missing"));
        self.derived_call(def, ty_args, args)
    }

    fn derived_call(&mut self, def: DefId, ty_args: Vec<TypeId>, args: Vec<Expr>) -> Expr {
        debug_assert!(
            !self.takes_ev(def),
            "derived impls call no function with effects"
        );
        let f = self.fn_id(def, 0);
        Expr::Call {
            callee: Callee::Direct { f, ty_args },
            args,
            fails: self.declared_fails(def),
            suspends: false,
        }
    }

    /// The type `name` of the std module `module` with the type arguments `args`.
    fn std_type(&mut self, module: &str, name: &str, args: Vec<TypeId>) -> TypeId {
        let id = self
            .std_def(module, name, DefKind::Type)
            .or_else(|| self.std_def(module, name, DefKind::Enum))
            .or_else(|| self.std_def(module, name, DefKind::Error))
            .and_then(|def| self.enums.get(def).copied())
            .unwrap_or_else(|| panic!("`{module}.{name}`, which derived impls use, is missing"));
        self.module.intern(Type::Enum(id, args))
    }

    /// The definition `name` of kind `kind` in the bundled module `module`.
    fn std_def(&self, module: &str, name: &str, kind: DefKind) -> Option<DefId> {
        let hir = self.hir;
        hir.defs
            .iter()
            .find(|(_, d)| {
                d.kind == kind
                    && d.name == name
                    && match d.module {
                        DefModule::Source(m) => hir
                            .modules
                            .get(m)
                            .is_some_and(|items| items.is_core && items.name.to_string() == module),
                        _ => false,
                    }
            })
            .map(|(def, _)| def)
    }
}

/// The trait function a derived impl defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Encode,
    Decode,
    DecodeAll,
    ToKey,
    FromKey,
    Shape,
}

fn string(s: &str) -> Expr {
    Expr::Lit(Lit::String(s.to_owned()))
}

fn int(n: usize) -> Expr {
    Expr::Lit(Lit::Int(
        i32::try_from(n).expect("a field count fits in Int"),
    ))
}

fn var(l: Local) -> Expr {
    Expr::Var(l)
}
