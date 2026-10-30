use super::*;
pub(super) use crate::contract::auxiliary::ConstBridgePlan as Bridges;
pub(in crate::compiler::emit) trait Emission: Sized {
    fn name(&self, source: TypeId, destination: TypeId) -> String;
    fn call(
        &self,
        source: TypeId,
        destination: TypeId,
        value: &str,
        layouts: &TypeLayouts<'_>,
    ) -> String;
    fn signature(&self, source: TypeId, destination: TypeId, layouts: &TypeLayouts<'_>) -> String;
    fn emit(
        &self,
        flow: &crate::contract::ir::Program,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        heap: bool,
        writer: &mut Writer,
    ) -> Result<(), Vec<Error>>;
    fn emit_fields(
        &self,
        a: TypeId,
        b: TypeId,
        variant: usize,
        layouts: &TypeLayouts<'_>,
        writer: &mut Writer,
    ) -> Result<(), Vec<Error>>;
    fn emit_callable(
        &self,
        a: TypeId,
        b: TypeId,
        flow: &crate::contract::ir::Program,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        writer: &mut Writer,
    ) -> Result<(), Vec<Error>>;
    fn emit_getter(
        &self,
        id: FnId,
        flow: &crate::contract::ir::Program,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        writer: &mut Writer,
    );
}
impl Emission for Bridges {
    fn name(&self, source: TypeId, destination: TypeId) -> String {
        format!("emela_const_bridge_{}", self.ids[&(source, destination)])
    }

    fn call(
        &self,
        source: TypeId,
        destination: TypeId,
        value: &str,
        layouts: &TypeLayouts<'_>,
    ) -> String {
        format!(
            "{}({})",
            self.name(source, destination),
            if layouts.type_repr(source) == Repr::Void {
                ""
            } else {
                value
            }
        )
    }

    fn signature(&self, source: TypeId, destination: TypeId, layouts: &TypeLayouts<'_>) -> String {
        format!(
            "static inline EMELA_UNUSED {} {}({})",
            layouts.c_type(destination),
            self.name(source, destination),
            if layouts.type_repr(source) == Repr::Void {
                "void".into()
            } else {
                format!("{} value", layouts.c_type(source))
            }
        )
    }

