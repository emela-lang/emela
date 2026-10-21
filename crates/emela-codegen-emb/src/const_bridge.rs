//! Reify a shared const value with its specialized data layouts and native callable bodies.
use super::*;

#[derive(Default)]
pub(super) struct Bridges {
    pub(super) getters: HashMap<FnId, FnId>,
    pairs: Vec<(TypeId, TypeId)>,
    ids: HashMap<(TypeId, TypeId), usize>,
    callables: Vec<(FnId, FnId, TypeId, TypeId)>,
}

pub(super) fn prepare(flow: &mut emela_lir::Module, groups: &[crate::const_origins::Group]) {
    for group in groups {
        for &id in &group.specializations {
            let function = &mut flow.functions[id];
            function.constant = false;
            let statement = function.stmts.alloc(Stmt::Let {
                dst: None,
                value: Value::Call {
                    callee: Callee::Direct(group.canonical),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
                on_defect: None,
                on_suspend: None,
            });
            // Keep both callable families reachable until the C initializer is replaced.
            let body = function.body.expect("const definition has a body");
            function.seqs[body].stmts.insert(0, statement);
        }
    }
}

impl Bridges {
    pub(super) fn new(
        flow: &emela_lir::Module,
        groups: &[crate::const_origins::Group],
        closures: &crate::static_closure::ClosureLowering,
        layouts: &mut TypeLayouts<'_>,
    ) -> Result<Self, Vec<Error>> {
        let mut result = Self::default();
        let mut family = HashSet::new();
        for group in groups {
            for &typed in &group.specializations {
                result.getters.insert(typed, group.canonical);
                family.insert((group.canonical, typed));
            }
        }
        for group in crate::const_origins::function_groups(flow)? {
            for &source in &group {
                for &destination in &group {
                    family.insert((source, destination));
                }
            }
        }
        loop {
            let mut added = false;
            for (&source, &(parent, ordinal)) in &closures.families {
                for (&destination, &(typed_parent, typed_ordinal)) in &closures.families {
                    if ordinal == typed_ordinal && family.contains(&(parent, typed_parent)) {
                        added |= family.insert((source, destination));
                    }
                }
            }
            if !added {
                break;
            }
        }
        let mut references: HashMap<FnId, Vec<TypeId>> = HashMap::new();
        for (_, function) in flow.functions.iter() {
            for (_, statement) in function.stmts.iter() {
                if let Stmt::Let {
                    dst: Some(dst),
                    value: Value::FnRef(target),
                    ..
                } = statement
                {
                    let ty = function.vars[*dst].ty;
                    let types = references.entry(*target).or_default();
                    if !types.contains(&ty) {
                        types.push(ty);
                    }
                }
            }
        }
        for (source, destination) in family {
            for &source_ty in references.get(&source).into_iter().flatten() {
                for &destination_ty in references.get(&destination).into_iter().flatten() {
                    result
                        .callables
                        .push((source, destination, source_ty, destination_ty));
                }
            }
        }
        result.callables.sort_by_key(|(a, b, c, d)| {
            (
                u32::from(a.into_raw()),
                u32::from(b.into_raw()),
                c.index(),
                d.index(),
            )
        });
        for group in groups {
            for &typed in &group.specializations {
                result.collect(
                    flow.functions[group.canonical].ret,
                    flow.functions[typed].ret,
                    layouts,
                )?;
            }
        }
        Ok(result)
    }

    fn collect(
        &mut self,
        source: TypeId,
        destination: TypeId,
        layouts: &mut TypeLayouts<'_>,
    ) -> Result<(), Vec<Error>> {
        if self.ids.contains_key(&(source, destination)) {
            return Ok(());
        }
        let index = self.pairs.len();
        self.ids.insert((source, destination), index);
        self.pairs.push((source, destination));
        if source == destination {
            return Ok(());
        }
        let mut dependencies = Vec::new();
        match (
            layouts.types[source].clone(),
            layouts.types[destination].clone(),
        ) {
            (Type::Any, _) => layouts.add_any_box_type(destination),
            (_, Type::Any) => layouts.add_any_box_type(source),
            (Type::List(a), Type::List(b)) => dependencies.push((a, b)),
            (Type::Tuple(a), Type::Tuple(b)) if a.len() == b.len() => {
                dependencies.extend(a.into_iter().zip(b))
            }
            (Type::Enum(a, _), Type::Enum(b, _)) if a == b => {
                let source_fields = self.fields(source, layouts);
                let destination_fields = self.fields(destination, layouts);
                for (left, right) in source_fields.into_iter().zip(destination_fields) {
                    for (a, b) in left.into_iter().zip(right) {
                        if layouts.has_materialized_payload(a)
                            && layouts.has_materialized_payload(b)
                        {
                            dependencies.push((a, b));
                        }
                    }
                }
            }
            (Type::Fn(_), Type::Fn(_)) => {
                for &(a, b, at, bt) in &self.callables {
                    if at != source || bt != destination {
                        continue;
                    }
                    match (
                        layouts.closure_functions.get(&a),
                        layouts.closure_functions.get(&b),
                    ) {
                        (Some(&ai), Some(&bi)) => {
                            let ac = &layouts.closure_environments[ai].captures;
                            let bc = &layouts.closure_environments[bi].captures;
                            if ac.len() != bc.len() {
                                return Err(invalid_flow_ir(
                                    "shared const",
                                    "specialized closure capture arity differs",
                                ));
                            }
                            dependencies.extend(ac.iter().zip(bc).map(|(a, b)| (a.ty, b.ty)));
                        }
                        (None, None) => {}
                        _ => {
                            return Err(invalid_flow_ir(
                                "shared const",
                                "specialized closure capture representation differs",
                            ));
                        }
                    }
                }
            }
            _ => {
                return Err(unsupported(
                    "shared const",
                    "incompatible canonical and specialized value layouts",
                ));
            }
        }
        for (a, b) in dependencies {
            self.collect(a, b, layouts)?;
        }
        Ok(())
    }

    fn fields(&self, ty: TypeId, layouts: &TypeLayouts<'_>) -> Vec<Vec<TypeId>> {
        let Some(&index) = layouts.aggregate_ids.get(&ty) else {
            return vec![];
        };
        match &layouts.aggregates[index].kind {
            AggregateKind::Tuple(fields) => vec![fields.clone()],
            AggregateKind::Enum(fields) => fields.clone(),
        }
    }

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
            "static inline {} {}({})",
            layouts.c_type(destination),
            self.name(source, destination),
            if layouts.type_repr(source) == Repr::Void {
                "void".into()
            } else {
                format!("{} value", layouts.c_type(source))
            }
        )
    }

    pub(super) fn emit(
        &self,
        flow: &emela_lir::Module,
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
                && (matches!(layouts.types[a], Type::Any | Type::List(_))
                    || matches!(layouts.types[b], Type::Any))
            {
                if layouts.type_repr(a) != Repr::Void {
                    writer.line("(void)value;");
                }
                if matches!(layouts.types[a], Type::List(_))
                    && matches!(layouts.types[b], Type::List(_))
                {
                    writer.line("if(value!=NULL) emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, \"fixed heap disabled\", UINT16_C(19));");
                    writer.ret(Some("NULL"));
                } else {
                    writer.line("emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, \"fixed heap disabled\", UINT16_C(19));");
                }
                writer.end_block();
                continue;
            }
            match (layouts.types[a].clone(), layouts.types[b].clone()) {
                (Type::Any, _) => {
                    if layouts.type_repr(b) == Repr::Void {
                        writer.line("(void)value;");
                        writer.ret(None);
                    } else {
                        let index = layouts.any_box_ids[&b];
                        writer.line(&format!("if(value.tag != 0 || value.payload.any_box == NULL || emela_heap_header(value.payload.any_box)->kind != UINT32_C({}) || ((emela_any_box *)value.payload.any_box)->type != UINT32_C({index})) emela_board_defect(EMELA_DEFECT_PANIC, \"shared const Any type mismatch\", UINT16_C(30));",layouts.any_box_kind()));
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
                        let index = layouts.any_box_ids[&a];
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
                        for variant in 0..self.fields(a, layouts).len() {
                            writer.line(&format!("case INT32_C({variant}):"));
                            writer.begin_block("");
                            self.emit_fields(a, b, variant, layouts, writer)?;
                            writer.line("break;");
                            writer.end_block();
                        }
                        writer.line("default: emela_board_defect(EMELA_DEFECT_PANIC, \"invalid shared const variant\", UINT16_C(28));");
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
        let source_fields = self.fields(a, layouts);
        let destination_fields = self.fields(b, layouts);
        for (index, (&at, &bt)) in source_fields[variant]
            .iter()
            .zip(&destination_fields[variant])
            .enumerate()
        {
            if layouts.type_repr(bt) == Repr::Void {
                continue;
            }
            if !self.ids.contains_key(&(at, bt)) {
                writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"unmaterialized shared const field\", UINT16_C(33));");
                continue;
            }
            let field = if matches!(layouts.types[a], Type::Tuple(_)) {
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
        flow: &emela_lir::Module,
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
                if layouts.closure_functions.contains_key(&id) {
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
                layouts.closure_functions.get(&source),
                layouts.closure_functions.get(&destination),
            ) {
                writer.line("if(value.env==NULL) emela_board_defect(EMELA_DEFECT_PANIC, \"null shared const environment\", UINT16_C(29));");
                writer.decl(
                    &format!("emela_closure_env_{ai} *"),
                    "source",
                    Some(&format!("(emela_closure_env_{ai} *)value.env")),
                );
                writer.decl(&format!("emela_closure_env_{bi} *"),"environment",Some(&format!("(emela_closure_env_{bi} *)emela_heap_allocate(sizeof(*environment),UINT32_C({}))",layouts.closure_environment_kind(bi))));
                writer.line("(void)source;");
                for (index, (ac, bc)) in layouts.closure_environments[ai]
                    .captures
                    .iter()
                    .zip(&layouts.closure_environments[bi].captures)
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
        writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"foreign shared const callable requires an explicit typed ABI\", UINT16_C(60));");
        Ok(())
    }

    pub(super) fn emit_getter(
        &self,
        id: FnId,
        flow: &emela_lir::Module,
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
        // A typed facade must not create a second source const context around canonical evaluation.
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
