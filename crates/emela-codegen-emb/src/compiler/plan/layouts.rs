//! Build C layouts and ABI/resource metadata. No C source is written here.
use super::*;
use crate::contract::abi::*;
pub(in crate::compiler::plan) trait LayoutPreparation<'a>: Sized {
    fn new(
        flow: &'a crate::contract::ir::Program,
        emitted: &HashSet<FnId>,
        closures: &crate::contract::lowered::ClosureMetadata,
        deferred_pool: bool,
    ) -> Result<Self, Vec<Error>>;
    fn add_any_box_type(&mut self, ty: TypeId);
    fn add_function_adapter(
        &mut self,
        source: TypeId,
        destination: TypeId,
        function: &str,
    ) -> Result<(), Vec<Error>>;
    fn validate_finite_types(
        &mut self,
        ty: TypeId,
        _function: &str,
        active: &mut HashSet<TypeId>,
        validated: &mut HashSet<TypeId>,
    ) -> Result<(), Vec<Error>>;
    fn collect(
        &mut self,
        ty: TypeId,
        function: &str,
        states: &mut HashMap<TypeId, u8>,
    ) -> Result<(), Vec<Error>>;
}
impl<'a> LayoutPreparation<'a> for LayoutBuilder<'a> {
    fn new(
        flow: &'a crate::contract::ir::Program,
        emitted: &HashSet<FnId>,
        closures: &crate::contract::lowered::ClosureMetadata,
        deferred_pool: bool,
    ) -> Result<Self, Vec<Error>> {
        let mut layouts = TypeLayouts {
            shapes: TypeShapes {
                core: &flow.core,
                types: flow.core.types.clone(),
                aggregates: Vec::new(),
                aggregate_ids: HashMap::new(),
                recursive_edges: HashSet::new(),
                expanding_enums: flow.core.expanding_enums.clone(),
                recursive_box_types: Vec::new(),
                recursive_box_ids: HashMap::new(),
                list_types: Vec::new(),
                list_ids: HashMap::new(),
                function_types: Vec::new(),
                closure_function_types: closures
                    .environments
                    .iter()
                    .map(|environment| environment.function_type)
                    .collect(),
                cell_types: Vec::new(),
                cell_ids: HashMap::new(),
                function_adapters: Vec::new(),
                function_adapter_ids: HashMap::new(),
                handler_layouts: Vec::new(),
                handler_layout_ids: HashMap::new(),
                any_box_types: Vec::new(),
                any_box_ids: HashMap::new(),
            },
            functions: FunctionAbis {
                discarded_results: HashMap::new(),
                function_results: flow.functions.iter().map(|(id, f)| (id, f.ret)).collect(),
                closure_functions: closures.functions.clone(),
                closure_environments: closures.environments.clone(),
                owned_string_returns: closures
                    .functions
                    .keys()
                    .filter_map(|id| {
                        let f = &flow.functions[*id];
                        (!f.suspends && matches!(flow.core.types[f.ret], Type::String))
                            .then(|| f.name.clone())
                    })
                    .collect(),
                runtime_consts: HashSet::new(),
                owned_aggregate_returns: HashSet::new(),
                error_shows: flow.error_shows.clone(),
            },
            runtime: RuntimeValues {
                resume_types: Vec::new(),
                has_resumes: suspend::has_operations(flow, emitted) || flow.has_scopes,
                has_unwind: suspend::has_operations(flow, emitted)
                    || flow.has_scopes
                    || suspend::has_protects(flow, emitted)
                    || (flow
                        .functions
                        .iter()
                        .any(|(id, f)| emitted.contains(&id) && f.suspends)
                        && flow.functions.iter().any(|(id, f)| {
                            emitted.contains(&id)
                                && f.stmts.iter().any(|(_, s)| {
                                    matches!(
                                        s,
                                        Stmt::Let {
                                            value: Value::Fiber {
                                                op: crate::contract::ir::FiberOp::CauseRaise,
                                                ..
                                            },
                                            ..
                                        }
                                    )
                                })
                        })),
                has_scopes: flow.has_scopes,
                deferred_pool,
                deferred_complete_types: Vec::new(),
                deferred_read_types: Vec::new(),
                deferred_poll_types: Vec::new(),
                deferred_await_types: Vec::new(),
                child_join_types: Vec::new(),
                child_await_types: Vec::new(),
                child_interrupts: false,
                has_causes: flow.functions.iter().any(|(id, f)| {
                    emitted.contains(&id)
                        && f.stmts.iter().any(|(_, s)| {
                            matches!(
                                s,
                                Stmt::Let {
                                    value: Value::Fiber {
                                        op: crate::contract::ir::FiberOp::Attempt { .. }
                                            | crate::contract::ir::FiberOp::Await { .. }
                                            | crate::contract::ir::FiberOp::CauseKind { .. }
                                            | crate::contract::ir::FiberOp::CauseToString
                                            | crate::contract::ir::FiberOp::CauseRaise,
                                        ..
                                    },
                                    ..
                                }
                            )
                        })
                }),
                has_cause_raise: flow.functions.iter().any(|(id, f)| {
                    emitted.contains(&id)
                        && f.stmts.iter().any(|(_, s)| {
                            matches!(
                                s,
                                Stmt::Let {
                                    value: Value::Fiber {
                                        op: crate::contract::ir::FiberOp::CauseRaise,
                                        ..
                                    },
                                    ..
                                }
                            )
                        })
                }),
                attempt_types: Vec::new(),
                fork_types: Vec::new(),
                evidence_nodes_required: false,
                any_abi: false,
                any_box_required: false,
                error_types: Vec::new(),
                has_error_abi: flow.functions.iter().any(|(id, function)| {
                    emitted.contains(&id)
                        && (function.fails
                            || function
                                .seqs
                                .iter()
                                .any(|(_, sequence)| matches!(sequence.term, Term::Fail(_)))
                            || function.stmts.iter().any(|(_, statement)| match statement {
                                Stmt::Let { value, .. } => match value {
                                    Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } => {
                                        on_fail.is_some()
                                    }
                                    Value::IsError { .. } | Value::AsError { .. } => true,
                                    _ => false,
                                },
                                _ => false,
                            }))
                }),
                managed_error_payloads: false,
            },
            properties: None,
        };
        let mut validated = HashSet::new();
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            for ty in std::iter::once(function.ret).chain(function.vars.iter().map(|(_, v)| v.ty)) {
                layouts.validate_finite_types(
                    ty,
                    &function.name,
                    &mut HashSet::new(),
                    &mut validated,
                )?;
            }
        }
        let mut states = HashMap::new();
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            layouts.collect(function.ret, &function.name, &mut states)?;
            for (_, variable) in function.vars.iter() {
                layouts.collect(variable.ty, &function.name, &mut states)?;
            }
            for (_, statement) in function.stmts.iter() {
                let Stmt::Let { dst, value, .. } = statement else {
                    continue;
                };
                match value {
                    Value::Builtin {
                        op: Builtin::CellNew,
                        args,
                    } => {
                        if let Some(content) =
                            args.first().and_then(|arg| operand_type(function, arg))
                        {
                            let cell = layouts.shapes.types.intern(Type::Cell(content));
                            layouts.collect(cell, &function.name, &mut states)?;
                        }
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Attempt { exit, .. },
                        args,
                        ..
                    } => {
                        let [Operand::Var(body)] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Attempt body"));
                        };
                        let body_ty = function.vars[*body].ty;
                        let Type::Fn(ft) = layouts.shapes.types[body_ty].clone() else {
                            return Err(invalid_flow_ir(&function.name, "Attempt function type"));
                        };
                        if ft.ev || !ft.params.is_empty() {
                            return Err(unsupported(&function.name, "Attempt body ABI"));
                        }
                        let mut params =
                            vec![TypeId::UNIT; layouts.shapes.core.enums[*exit].params];
                        if params.is_empty() {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "Attempt Exit type parameters",
                            ));
                        }
                        params[0] = ft.ret;
                        let result = dst
                            .map(|v| function.vars[v].ty)
                            .unwrap_or_else(|| layouts.shapes.types.enum_type(*exit, params));
                        layouts.collect(result, &function.name, &mut states)?;
                        if child::success_type(&layouts, result) != Some(ft.ret) {
                            return Err(invalid_flow_ir(&function.name, "Attempt result layout"));
                        }
                        if !layouts.runtime.attempt_types.contains(&(body_ty, result)) {
                            layouts.runtime.attempt_types.push((body_ty, result));
                        }
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Interrupt,
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Interrupt arity"));
                        };
                        child::value_type(&layouts, function, handle)?;
                        layouts.runtime.child_interrupts = true;
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Await { exit },
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Await arity"));
                        };
                        let ty = child::value_type(&layouts, function, handle)?;
                        let mut params =
                            vec![TypeId::UNIT; layouts.shapes.core.enums[*exit].params];
                        let Some(first) = params.first_mut() else {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "Await Exit type parameters",
                            ));
                        };
                        *first = ty;
                        let result = layouts.shapes.types.enum_type(*exit, params);
                        if dst.is_some_and(|dst| {
                            layouts.shapes.types.erase_sets(function.vars[dst].ty)
                                != layouts.shapes.types.erase_sets(result)
                        }) {
                            return Err(invalid_flow_ir(&function.name, "Await result type"));
                        }
                        layouts.collect(result, &function.name, &mut states)?;
                        if child::success_type(&layouts, result) != Some(ty) {
                            return Err(invalid_flow_ir(&function.name, "Await Success payload"));
                        }
                        layouts.add_any_box_type(ty);
                        if !layouts.runtime.child_await_types.contains(&result) {
                            layouts.runtime.child_await_types.push(result);
                        }
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Join,
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Join arity"));
                        };
                        let ty = child::value_type(&layouts, function, handle)?;
                        if dst.is_some_and(|dst| function.vars[dst].ty != ty) {
                            return Err(invalid_flow_ir(&function.name, "Join result type"));
                        }
                        layouts.add_any_box_type(ty);
                        if !layouts.runtime.child_join_types.contains(&ty) {
                            layouts.runtime.child_join_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::DeferredAwait,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.runtime.deferred_read_types.contains(&ty) {
                            layouts.runtime.deferred_read_types.push(ty);
                        }
                        if !layouts.runtime.deferred_await_types.contains(&ty) {
                            layouts.runtime.deferred_await_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Poll,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.runtime.deferred_read_types.contains(&ty) {
                            layouts.runtime.deferred_read_types.push(ty);
                        }
                        if let Some(dst) = dst {
                            let option = function.vars[*dst].ty;
                            if option_payload_type(&layouts, option) != Some(ty) {
                                return Err(invalid_flow_ir(&function.name, "Poll result type"));
                            }
                            if !layouts.runtime.deferred_poll_types.contains(&option) {
                                layouts.runtime.deferred_poll_types.push(option);
                            }
                        }
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Complete,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.runtime.deferred_complete_types.contains(&ty) {
                            layouts.runtime.deferred_complete_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Fork,
                        args,
                        ..
                    } => {
                        if let Some(Operand::Var(body)) = args.get(1) {
                            let ty = function.vars[*body].ty;
                            if !layouts.runtime.fork_types.contains(&ty) {
                                layouts.runtime.fork_types.push(ty);
                            }
                        }
                        layouts.runtime.any_abi = true;
                        layouts.runtime.any_box_required = true;
                    }
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::Deferred,
                        ..
                    } => {
                        layouts.runtime.any_abi = true;
                        layouts.runtime.any_box_required = true;
                    }
                    Value::Handler { effect, ops, .. } => {
                        let operations = ops
                            .iter()
                            .map(|&operation| function.vars[operation].ty)
                            .collect::<Vec<_>>();
                        for (index, &ty) in operations.iter().enumerate() {
                            if !flow.core.effects[*effect].ops[index].suspend {
                                continue;
                            }
                            let Type::Fn(ft) = &layouts.shapes.types[ty] else {
                                unreachable!()
                            };
                            let resume = *ft
                                .params
                                .last()
                                .expect("suspend handler has a resume parameter");
                            if !layouts.runtime.resume_types.contains(&resume) {
                                layouts.runtime.resume_types.push(resume);
                            }
                            layouts.shapes.closure_function_types.insert(resume);
                        }
                        if let Some(previous) = layouts
                            .shapes
                            .handler_layout_ids
                            .get(effect)
                            .and_then(|index| layouts.shapes.handler_layouts.get(*index))
                        {
                            if previous.operations != operations {
                                return Err(invalid_flow_ir(
                                    &function.name,
                                    "handlers for one effect have inconsistent operation function types",
                                ));
                            }
                        } else {
                            let index = layouts.shapes.handler_layouts.len();
                            layouts.shapes.handler_layouts.push(HandlerLayout {
                                effect: *effect,
                                operations,
                            });
                            layouts.shapes.handler_layout_ids.insert(*effect, index);
                        }
                    }
                    Value::EvExtend { bindings, .. } if !bindings.is_empty() => {
                        layouts.runtime.evidence_nodes_required = true;
                    }
                    Value::EvLookup { .. } => {
                        layouts.runtime.evidence_nodes_required = true;
                    }
                    Value::Coerce(source) => {
                        if let Some(destination) = dst {
                            let source_ty = function.vars[*source].ty;
                            let destination_ty = function.vars[*destination].ty;
                            if matches!(layouts.shapes.types[source_ty], Type::Any)
                                && !matches!(layouts.shapes.types[destination_ty], Type::Any)
                            {
                                layouts.add_any_box_type(destination_ty);
                            } else if matches!(layouts.shapes.types[destination_ty], Type::Any)
                                && !matches!(layouts.shapes.types[source_ty], Type::Any)
                            {
                                layouts.add_any_box_type(source_ty);
                            }
                            if matches!(layouts.shapes.types[source_ty], Type::Fn(_))
                                && matches!(layouts.shapes.types[destination_ty], Type::Fn(_))
                            {
                                layouts.add_function_adapter(
                                    source_ty,
                                    destination_ty,
                                    &function.name,
                                )?;
                            }
                        }
                    }
                    Value::Call {
                        callee: Callee::Op { effect, op, .. },
                        args,
                        ..
                    } if flow.core.effects[*effect].ops[*op].type_params > 0 => {
                        for argument in args {
                            if let Some(ty) = operand_type(function, argument) {
                                layouts.add_any_box_type(ty);
                            }
                        }
                        if let Some(destination) = dst {
                            layouts.add_any_box_type(function.vars[*destination].ty);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut function_index = 0;
        let mut list_index = 0;
        let mut cell_index = 0;
        loop {
            if let Some(&ty) = layouts.shapes.function_types.get(function_index) {
                let Type::Fn(ft) = layouts.shapes.types[ty].clone() else {
                    unreachable!()
                };
                function_index += 1;
                for dependency in ft.params.into_iter().chain([ft.ret]) {
                    layouts.collect(dependency, "function value signature", &mut states)?;
                }
            } else if let Some(&ty) = layouts.shapes.list_types.get(list_index) {
                let Type::List(element) = layouts.shapes.types[ty] else {
                    unreachable!()
                };
                list_index += 1;
                layouts.collect(element, "List element", &mut states)?;
            } else if let Some(&ty) = layouts.shapes.cell_types.get(cell_index) {
                let Type::Cell(content) = layouts.shapes.types[ty] else {
                    unreachable!()
                };
                cell_index += 1;
                layouts.collect(content, "Cell content", &mut states)?;
            } else {
                break;
            }
        }
        layouts.runtime.has_error_abi |= layouts.shapes.function_types . iter () . any (| & ty | matches ! (& layouts.shapes.types [ty] , Type :: Fn (function_type) if function_type . fails) ,) ;
        if layouts.runtime.has_error_abi {
            let error_ids : Vec < EnumId > = flow . core . enums . iter () . filter_map (| (id , def) | { (def . error && states . keys () . any (| ty | matches ! (layouts.shapes.types [* ty] , Type :: Enum (used , _) if used == id) ,)) . then_some (id) }) . collect () ;
            for id in error_ids {
                let ty = layouts.shapes.types.enum_type(id, Vec::new());
                layouts.collect(ty, "EmelaError", &mut states)?;
                layouts.runtime.error_types.push((id, ty));
            }
        }
        let error_dependencies = layouts.error_declaration_dependencies();
        for &index in &error_dependencies {
            let aggregate = &layouts.shapes.aggregates[index];
            let variants = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in variants.into_iter().enumerate() {
                for (field, &ty) in fields.iter().enumerate() {
                    if matches!(layouts.shapes.types[ty], Type::Any) {
                        layouts
                            .shapes
                            .recursive_edges
                            .insert((aggregate.ty, variant, field));
                    }
                }
            }
        }
        for aggregate in &layouts.shapes.aggregates {
            let fields = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in fields.into_iter().enumerate() {
                for (field, &field_ty) in fields.iter().enumerate() {
                    if layouts
                        .shapes
                        .recursive_edges
                        .contains(&(aggregate.ty, variant, field))
                        && layouts.has_materialized_payload(field_ty)
                        && !layouts.shapes.recursive_box_ids.contains_key(&field_ty)
                    {
                        let index = layouts.shapes.recursive_box_types.len();
                        layouts.shapes.recursive_box_types.push(field_ty);
                        layouts.shapes.recursive_box_ids.insert(field_ty, index);
                    }
                }
            }
        }
        layouts.runtime.managed_error_payloads = layouts
            .runtime
            .error_types
            .iter()
            .any(|&(_, ty)| layouts.is_managed_value(ty));
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            let mut types = function
                .vars
                .iter()
                .map(|(_, variable)| variable.ty)
                .collect::<Vec<_>>();
            types.push(function.ret);
            types.extend(layouts.shapes.list_types.iter().copied());
            for ty in types {
                if !layouts.contains_heap_ref(ty, &mut HashSet::new()) {
                    continue;
                }
                match layouts.shapes.types[ty].clone() {
                    Type::List(_) => {}
                    Type::Cell(_) => {}
                    Type::Handler(_) | Type::Evidence | Type::FiberScope => {}
                    Type::Fn(_) => {}
                    Type::Any => {}
                    Type::Tuple(_) | Type::Enum(_, _) => {}
                    _ => {
                        return Err(unsupported(
                            &function.name,
                            "managed List reference in an unsupported value type",
                        ));
                    }
                }
            }
        }
        Ok(layouts)
    }

    fn add_any_box_type(&mut self, ty: TypeId) {
        self.runtime.any_abi = true;
        if self.type_repr(ty) == Repr::Void || matches!(self.shapes.types[ty], Type::Any) {
            return;
        }
        self.runtime.any_box_required = true;
        if self.is_runtime_handle(ty) {
            return;
        }
        if !self.shapes.any_box_ids.contains_key(&ty) {
            let index = self.shapes.any_box_types.len();
            self.shapes.any_box_types.push(ty);
            self.shapes.any_box_ids.insert(ty, index);
        }
    }

    fn add_function_adapter(
        &mut self,
        source: TypeId,
        destination: TypeId,
        function: &str,
    ) -> Result<(), Vec<Error>> {
        let (Type::Fn(source_fn), Type::Fn(destination_fn)) = (
            self.shapes.types[source].clone(),
            self.shapes.types[destination].clone(),
        ) else {
            return Err(invalid_flow_ir(
                function,
                "function Coerce has non-function types",
            ));
        };
        if self.shapes.closure_function_types.contains(&source) {
            self.shapes.closure_function_types.insert(destination);
        }
        if source_fn.params == destination_fn.params
            && source_fn.ret == destination_fn.ret
            && source_fn.fails == destination_fn.fails
            && source_fn.suspends == destination_fn.suspends
            && source_fn.ev == destination_fn.ev
        {
            return Ok(());
        }
        let compatible = source_fn.params == destination_fn.params
            && source_fn.ret == destination_fn.ret
            && (!source_fn.suspends || destination_fn.suspends)
            && source_fn.sets.is_none()
            && destination_fn.sets.is_none()
            && (!source_fn.ev || destination_fn.ev);
        if !compatible {
            return Err(unsupported(
                function,
                &format!(
                    "function-value coercion with incompatible parameters, result, or effect flags ({source_fn:?} -> {destination_fn:?})"
                ),
            ));
        }
        if self
            .shapes
            .function_adapter_ids
            .contains_key(&(source, destination))
        {
            return Ok(());
        }
        let index = self.shapes.function_adapters.len();
        self.shapes.function_adapters.push(FunctionAdapterLayout {
            source,
            destination,
        });
        self.shapes
            .function_adapter_ids
            .insert((source, destination), index);
        self.shapes.closure_function_types.insert(destination);
        Ok(())
    }

    fn validate_finite_types(
        &mut self,
        ty: TypeId,
        _function: &str,
        active: &mut HashSet<TypeId>,
        validated: &mut HashSet<TypeId>,
    ) -> Result<(), Vec<Error>> {
        if validated.contains(&ty) || !active.insert(ty) {
            return Ok(());
        }
        let dependencies = match self.shapes.types[ty].clone() {
            Type::Fn(ft) => ft.params.into_iter().chain([ft.ret]).collect(),
            Type::Tuple(fields) => fields,
            Type::List(element) | Type::Cell(element) => vec![element],
            Type::Enum(id, args) if self.enum_layout_required(id) => self.shapes.core.enums[id]
                .variants
                .iter()
                .flat_map(|variant| &variant.tys)
                .map(|&field| self.shapes.types.subst(field, &args))
                .collect(),
            _ => vec![],
        };
        let lazy_fields = matches ! (self.shapes.types [ty] , Type :: Enum (id , _) if self.shapes.expanding_enums . contains (& id));
        for dependency in dependencies {
            if lazy_fields && self.type_repr(dependency) == Repr::Ref {
                continue;
            }
            self.validate_finite_types(dependency, _function, active, validated)?;
        }
        active.remove(&ty);
        validated.insert(ty);
        Ok(())
    }

    fn collect(
        &mut self,
        ty: TypeId,
        function: &str,
        states: &mut HashMap<TypeId, u8>,
    ) -> Result<(), Vec<Error>> {
        if let Type::Enum(id, args) = self.shapes.types[ty].clone()
            && self.shapes.core.enums[id].name == "Std.Fiber.Exit"
            && args
                .iter()
                .any(|&arg| matches!(self.shapes.types[arg], Type::Set(_)))
        {
            let args = args
                .into_iter()
                .map(|arg| {
                    if matches!(self.shapes.types[arg], Type::Set(_)) {
                        TypeId::UNIT
                    } else {
                        arg
                    }
                })
                .collect();
            let canonical = self.shapes.types.enum_type(id, args);
            self.collect(canonical, function, states)?;
            let index = self.shapes.aggregate_ids[&canonical];
            self.shapes.aggregate_ids.insert(ty, index);
            states.insert(ty, 2);
            return Ok(());
        }
        if matches!(self.shapes.types[ty], Type::Cell(_)) {
            if !self.shapes.cell_ids.contains_key(&ty) {
                self.shapes
                    .cell_ids
                    .insert(ty, self.shapes.cell_types.len());
                self.shapes.cell_types.push(ty);
            }
            return Ok(());
        }
        if matches!(self.shapes.types[ty], Type::Fn(_)) {
            if self.shapes.function_types.contains(&ty) {
                return Ok(());
            }
            self.shapes.function_types.push(ty);
            return Ok(());
        }
        if matches!(self.shapes.types[ty], Type::Any) {
            self.runtime.any_abi = true;
            return Ok(());
        }
        if matches!(self.shapes.types[ty], Type::List(_)) {
            if !self.shapes.list_ids.contains_key(&ty) {
                let index = self.shapes.list_types.len();
                self.shapes.list_types.push(ty);
                self.shapes.list_ids.insert(ty, index);
            }
            return Ok(());
        }
        let kind = match self.shapes.types[ty].clone() {
            Type::Tuple(fields) => Some(AggregateKind::Tuple(fields)),
            Type::Enum(id, args) if self.enum_layout_required(id) => {
                let raw_fields: Vec<Vec<TypeId>> = self.shapes.core.enums[id]
                    .variants
                    .iter()
                    .map(crate::semantics::variants::types)
                    .collect();
                let fields = raw_fields
                    .iter()
                    .map(|variant| {
                        variant
                            .iter()
                            .map(|&field| self.shapes.types.subst(field, &args))
                            .collect()
                    })
                    .collect();
                Some(AggregateKind::Enum(fields))
            }
            _ => None,
        };
        let Some(kind) = kind else {
            return Ok(());
        };
        match states.get(&ty) {
            Some(2) => return Ok(()),
            Some(1) => {
                return Err(unsupported(
                    function,
                    &format!("recursive inline aggregate type {}", type_name(ty)),
                ));
            }
            _ => {}
        }
        states.insert(ty, 1);
        let variants = match &kind {
            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
            AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
        };
        for (variant, fields) in variants.into_iter().enumerate() {
            for (index, &field) in fields.iter().enumerate() {
                if self.type_repr(field) == Repr::Void {
                    continue;
                }
                let lazy_field = matches ! (self.shapes.types [ty] , Type :: Enum (id , _) if self.shapes.expanding_enums . contains (& id))
                    && self.type_repr(field) == Repr::Ref;
                if lazy_field || matches!(states.get(&field), Some(1)) {
                    self.shapes.recursive_edges.insert((ty, variant, index));
                } else {
                    self.collect(field, function, states)?;
                }
            }
        }
        let index = self.shapes.aggregates.len();
        self.shapes.aggregate_ids.insert(ty, index);
        self.shapes.aggregates.push(AggregateLayout { ty, kind });
        states.insert(ty, 2);
        Ok(())
    }
}

pub(super) fn freeze(layouts: LayoutBuilder<'_>) -> CLayoutPlan<'_> {
    assert!(layouts.properties.is_none());
    let types = (0..layouts.shapes.types.len())
        .map(|index| {
            let ty = TypeId::from_index(index);
            TypeProperties {
                representation: layouts.type_repr(ty),
                owned: layouts.is_managed_value(ty),
                heap: layouts.contains_heap_ref(ty, &mut HashSet::new()),
                borrowed_views: layouts.has_borrowed_views(ty),
                cell_cycles: layouts.cell_content_can_cycle(ty, &mut HashSet::new()),
            }
        })
        .collect();
    let cell_cycles = layouts.has_cell_cycles();
    let error_declarations = layouts.error_declaration_dependencies();
    layouts.finish(LayoutProperties {
        types,
        cell_cycles,
        error_declarations,
    })
}
