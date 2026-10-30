//! Render the already selected ABI and ownership helpers.
use super::*;
use crate::contract::abi::*;
pub(in crate::compiler::emit) trait LayoutEmission: Sized {
    fn emit_retain_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer);
    fn emit_enqueue_value_expression(
        &self,
        ty: TypeId,
        expression: &str,
        pending: &str,
        tail: &str,
        writer: &mut Writer,
    );
    fn emit_release_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer);
    fn emit_noheap_ownership_stubs(&self, writer: &mut Writer);
    fn emit_heap_ownership_helpers(&self, writer: &mut Writer);
    fn emit_aggregate_retain_body(&self, aggregate: &AggregateLayout, writer: &mut Writer);
    fn emit_aggregate_enqueue_body(&self, aggregate: &AggregateLayout, writer: &mut Writer);
    fn emit_heap_destroy_switch(
        &self,
        environments: &[crate::contract::lowered::ClosureEnvironment],
        writer: &mut Writer,
    );
    fn emit_public_release_declarations(&self, writer: &mut Writer);
    fn emit_public_release_helpers(&self, writer: &mut Writer);
    fn emit_public_function_ownership_declarations(&self, writer: &mut Writer);
    fn emit_public_function_ownership_helpers(&self, writer: &mut Writer);
    fn emit_list_node_declarations(&self, writer: &mut Writer);
    fn emit_runtime_value_declarations(&self, writer: &mut Writer);
    fn emit_declarations(&self, writer: &mut Writer);
    fn emit_string_declaration(&self, writer: &mut Writer);
    fn emit_error_declaration(&self, writer: &mut Writer);
    fn emit_freeze_value_in_place(&self, ty: TypeId, value: &str, writer: &mut Writer);
    fn emit_own_value_in_place(&self, ty: TypeId, value: &str, writer: &mut Writer);
    fn emit_owned_value_store(&self, ty: TypeId, source: &str, target: &str, writer: &mut Writer);
}
impl LayoutEmission for CLayoutPlan<'_> {
    fn emit_retain_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.shapes.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!("emela_heap_retain({expression});"));
        } else if matches!(self.shapes.types[ty], Type::Any) {
            writer.line(&format!("emela_any_retain({expression});"));
        } else if matches!(self.shapes.types[ty], Type::String | Type::Bytes) {
            writer.line(&format!("emela_heap_retain(({expression}).owner);"));
        } else if matches!(self.shapes.types[ty], Type::Fn(_)) {
            writer.line(&format!("emela_heap_retain(({expression}).env);"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_retain_value_{index}(&({expression}));"
            ));
        }
    }

    fn emit_enqueue_value_expression(
        &self,
        ty: TypeId,
        expression: &str,
        pending: &str,
        tail: &str,
        writer: &mut Writer,
    ) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.shapes.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!(
                "emela_heap_enqueue({expression}, {pending}, {tail});"
            ));
        } else if matches!(self.shapes.types[ty], Type::Any) {
            writer.line(&format!(
                "emela_any_enqueue({expression}, {pending}, {tail});"
            ));
        } else if matches!(self.shapes.types[ty], Type::String | Type::Bytes) {
            writer.line(&format!(
                "emela_heap_enqueue(({expression}).owner, {pending}, {tail});"
            ));
        } else if matches!(self.shapes.types[ty], Type::Fn(_)) {
            writer.line(&format!(
                "emela_heap_enqueue(({expression}).env, {pending}, {tail});"
            ));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(&({expression}), {pending}, {tail});"
            ));
        }
    }

    fn emit_release_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.shapes.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!("emela_heap_release({expression});"));
        } else if matches!(self.shapes.types[ty], Type::Any) {
            writer.line(&format!("emela_any_release({expression});"));
        } else if matches!(self.shapes.types[ty], Type::String | Type::Bytes) {
            writer.line(&format!("emela_heap_release(({expression}).owner);"));
        } else if matches!(self.shapes.types[ty], Type::Fn(_)) {
            writer.line(&format!("emela_heap_release(({expression}).env);"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_release_value_{index}(&({expression}));"
            ));
        }
    }

    fn emit_noheap_ownership_stubs(&self, writer: &mut Writer) {
        for (index, aggregate) in self.shapes.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty)
                || self.contains_heap_ref(aggregate.ty, &mut HashSet::new())
            {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "static inline EMELA_UNUSED void emela_heap_retain_value_{index}(const {aggregate_type} *value)"
            ));
            writer.line("(void)value;");
            writer.end_block();
            writer.begin_block(&format!(
                "static inline EMELA_UNUSED void emela_heap_release_value_{index}({aggregate_type} *value)"
            ));
            writer.line("(void)value;");
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_heap_ownership_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.shapes.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "static void emela_heap_retain_value_{index}(const {aggregate_type} *value)"
            ));
            self.emit_aggregate_retain_body(aggregate, writer);
            writer.end_block();
            writer . begin_block (& format ! ("static void emela_heap_enqueue_value_{index}(const {aggregate_type} *value, emela_heap_block **pending, emela_heap_block **tail)")) ;
            self.emit_aggregate_enqueue_body(aggregate, writer);
            writer.end_block();
            writer.begin_block(&format!(
                "static void emela_heap_release_value_{index}(const {aggregate_type} *value)"
            ));
            writer.line("emela_heap_block *pending = NULL;");
            writer.line("emela_heap_block *tail = NULL;");
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(value, &pending, &tail);"
            ));
            writer.line("emela_heap_drain(pending, tail);");
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_aggregate_retain_body(&self, aggregate: &AggregateLayout, writer: &mut Writer) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_recursive_box_field(aggregate.ty, 0, index) {
                        writer.line(&format!("emela_heap_retain(value->f{index});"));
                        continue;
                    }
                    if self.is_managed_value(field_ty) {
                        self.emit_retain_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_recursive_box_field(aggregate.ty, variant, index) {
                            writer.line(&format!(
                                "emela_heap_retain(value->payload.v{variant}.f{index});"
                            ));
                            continue;
                        }
                        if self.is_managed_value(field_ty) {
                            self.emit_retain_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    fn emit_aggregate_enqueue_body(&self, aggregate: &AggregateLayout, writer: &mut Writer) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_recursive_box_field(aggregate.ty, 0, index) {
                        writer.line(&format!(
                            "emela_heap_enqueue(value->f{index}, pending, tail);"
                        ));
                        continue;
                    }
                    if self.is_managed_value(field_ty) {
                        self.emit_enqueue_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            "pending",
                            "tail",
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_recursive_box_field(aggregate.ty, variant, index) {
                            writer . line (& format ! ("emela_heap_enqueue(value->payload.v{variant}.f{index}, pending, tail);")) ;
                            continue;
                        }
                        if self.is_managed_value(field_ty) {
                            self.emit_enqueue_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                "pending",
                                "tail",
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    fn emit_heap_destroy_switch(
        &self,
        environments: &[crate::contract::lowered::ClosureEnvironment],
        writer: &mut Writer,
    ) {
        if self.runtime.has_resumes && self.has_error_ownership() {
            writer . line ("static void emela_resume_enqueue_error(void *value, emela_heap_block **pending, emela_heap_block **tail);") ;
        }
        writer . begin_block ("static void emela_heap_destroy(uint32_t kind, void *value, emela_heap_block **pending, emela_heap_block **tail)" ,) ;
        writer.line("(void)value; (void)pending; (void)tail;");
        writer.begin_block("switch (kind)");
        for &list_ty in &self.shapes.list_types {
            let Type::List(element) = self.shapes.types[list_ty] else {
                unreachable!("list layout has a non-List type")
            };
            let node_type = self.list_node_name(list_ty);
            let kind = self.list_index(list_ty);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!("{node_type} *node = ({node_type} *)value;"));
            if self.type_repr(element) != Repr::Void && self.is_managed_value(element) {
                self.emit_enqueue_value_expression(
                    element,
                    "node->head",
                    "pending",
                    "tail",
                    writer,
                );
            }
            writer.line("emela_heap_enqueue(node->tail, pending, tail);");
            writer.line("break;");
            writer.end_block();
        }
        for &box_ty in &self.shapes.recursive_box_types {
            let box_index = self.shapes.recursive_box_ids[&box_ty];
            let kind = self.shapes.list_types.len() + box_index;
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            self.emit_enqueue_value_expression(
                box_ty,
                &format!("(*(({} const *)value))", self.c_type(box_ty)),
                "pending",
                "tail",
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        for (index, &cell_ty) in self.shapes.cell_types.iter().enumerate() {
            let Type::Cell(content) = self.shapes.types[cell_ty] else {
                unreachable!()
            };
            let kind = self.shapes.list_types.len() + self.shapes.recursive_box_types.len() + index;
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            self.emit_enqueue_value_expression(
                content,
                &format!("(*(({} *)value))", self.cell_content_c_type(content)),
                "pending",
                "tail",
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        let string_owner_kind = self.string_owner_kind();
        writer.line(&format!("case UINT32_C({string_owner_kind}):"));
        writer.begin_block("");
        writer.line("break;");
        writer.end_block();
        if self.runtime.has_resumes {
            if self.has_error_ownership() {
                writer.line(&format!(
                    "case UINT32_C({}): emela_resume_enqueue_error(value, pending, tail); break;",
                    self.resume_kind()
                ));
            } else {
                writer.line(&format!("case UINT32_C({}): break;", self.resume_kind()));
            }
        }
        if self.runtime.has_causes {
            writer.line(&format!("case UINT32_C({}): {{", self.cause_kind()));
            if self.has_error_ownership() {
                writer.line("emela_cause *cause = (emela_cause *)value;");
                writer.line(
                    "if (cause->kind == 0) emela_error_enqueue(cause->error, pending, tail);",
                );
            }
            writer.line("break; }");
        }
        if self.has_error_notes() {
            writer . line (& format ! ("case UINT32_C({}): emela_heap_enqueue(((emela_error_notes *)value)->head, pending, tail); break;" , self . error_notes_kind ())) ;
            writer . line (& format ! ("case UINT32_C({}): emela_heap_enqueue(((emela_error_note *)value)->next, pending, tail); break;" , self . error_note_kind ())) ;
        }
        if self.runtime.has_scopes {
            writer.line(&format!("case UINT32_C({}): {{", self.scope_kind()));
            if self.has_error_ownership() {
                writer.line(
                    "emela_error_enqueue(((emela_scope *)value)->first_error, pending, tail);",
                );
            }
            writer.line("break; }");
            writer.line(&format!("case UINT32_C({}): {{", self.child_kind()));
            writer.line("emela_child *child = (emela_child *)value;");
            if self.has_cell_cycles() {
                writer . line ("if (emela_heap_trace_mode == 0 || emela_heap_trace_mode == 3) emela_child_unlink(child);") ;
            } else {
                writer.line("emela_child_unlink(child);");
            }
            writer.line("emela_heap_enqueue(child->scope, pending, tail);");
            if self.has_error_ownership() {
                writer.line("emela_error_enqueue(child->error, pending, tail);");
            }
            self.emit_enqueue_value_expression(
                TypeId::ANY,
                "child->result",
                "pending",
                "tail",
                writer,
            );
            writer.line("break; }");
            writer.line(&format!("case UINT32_C({}): {{", self.deferred_kind()));
            if self.runtime.deferred_pool {
                writer.line("emela_deferred *deferred = (emela_deferred *)value;");
                writer.begin_block("if (deferred->slot != EMELA_SLOT_NONE)");
                if !self.has_cell_cycles() {
                    writer.line("EmelaAny stored = emela_deferred_recycle(deferred);");
                } else {
                    writer . line ("EmelaAny stored = emela_heap_trace_mode == 1 || emela_heap_trace_mode == 2 ? emela_deferred_slot_get(deferred)->value : emela_deferred_recycle(deferred);") ;
                }
                self.emit_enqueue_value_expression(
                    TypeId::ANY,
                    "stored",
                    "pending",
                    "tail",
                    writer,
                );
                writer.end_block();
            }
            writer.line("emela_heap_enqueue(((emela_deferred *)value)->scope, pending, tail);");
            writer.line("break; }");
        }
        for (index, environment) in environments.iter().enumerate() {
            let kind = self.closure_environment_kind(index);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "emela_closure_env_{index} *environment = (emela_closure_env_{index} *)value;"
            ));
            writer.line("(void)environment;");
            for (capture_index, capture) in environment.captures.iter().enumerate() {
                if self.type_repr(capture.ty) != Repr::Void && self.is_managed_value(capture.ty) {
                    self.emit_enqueue_value_expression(
                        capture.ty,
                        &format!("environment->c{capture_index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
            }
            writer.line("break;");
            writer.end_block();
        }
        for (index, adapter) in self.shapes.function_adapters.iter().enumerate() {
            let kind = self.function_adapter_kind(index);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "emela_fn_coerce_env_{index} *environment = (emela_fn_coerce_env_{index} *)value;"
            ));
            writer.line("(void)environment;");
            if self.is_managed_value(adapter.source) {
                self.emit_enqueue_value_expression(
                    adapter.source,
                    "environment->source",
                    "pending",
                    "tail",
                    writer,
                );
            }
            writer.line("break;");
            writer.end_block();
        }
        if self.runtime.evidence_nodes_required {
            let kind = self.evidence_node_kind();
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line("emela_evidence_node *entry = (emela_evidence_node *)value;");
            writer.line("emela_heap_enqueue(entry->handler, pending, tail);");
            writer.line("emela_heap_enqueue(entry->parent, pending, tail);");
            writer.line("break;");
            writer.end_block();
        }
        for layout in &self.shapes.handler_layouts {
            let kind = self.handler_kind(layout.effect);
            let handler_type = crate::semantics::abi::handler_type_name(layout.effect);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "{handler_type} *handler = ({handler_type} *)value;"
            ));
            writer.line("(void)handler;");
            for (index, &operation) in layout.operations.iter().enumerate() {
                if self.is_managed_value(operation) {
                    self.emit_enqueue_value_expression(
                        operation,
                        &format!("handler->op_{index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
            }
            writer.line("break;");
            writer.end_block();
        }
        if self.runtime.any_box_required {
            let kind = self.any_box_kind();
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line("emela_any_box *box = (emela_any_box *)value;");
            writer.begin_block("switch (box->type)");
            for (index, &ty) in self.shapes.any_box_types.iter().enumerate() {
                writer.line(&format!("case UINT32_C({index}):"));
                writer.begin_block("");
                if self.is_managed_value(ty) {
                    self.emit_enqueue_value_expression(
                        ty,
                        &format!("box->payload.v{index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
                writer.line("break;");
                writer.end_block();
            }
            writer.line("default:");
            writer.begin_block("");
            writer.line(
                "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid Any box type\", UINT16_C(20));",
            );
            writer.line("break;");
            writer.end_block();
            writer.end_block();
            writer.line("break;");
            writer.end_block();
        }
        writer.line("default:");
        writer.begin_block("");
        writer.line(&format!(
            "emela_board_defect(EMELA_DEFECT_PANIC, {}, UINT16_C({}));",
            c_string_literal(b"invalid fixed heap reference"),
            b"invalid fixed heap reference".len()
        ));
        writer.line("break;");
        writer.end_block();
        writer.end_block();
        writer.end_block();
        writer.blank();
    }

    fn emit_public_release_declarations(&self, writer: &mut Writer) {
        for (index, aggregate) in self.shapes.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.line("/* Retains heap references before copying this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value);"
            ));
            writer.line("/* Releases the heap references owned by this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value);"
            ));
        }
    }

    fn emit_public_release_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.shapes.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_retain_value_{index}(value);"));
            writer.end_block();
            writer.end_block();
            writer.begin_block(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_release_value_{index}(value);"));
            writer.line(&format!("*value = ({aggregate_type}){{0}};"));
            writer.end_block();
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_public_function_ownership_declarations(&self, writer: &mut Writer) {
        for &ty in &self.shapes.function_types {
            if !self.shapes.closure_function_types.contains(&ty) {
                continue;
            }
            let function_type = self.c_type(ty);
            writer.line("/* Retains the environment owned by this function value. */");
            writer.line(&format!(
                "void emela_embedded_retain_function_{}(const {function_type} *value);",
                ty.index()
            ));
            writer.line("/* Releases the environment owned by this function value. */");
            writer.line(&format!(
                "void emela_embedded_release_function_{}({function_type} *value);",
                ty.index()
            ));
        }
    }

    fn emit_public_function_ownership_helpers(&self, writer: &mut Writer) {
        for &ty in &self.shapes.function_types {
            if !self.shapes.closure_function_types.contains(&ty) {
                continue;
            }
            let function_type = self.c_type(ty);
            writer.begin_block(&format!(
                "void emela_embedded_retain_function_{}(const {function_type} *value)",
                ty.index()
            ));
            writer.begin_block("if (value != NULL)");
            writer.line("emela_heap_retain(value->env);");
            writer.end_block();
            writer.end_block();
            writer.blank();
            writer.begin_block(&format!(
                "void emela_embedded_release_function_{}({function_type} *value)",
                ty.index()
            ));
            writer.begin_block("if (value != NULL)");
            writer.line("emela_heap_release(value->env);");
            writer.end_block();
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_list_node_declarations(&self, writer: &mut Writer) {
        for &ty in &self.shapes.list_types {
            let Type::List(element) = self.shapes.types[ty].clone() else {
                unreachable!("list layout has a non-List type")
            };
            writer.line(&format!("typedef struct {} {{", self.list_node_name(ty)));
            if self.type_repr(element) == Repr::Void {
                writer.line("  uint8_t _unit;");
            } else {
                writer.line(&format!("  {} head;", self.c_type(element)));
            }
            writer.line("  void *tail;");
            writer.line(&format!("}} {};", self.list_node_name(ty)));
            writer.blank();
        }
    }

    fn emit_runtime_value_declarations(&self, writer: &mut Writer) {
        if self.runtime.evidence_nodes_required {
            writer.line("typedef struct emela_evidence_node {");
            writer.line("  uint32_t effect;");
            writer.line("  void *handler;");
            writer.line("  void *parent;");
            writer.line("} emela_evidence_node;");
            writer.blank();
        }
        for (index, adapter) in self.shapes.function_adapters.iter().enumerate() {
            writer.line(&format!("typedef struct emela_fn_coerce_env_{index} {{"));
            writer.line(&format!("  {} source;", self.c_type(adapter.source)));
            writer.line(&format!("}} emela_fn_coerce_env_{index};"));
            writer.blank();
        }
        if self.runtime.any_box_required {
            writer.line("typedef struct emela_any_box {");
            writer.line("  uint32_t type;");
            writer.line("  union {");
            writer.line("    uint8_t _empty;");
            for (index, &ty) in self.shapes.any_box_types.iter().enumerate() {
                writer.line(&format!("    {} v{index};", self.c_type(ty)));
            }
            writer.line("  } payload;");
            writer.line("} emela_any_box;");
            writer.blank();
        }
        for layout in &self.shapes.handler_layouts {
            writer.line(&format!(
                "typedef struct {} {{",
                crate::semantics::abi::handler_type_name(layout.effect)
            ));
            writer.line("  int32_t active;");
            writer.line("  const char *released_message;");
            writer.line("  uint16_t released_message_length;");
            for (index, _) in layout
                .operations
                .iter()
                .enumerate()
                .filter(|(i, _)| self.shapes.core.effects[layout.effect].ops[*i].suspend)
            {
                writer.line(&format!("  const char *resume_error_{index};"));
                writer.line(&format!("  uint16_t resume_error_length_{index};"));
            }
            if layout.operations.is_empty() {
                writer.line("  uint8_t _empty;");
            } else {
                for (index, &operation) in layout.operations.iter().enumerate() {
                    writer.line(&format!("  {} op_{index};", self.c_type(operation)));
                }
            }
            writer.line(&format!(
                "}} {};",
                crate::semantics::abi::handler_type_name(layout.effect)
            ));
            writer.blank();
        }
    }

    fn emit_declarations(&self, writer: &mut Writer) {
        for &ty in &self.shapes.function_types {
            let name = function_type_name(ty);
            writer.line(&format!("typedef struct {name} {name};"));
        }
        for aggregate in &self.shapes.aggregates {
            let name = type_name(aggregate.ty);
            writer.line(&format!("typedef struct {name} {name};"));
        }
        writer.blank();
        for &ty in &self.shapes.function_types {
            let Type::Fn(function_type) = self.shapes.types[ty].clone() else {
                unreachable!("function type registry contains a non-function")
            };
            let mut parameters = vec!["void *emela_env".to_owned()];
            if function_type.suspends {
                parameters.insert(0, "emela_fiber *fiber".to_owned());
            }
            parameters.extend(
                function_type
                    .params
                    .iter()
                    .filter(|&&parameter| self.type_repr(parameter) != Repr::Void)
                    .map(|&parameter| self.c_type(parameter)),
            );
            if function_type.ev {
                parameters.push("void *emela_evidence".to_owned());
            }
            if function_type.fails {
                parameters.push("EmelaError *emela_error_out".to_owned());
            }
            if function_type.suspends && self.type_repr(function_type.ret) != Repr::Void {
                parameters.push(format!("{} *result", self.c_type(function_type.ret)));
            }
            writer.line(&format!("struct {} {{", function_type_name(ty)));
            writer.line(&format!(
                "  {} (*call)({});",
                if function_type.suspends {
                    "void".to_owned()
                } else {
                    self.c_type(function_type.ret)
                },
                parameters.join(", ")
            ));
            writer.line("  void *env;");
            writer.line("};");
        }
        if !self.shapes.function_types.is_empty() {
            writer.blank();
        }
        let error_dependencies = self.error_declaration_dependencies();
        let declarations = self
            .shapes
            .aggregates
            .iter()
            .enumerate()
            .filter(|(index, _)| error_dependencies.contains(index))
            .map(|(_, aggregate)| Some(aggregate))
            .chain(std::iter::once(None))
            .chain(
                self.shapes
                    .aggregates
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !error_dependencies.contains(index))
                    .map(|(_, aggregate)| Some(aggregate)),
            );
        for aggregate in declarations {
            let Some(aggregate) = aggregate else {
                self.emit_error_declaration(writer);
                continue;
            };
            writer.line(&format!("struct {} {{", type_name(aggregate.ty)));
            match &aggregate.kind {
                AggregateKind::Tuple(fields) => {
                    let mut represented = false;
                    for (index, &field) in fields.iter().enumerate() {
                        if self.type_repr(field) == Repr::Void {
                            continue;
                        }
                        represented = true;
                        let field_type = if self.is_recursive_box_field(aggregate.ty, 0, index) {
                            "void *".to_owned()
                        } else {
                            self.c_type(field)
                        };
                        writer.line(&format!("  {field_type} f{index};"));
                    }
                    if !represented {
                        writer.line("  uint8_t _unit;");
                    }
                }
                AggregateKind::Enum(variants) => {
                    writer.line("  int32_t tag;");
                    writer.line("  union {");
                    for (variant_index, fields) in variants.iter().enumerate() {
                        let represented_fields = fields
                            .iter()
                            .enumerate()
                            .filter(|(_, field)| self.type_repr(**field) != Repr::Void)
                            .map(|(field_index, field)| {
                                let field_type = if self.is_recursive_box_field(
                                    aggregate.ty,
                                    variant_index,
                                    field_index,
                                ) {
                                    "void *".to_owned()
                                } else {
                                    self.c_type(*field)
                                };
                                format!("{field_type} f{field_index};")
                            })
                            .collect::<Vec<_>>();
                        let body = if represented_fields.is_empty() {
                            "uint8_t _unit;".to_owned()
                        } else {
                            represented_fields.join(" ")
                        };
                        writer.line(&format!("    struct {{ {body} }} v{variant_index};"));
                    }
                    writer.line("  } payload;");
                }
            }
            writer.line("};");
            writer.blank();
        }
    }

    fn emit_string_declaration(&self, writer: &mut Writer) {
        writer.line(
            "typedef struct { const char *data; uint16_t length; void *owner; } EmelaString;",
        );
        writer.blank();
    }

    fn emit_error_declaration(&self, writer: &mut Writer) {
        if !self.runtime.has_error_abi && !self.runtime.any_abi {
            return;
        }
        writer.line("struct EmelaError {");
        writer.line("  uint32_t tag;");
        if self.has_error_notes() {
            writer.line("  void *notes;");
        }
        writer.line("  union {");
        if self.runtime.any_abi {
            writer.line("    void *any_box;");
        }
        if self.runtime.error_types.is_empty() {
            if !self.runtime.any_abi {
                writer.line("    uint8_t _empty;");
            }
        } else {
            for (enum_id, ty) in &self.runtime.error_types {
                writer.line(&format!(
                    "    {} e{};",
                    self.c_type(*ty),
                    enum_index(*enum_id)
                ));
            }
        }
        writer.line("  } payload;");
        writer.line("};");
        writer.blank();
    }

    fn emit_freeze_value_in_place(&self, ty: TypeId, value: &str, writer: &mut Writer) {
        if matches!(self.shapes.types[ty], Type::String | Type::Bytes) {
            writer.begin_block(&format!("if (({value}).owner == NULL)"));
            let copy = if matches!(self.shapes.types[ty], Type::Bytes) {
                "emela_heap_copy_bytes"
            } else {
                "emela_heap_copy_string"
            };
            writer.assign(value, &format!("{copy}({value})"));
            writer.end_block();
        } else if self.has_borrowed_views(ty) {
            let index = self.shapes.aggregate_ids[&ty];
            writer.line(&format!("emela_heap_freeze_value_{index}(&({value}));"));
        }
    }

    fn emit_own_value_in_place(&self, ty: TypeId, value: &str, writer: &mut Writer) {
        if matches!(self.shapes.types[ty], Type::Any) && self.is_managed_value(ty) {
            writer.line(&format!("emela_any_own(&({value}));"));
        } else if matches!(self.shapes.types[ty], Type::String | Type::Bytes) {
            let own = if matches!(self.shapes.types[ty], Type::Bytes) {
                "emela_heap_own_bytes"
            } else {
                "emela_heap_own_string"
            };
            writer.assign(value, &format!("{own}({value})"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!("emela_heap_own_value_{index}(&({value}));"));
        } else if self.is_managed_value(ty) {
            self.emit_retain_value_expression(ty, value, writer);
        }
    }

    fn emit_owned_value_store(&self, ty: TypeId, source: &str, target: &str, writer: &mut Writer) {
        writer.assign(target, source);
        self.emit_own_value_in_place(ty, target, writer);
    }
}
