//! Structural operations over the finite set of materialized payload layouts.
use super::*;
use crate::DerivedOperation as DeriveKind;

pub(super) struct Plan {
    pub(super) show: bool,
    pub(super) eq: bool,
    types: Vec<TypeId>,
    show_types: HashSet<TypeId>,
    eq_types: HashSet<TypeId>,
}

impl Plan {
    pub(super) fn new(
        flow: &emela_lir::Module,
        emitted: &HashSet<FnId>,
        layouts: &TypeLayouts<'_>,
    ) -> Result<Self, Vec<Error>> {
        let mut roots = Vec::new();
        let mut show = false;
        let mut eq = false;
        for (id, f) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
            let Some(kind) = crate::derived::kind(f) else {
                continue;
            };
            let ty = f
                .params
                .first()
                .map(|&p| f.vars[p].ty)
                .ok_or_else(|| invalid_flow_ir(&f.name, "derived operation has no argument"))?;
            let valid = !f.suspends
                && !f.fails
                && f.body.is_none()
                && !f.exported
                && crate::derived::supported(&flow.core, ty, kind)
                && match kind {
                    DeriveKind::Show => f.params.len() == 1 && f.ret == TypeId::STRING,
                    DeriveKind::Eq => {
                        f.params.len() == 2 && f.vars[f.params[1]].ty == ty && f.ret == TypeId::BOOL
                    }
                };
            if !valid {
                return Err(unsupported(
                    &f.name,
                    "invalid or unsupported embedded structural derivation signature",
                ));
            }
            show |= kind == DeriveKind::Show;
            eq |= kind == DeriveKind::Eq;
            let _ = id;
            roots.push((kind, ty));
        }
        let mut show_types = HashSet::new();
        let mut eq_types = HashSet::new();
        fn collect(layouts: &TypeLayouts<'_>, ty: TypeId, seen: &mut HashSet<TypeId>) {
            if !seen.insert(ty) {
                return;
            }
            match layouts.types[ty].clone() {
                Type::List(t) => collect(layouts, t, seen),
                _ => {
                    if let Some(&index) = layouts.aggregate_ids.get(&ty) {
                        let variants = match &layouts.aggregates[index].kind {
                            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                            AggregateKind::Enum(variants) => {
                                variants.iter().map(Vec::as_slice).collect()
                            }
                        };
                        for (variant, fields) in variants.into_iter().enumerate() {
                            for (field, &t) in fields.iter().enumerate() {
                                if !layouts.is_recursive_box_field(ty, variant, field) {
                                    collect(layouts, t, seen);
                                }
                            }
                        }
                    }
                }
            }
        }
        for (kind, ty) in roots {
            let seen = if kind == DeriveKind::Show {
                &mut show_types
            } else {
                &mut eq_types
            };
            collect(layouts, ty, seen);
        }
        for (kind, enabled) in [(DeriveKind::Show, show), (DeriveKind::Eq, eq)] {
            if !enabled {
                continue;
            }
            let seen = if kind == DeriveKind::Show {
                &mut show_types
            } else {
                &mut eq_types
            };
            for &ty in &layouts.recursive_box_types {
                if crate::derived::supported(&flow.core, ty, kind) {
                    collect(layouts, ty, seen);
                }
            }
        }
        let mut types: Vec<_> = show_types
            .union(&eq_types)
            .copied()
            .filter(|&t| layouts.type_repr(t) != Repr::Void)
            .collect();
        types.sort_by_key(|t| t.index());
        Ok(Self {
            show,
            eq,
            types,
            show_types,
            eq_types,
        })
    }

    pub(super) fn needs_float(&self, layouts: &TypeLayouts<'_>) -> bool {
        self.show
            && self
                .show_types
                .iter()
                .any(|&t| matches!(layouts.types[t], Type::Float))
    }

    pub(super) fn emit(&self, layouts: &TypeLayouts<'_>, writer: &mut Writer) {
        if !self.show && !self.eq {
            return;
        }
        if self.show {
            writer.line("typedef struct { char *buffer; uint16_t capacity; uint16_t length; } emela_derived_writer;");
            writer.begin_block("static inline char *emela_derived_output(emela_derived_writer *w)");
            writer.ret(Some("w->buffer == NULL ? NULL : w->buffer + w->length"));
            writer.end_block();
            writer.begin_block("static inline void emela_derived_text(emela_derived_writer *w, const char *text, uint16_t count)");
            writer.line("if (count > (uint16_t)(w->capacity - w->length) || (count != 0 && w->buffer == NULL)) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
            writer.line("for (uint32_t i = 0; i < count; ++i) w->buffer[(uint32_t)w->length + i] = text[i];");
            writer.line("w->length = (uint16_t)(w->length + count);");
            writer.end_block();
        }
        for &ty in &self.types {
            if self.eq_types.contains(&ty) {
                writer.line(&format!(
                    "static inline int32_t emela_derived_eq_{}({} a, {} b);",
                    ty.index(),
                    layouts.c_type(ty),
                    layouts.c_type(ty)
                ));
            }
            if self.show_types.contains(&ty) {
                writer.line(&format!(
                    "static inline void emela_derived_show_{}({} value, emela_derived_writer *w);",
                    ty.index(),
                    layouts.c_type(ty)
                ));
            }
        }
        if !layouts.recursive_box_types.is_empty() {
            if self.eq {
                self.emit_box(layouts, DeriveKind::Eq, writer);
            }
            if self.show {
                self.emit_box(layouts, DeriveKind::Show, writer);
            }
        }
        for &ty in &self.types {
            if self.eq_types.contains(&ty) {
                self.emit_type(layouts, ty, DeriveKind::Eq, writer);
            }
            if self.show_types.contains(&ty) {
                self.emit_type(layouts, ty, DeriveKind::Show, writer);
            }
        }
        writer.blank();
    }

    fn emit_box(&self, layouts: &TypeLayouts<'_>, kind: DeriveKind, w: &mut Writer) {
        let eq = kind == DeriveKind::Eq;
        w.begin_block(if eq {
            "static inline int32_t emela_derived_eq_box(void *a, void *b)"
        } else {
            "static inline void emela_derived_show_box(void *value, emela_derived_writer *w)"
        });
        if eq {
            w.line("if (a == NULL || b == NULL) return a == b;");
            w.line("uint32_t kind = emela_heap_header(a)->kind;");
            w.line("if (kind != emela_heap_header(b)->kind) return INT32_C(0);");
        } else {
            w.line("if (value == NULL) emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid derived payload\", UINT16_C(23));");
            w.line("uint32_t kind = emela_heap_header(value)->kind;");
        }
        w.begin_block("switch (kind)");
        for &ty in &layouts.recursive_box_types {
            let types = if eq { &self.eq_types } else { &self.show_types };
            if !types.contains(&ty) {
                continue;
            }
            w.line(&format!(
                "case UINT32_C({}):",
                layouts.recursive_box_kind(ty).unwrap()
            ));
            let c = layouts.c_type(ty);
            if eq {
                w.line(&format!(
                    "return emela_derived_eq_{}(*({c} const *)a, *({c} const *)b);",
                    ty.index()
                ));
            } else {
                w.line(&format!(
                    "emela_derived_show_{}(*({c} const *)value, w); return;",
                    ty.index()
                ));
            }
        }
        w.line("default: emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid derived payload\", UINT16_C(23));");
        w.end_block();
        w.end_block();
    }

    fn emit_type(&self, l: &TypeLayouts<'_>, ty: TypeId, kind: DeriveKind, w: &mut Writer) {
        let eq = kind == DeriveKind::Eq;
        let c = l.c_type(ty);
        w.begin_block(&if eq {
            format!(
                "static inline int32_t emela_derived_eq_{}({c} a, {c} b)",
                ty.index()
            )
        } else {
            format!(
                "static inline void emela_derived_show_{}({c} value, emela_derived_writer *w)",
                ty.index()
            )
        });
        w.line(if eq {
            "(void)a; (void)b;"
        } else {
            "(void)value; (void)w;"
        });
        match &l.types[ty] {
            Type::Fn(_) if !eq => text(w, "<fn>"),
            Type::Int | Type::Int64 | Type::Float | Type::Bool if eq => w.line("return a == b;"),
            Type::String if eq => {
                w.line("if (a.length != b.length) return INT32_C(0);");
                w.line("for (uint32_t i = 0; i < a.length; ++i) if (a.data[i] != b.data[i]) return INT32_C(0);");
                w.line("return INT32_C(1);");
            }
            Type::Int | Type::Int64 | Type::Float | Type::String => {
                let call = match l.types[ty] {
                    Type::Float => "emela_emb_float_to_string",
                    Type::String => "emela_emb_string_show",
                    _ => "emela_emb_int_to_string",
                };
                w.line(&format!("EmelaString text = {call}(value, emela_derived_output(w), (uint16_t)(w->capacity - w->length));"));
                w.line("w->length = (uint16_t)(w->length + text.length);");
            }
            Type::Bool => {
                w.begin_block("if (value)");
                text(w, "True");
                w.end_block();
                w.begin_block("else");
                text(w, "False");
                w.end_block();
            }
            Type::List(element) => {
                let node = l.list_node_name(ty);
                if eq {
                    w.line(&format!("{node} *left = a, *right = b;"));
                    w.begin_block("while (left != NULL && right != NULL)");
                    self.field(l, *element, "left->head", "right->head", false, kind, w);
                    w.line("left = left->tail; right = right->tail;");
                    w.end_block();
                    w.line("return left == right;");
                } else {
                    w.line(&format!("{node} *node = value;"));
                    text(w, "[");
                    w.line("int first = 1;");
                    w.begin_block("while (node != NULL)");
                    w.begin_block("if (!first)");
                    text(w, ", ");
                    w.end_block();
                    self.field(l, *element, "node->head", "", false, kind, w);
                    w.line("first = 0; node = node->tail;");
                    w.end_block();
                    text(w, "]");
                }
            }
            Type::Tuple(_) => {
                let AggregateKind::Tuple(fields) = &l.aggregates[l.aggregate_ids[&ty]].kind else {
                    unreachable!()
                };
                if !eq {
                    text(w, "(");
                }
                for (i, &t) in fields.iter().enumerate() {
                    if !eq && i != 0 {
                        text(w, ", ");
                    }
                    self.field(
                        l,
                        t,
                        &format!("{}.f{i}", if eq { "a" } else { "value" }),
                        &format!("b.f{i}"),
                        l.is_recursive_box_field(ty, 0, i),
                        kind,
                        w,
                    );
                }
                if eq {
                    w.line("return INT32_C(1);");
                } else {
                    text(w, ")");
                }
            }
            Type::Enum(id, _) => {
                let def = &l.core.enums[*id];
                if def.variants.is_empty() {
                    let message = if eq {
                        format!("cannot compare values of {}", def.name)
                    } else {
                        format!("cannot show a value of {}", def.name)
                    };
                    w.line(&format!(
                        "emela_board_defect(EMELA_DEFECT_PANIC, {}, UINT16_C({}));",
                        c_string_literal(message.as_bytes()),
                        message.len()
                    ));
                } else {
                    let scalar = l.enum_is_i32(*id);
                    let tag = |name: &str| {
                        if scalar {
                            name.to_owned()
                        } else {
                            format!("{name}.tag")
                        }
                    };
                    if eq {
                        w.line(&format!(
                            "if ({} != {}) return INT32_C(0);",
                            tag("a"),
                            tag("b")
                        ));
                    }
                    w.begin_block(&format!("switch ({})", tag(if eq { "a" } else { "value" })));
                    let variants = l
                        .aggregate_ids
                        .get(&ty)
                        .map(|&i| match &l.aggregates[i].kind {
                            AggregateKind::Enum(v) => v,
                            _ => unreachable!(),
                        });
                    for (v, definition) in def.variants.iter().enumerate() {
                        w.line(&format!("case INT32_C({v}):"));
                        w.begin_block("");
                        if !eq {
                            text(w, &definition.name);
                        }
                        if let Some(variants) = variants {
                            for (i, &t) in variants[v].iter().enumerate() {
                                if !eq {
                                    text(w, if i == 0 { "(" } else { ", " });
                                    if let emela_core::VariantFields::Named(names) =
                                        &definition.fields
                                    {
                                        text(w, &format!("{}: ", names[i]));
                                    }
                                }
                                self.field(
                                    l,
                                    t,
                                    &format!(
                                        "{}.payload.v{v}.f{i}",
                                        if eq { "a" } else { "value" }
                                    ),
                                    &format!("b.payload.v{v}.f{i}"),
                                    l.is_recursive_box_field(ty, v, i),
                                    kind,
                                    w,
                                );
                            }
                            if !eq && !variants[v].is_empty() {
                                text(w, ")");
                            }
                        }
                        w.line(if eq { "return INT32_C(1);" } else { "return;" });
                        w.end_block();
                    }
                    w.line("default: emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid derived payload\", UINT16_C(23));");
                    w.end_block();
                }
            }
            _ => unreachable!("unshowable type excluded by derivation validation"),
        }
        w.end_block();
    }

    // Each display field carries both operands and their ownership metadata.
    #[allow(clippy::too_many_arguments)]
    fn field(
        &self,
        l: &TypeLayouts<'_>,
        ty: TypeId,
        a: &str,
        b: &str,
        boxed: bool,
        kind: DeriveKind,
        w: &mut Writer,
    ) {
        if l.type_repr(ty) == Repr::Void {
            if kind == DeriveKind::Show {
                text(w, if ty == TypeId::UNIT { "()" } else { "" });
            }
            return;
        }
        if kind == DeriveKind::Eq {
            let call = if boxed {
                format!("emela_derived_eq_box({a}, {b})")
            } else {
                format!("emela_derived_eq_{}({a}, {b})", ty.index())
            };
            w.line(&format!("if (!{call}) return INT32_C(0);"));
        } else {
            w.line(&if boxed {
                format!("emela_derived_show_box({a}, w);")
            } else {
                format!("emela_derived_show_{}({a}, w);", ty.index())
            });
        }
    }
}