    fn emit(
        &self,
        flow: &crate::contract::ir::Program,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        heap: bool,
        writer: &mut Writer,
    ) -> Result<(), Vec<Error>> {
        for &(a, b) in &self.pairs {
            writer.line(&format!("{};", self.signature(a, b, layouts)));
        }
        for &(a, b) in &self.pairs {
            writer.begin_block(&self.signature(a, b, layouts));
            if a == b {
                if layouts.type_repr(a) != Repr::Void {
                    layouts.emit_own_value_in_place(a, "value", writer);
                    writer.ret(Some("value"));
                } else {
                    writer.ret(None);
                }
                writer.end_block();
                continue;
            }
            if !heap
                && (matches!(layouts.shapes.types[a], Type::Any | Type::List(_))
                    || matches!(layouts.shapes.types[b], Type::Any))
            {
                if layouts.type_repr(a) != Repr::Void {
                    writer.line("(void)value;");
                }
                if matches!(layouts.shapes.types[a], Type::List(_))
                    && matches!(layouts.shapes.types[b], Type::List(_))
                {
                    writer . line ("if(value!=NULL) emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, \"fixed heap disabled\", UINT16_C(19));") ;
                    writer.ret(Some("NULL"));
                } else {
                    writer . line ("emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, \"fixed heap disabled\", UINT16_C(19));") ;
                }
                writer.end_block();
                continue;
            }
            match (
                layouts.shapes.types[a].clone(),
                layouts.shapes.types[b].clone(),
            ) {
                (Type::Any, _) => {
                    if layouts.type_repr(b) == Repr::Void {
                        writer.line("(void)value;");
                        writer.ret(None);
                    } else {
                        let index = layouts.shapes.any_box_ids[&b];
                        writer . line (& format ! ("if(value.tag != 0 || value.payload.any_box == NULL || emela_heap_header(value.payload.any_box)->kind != UINT32_C({}) || ((emela_any_box *)value.payload.any_box)->type != UINT32_C({index})) emela_board_defect(EMELA_DEFECT_PANIC, \"shared const Any type mismatch\", UINT16_C(30));" , layouts . any_box_kind ())) ;
                        writer.decl(
                            &layouts.c_type(b),
                            "result",
                            Some(&format!(
                                "((emela_any_box *)value.payload.any_box)->payload.v{index}"
                            )),
                        );
                        layouts.emit_own_value_in_place(b, "result", writer);
                        writer.ret(Some("result"));
                    }
                }
                (_, Type::Any) => {
                    if layouts.type_repr(a) == Repr::Void {
                        writer.ret(Some("(EmelaAny){0}"));
                    } else {
                        let index = layouts.shapes.any_box_ids[&a];
                        writer.decl(
                            "emela_any_box *",
                            "box",
                            Some(&format!(
                                "(emela_any_box *)emela_heap_allocate(sizeof(*box), UINT32_C({}))",
                                layouts.any_box_kind()
                            )),
                        );
                        writer.assign("box->type", &format!("UINT32_C({index})"));
                        layouts.emit_owned_value_store(
                            a,
                            "value",
                            &format!("box->payload.v{index}"),
                            writer,
                        );
                        writer.ret(Some("(EmelaAny){.payload.any_box=box}"));
                    }
                }
                (Type::List(at), Type::List(bt)) => {
                    let an = layouts.list_node_name(a);
                    let bn = layouts.list_node_name(b);
                    writer.line("void *result=NULL;void **tail=&result;");
                    writer.begin_block("while(value != NULL)");
                    writer.decl(
                        &format!("{an} *"),
                        "source",
                        Some(&format!("({an} *)value")),
                    );
                    writer.decl(
                        &format!("{bn} *"),
                        "node",
                        Some(&format!(
                            "({bn} *)emela_heap_allocate(sizeof(*node),UINT32_C({}))",
                            layouts.list_index(b)
                        )),
                    );
                    writer.assign("node->tail", "NULL");
                    if layouts.type_repr(bt) == Repr::Void {
                        writer.assign("node->_unit", "0");
                    } else {
                        writer.assign("node->head", &self.call(at, bt, "source->head", layouts));
                    }
                    writer.assign("*tail", "node");
                    writer.assign("tail", "&node->tail");
                    writer.assign("value", "source->tail");
                    writer.end_block();
                    writer.ret(Some("result"));
                }
                (Type::Tuple(_), Type::Tuple(_)) => {
                    writer.decl(&layouts.c_type(b), "result", Some("{0}"));
                    self.emit_fields(a, b, 0, layouts, writer)?;
                    writer.ret(Some("result"));
                }
                (Type::Enum(id, _), Type::Enum(_, _)) => {
                    if !layouts.enum_layout_required(id) {
                        layouts.emit_own_value_in_place(b, "value", writer);
                        writer.ret(Some("value"));
                    } else {
                        writer.decl(&layouts.c_type(b), "result", Some("{0}"));
                        writer.assign("result.tag", "value.tag");
                        writer.begin_block("switch(value.tag)");
                        for variant in 0..crate::semantics::abi::bridge_fields(layouts, a).len() {
                            writer.line(&format!("case INT32_C({variant}):"));
                            writer.begin_block("");
                            self.emit_fields(a, b, variant, layouts, writer)?;
                            writer.line("break;");
                            writer.end_block();
                        }
                        writer . line ("default: emela_board_defect(EMELA_DEFECT_PANIC, \"invalid shared const variant\", UINT16_C(28));") ;
                        writer.end_block();
                        writer.ret(Some("result"));
                    }
                }
                (Type::Fn(_), Type::Fn(_)) => {
                    self.emit_callable(a, b, flow, layouts, names, writer)?
                }
                _ => unreachable!("bridge pairs are validated"),
            }
            writer.end_block();
            writer.blank();
        }
        Ok(())
    }

    fn emit_fields(
        &self,
        a: TypeId,
        b: TypeId,
        variant: usize,
        layouts: &TypeLayouts<'_>,
        writer: &mut Writer,
    ) -> Result<(), Vec<Error>> {
        let source_fields = crate::semantics::abi::bridge_fields(layouts, a);
        let destination_fields = crate::semantics::abi::bridge_fields(layouts, b);
        for (index, (&at, &bt)) in source_fields[variant]
            .iter()
            .zip(&destination_fields[variant])
            .enumerate()
        {
            if layouts.type_repr(bt) == Repr::Void {
                continue;
            }
            if !self.ids.contains_key(&(at, bt)) {
                writer . line ("emela_board_defect(EMELA_DEFECT_PANIC, \"unmaterialized shared const field\", UINT16_C(33));") ;
                continue;
            }
            let field = if matches!(layouts.shapes.types[a], Type::Tuple(_)) {
                format!("f{index}")
            } else {
                format!("payload.v{variant}.f{index}")
            };
            let source = if layouts.is_recursive_box_field(a, variant, index) {
                format!("(*(({} const *)value.{field}))", layouts.c_type(at))
            } else {
                format!("value.{field}")
            };
            let converted = self.call(at, bt, &source, layouts);
            if layouts.is_recursive_box_field(b, variant, index) {
                let kind = layouts.recursive_box_kind(bt).ok_or_else(|| {
                    invalid_flow_ir("shared const", "materialized bridge field has no box kind")
                })?;
                writer.assign(
                    &format!("result.{field}"),
                    &format!(
                        "emela_heap_allocate(sizeof({}),UINT32_C({kind}))",
                        layouts.c_type(bt)
                    ),
                );
                writer.assign(
                    &format!("(*(({} *)result.{field}))", layouts.c_type(bt)),
                    &converted,
                );
            } else {
                writer.assign(&format!("result.{field}"), &converted);
            }
        }
        Ok(())
    }

    fn emit_callable(
        &self,
        a: TypeId,
        b: TypeId,
        flow: &crate::contract::ir::Program,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        writer: &mut Writer,
    ) -> Result<(), Vec<Error>> {
        let mut found = false;
        for &(source, destination, at, bt) in &self.callables {
            if at != a
                || bt != b
                || !names.contains_key(&source)
                || !names.contains_key(&destination)
            {
                continue;
            }
            found = true;
            let pointer = |id: FnId, ty: TypeId| {
                if layouts.functions.closure_functions.contains_key(&id) {
                    format!(
                        "{}{}",
                        names[&id],
                        if flow.functions[id].suspends {
                            "_push"
                        } else {
                            ""
                        }
                    )
                } else {
                    function_ref_adapter_name(id, ty)
                }
            };
            writer.begin_block(&format!("if(value.call == {})", pointer(source, at)));
            writer.decl(&layouts.c_type(b), "result", Some("{0}"));
            writer.assign("result.call", &pointer(destination, bt));
            if let (Some(&ai), Some(&bi)) = (
                layouts.functions.closure_functions.get(&source),
                layouts.functions.closure_functions.get(&destination),
            ) {
                writer . line ("if(value.env==NULL) emela_board_defect(EMELA_DEFECT_PANIC, \"null shared const environment\", UINT16_C(29));") ;
                writer.decl(
                    &format!("emela_closure_env_{ai} *"),
                    "source",
                    Some(&format!("(emela_closure_env_{ai} *)value.env")),
                );
                writer . decl (& format ! ("emela_closure_env_{bi} *") , "environment" , Some (& format ! ("(emela_closure_env_{bi} *)emela_heap_allocate(sizeof(*environment),UINT32_C({}))" , layouts . closure_environment_kind (bi)))) ;
                writer.line("(void)source;");
                for (index, (ac, bc)) in layouts.functions.closure_environments[ai]
                    .captures
                    .iter()
                    .zip(&layouts.functions.closure_environments[bi].captures)
                    .enumerate()
                {
                    if layouts.type_repr(bc.ty) == Repr::Void {
                        continue;
                    }
                    writer.assign(
                        &format!("environment->c{index}"),
                        &self.call(ac.ty, bc.ty, &format!("source->c{index}"), layouts),
                    );
                }
                writer.assign("result.env", "environment");
            }
            writer.ret(Some("result"));
            writer.end_block();
        }
        if !found {
            writer.line("(void)value;");
        }
        writer . line ("emela_board_defect(EMELA_DEFECT_PANIC, \"foreign shared const callable requires an explicit typed ABI\", UINT16_C(60));") ;
        Ok(())
    }