fn text(w: &mut Writer, text: &str) {
    w.line(&format!(
        "emela_derived_text(w, {}, UINT16_C({}));",
        c_string_literal(text.as_bytes()),
        text.len()
    ));
}

pub(super) fn emit_adapter(
    layouts: &TypeLayouts<'_>,
    f: &Function,
    name: &str,
    writer: &mut Writer,
) {
    let kind = crate::derived::kind(f).unwrap();
    writer.begin_block(&signature(
        layouts,
        f,
        name,
        true,
        false,
        kind == DeriveKind::Show,
    ));
    let ty = f.vars[f.params[0]].ty;
    if kind == DeriveKind::Eq {
        writer.ret(Some(&format!(
            "emela_derived_eq_{}({}, {})",
            ty.index(),
            variable_name(f.params[0]),
            variable_name(f.params[1])
        )));
    } else {
        writer.line("if (emela_string_output == NULL && emela_string_capacity != 0) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
        writer
            .line("emela_derived_writer output = {emela_string_output, emela_string_capacity, 0};");
        writer.line(&format!(
            "emela_derived_show_{}({}, &output);",
            ty.index(),
            variable_name(f.params[0])
        ));
        writer.ret(Some(
            "(EmelaString){.data=emela_string_output,.length=output.length,.owner=NULL}",
        ));
    }
    writer.end_block();
}