    fn emit_getter(
        &self,
        id: FnId,
        flow: &crate::contract::ir::Program,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        writer: &mut Writer,
    ) {
        let canonical = self.getters[&id];
        let source = flow.functions[canonical].ret;
        let destination = flow.functions[id].ret;
        let index = u32::from(id.into_raw());
        let state = format!("emela_const_state_{index}");
        let cache = format!("emela_const_value_{index}");
        let is_void = layouts.type_repr(destination) == Repr::Void;
        writer.line(&format!("static EmelaConstState {state};"));
        if !is_void {
            writer.line(&format!("static {} {cache};", layouts.c_type(destination)));
        }
        writer.begin_block(&signature(
            layouts,
            &flow.functions[id],
            &names[&id],
            !flow.functions[id].exported,
            false,
            false,
        ));
        for (&target, name) in names {
            writer.line(&format!(
                "(void)&{}{};",
                name,
                if flow.functions[target].suspends {
                    "_push"
                } else {
                    ""
                }
            ));
        }
        writer.begin_block(&format!("if({state}.status==3)"));
        writer.line(&format!(
            "emela_embedded_report_defect({state}.kind,{state}.message,{state}.length);"
        ));
        writer.end_block();
        writer.begin_block(&format!("if({state}.status==1)"));
        let message = format!(
            "const {} refers to itself while being evaluated",
            flow.functions[canonical].name
        );
        writer.line(&format!(
            "emela_const_self_reference(EMELA_DEFECT_PANIC,{},UINT16_C({}));",
            c_string_literal(message.as_bytes()),
            message.len()
        ));
        writer.end_block();
        writer.begin_block(&format!("if({state}.status==0)"));
        if layouts.type_repr(source) == Repr::Void {
            writer.line(&format!("{}();", names[&canonical]));
        } else {
            writer.decl(
                &layouts.c_type(source),
                "canonical",
                Some(&format!("{}()", names[&canonical])),
            );
        }
        let prefix = format!(
            "while evaluating const {}: ",
            flow.functions[canonical].name
        );
        writer.line(&format!(
            "EmelaConstContext context={{emela_const_current,&{state},{},UINT16_C({})}};",
            c_string_literal(prefix.as_bytes()),
            prefix.len()
        ));
        writer.assign(&format!("{state}.status"), "1");
        writer.assign("emela_const_current", "&context");
        let call = self.call(source, destination, "canonical", layouts);
        if is_void {
            writer.line(&format!("{call};"));
        } else {
            writer.assign(&cache, &call);
        }
        if layouts.type_repr(source) != Repr::Void && layouts.is_managed_value(source) {
            layouts.emit_release_value_expression(source, "canonical", writer);
        }
        writer.assign("emela_const_current", "context.previous");
        writer.assign(&format!("{state}.status"), "2");
        writer.end_block();
        if is_void {
            writer.ret(None);
        } else {
            layouts.emit_retain_value_expression(destination, &cache, writer);
            writer.ret(Some(&cache));
        }
        writer.end_block();
        writer.blank();
    }
}
