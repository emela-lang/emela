//! Backend-owned resumable control flow. Input remains verified Flow IR.

use super::*;

pub(super) fn scalar(types: &TypeTable, ty: TypeId) -> bool {
    matches!(
        types[ty],
        Type::Int | Type::Int64 | Type::Float | Type::Bool | Type::Unit | Type::Never
    )
}

pub(super) fn validate(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    options: &Options,
) -> Result<bool, Vec<Error>> {
    let mut enabled = has_operations(flow, emitted) || crate::fiber::has_scopes(flow, emitted);
    for (id, function) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
        if !function.suspends {
            continue;
        }
        enabled = true;
        if function.constant {
            return Err(unsupported(&function.name, "const suspend function"));
        }
        for (_, stmt) in function.stmts.iter() {
            if let Stmt::Let { value, .. } = stmt {
                match value {
                    Value::Fiber {
                        op:
                            emela_core::FiberOp::Scope { .. }
                            | emela_core::FiberOp::Fork
                            | emela_core::FiberOp::Deferred,
                        ..
                    } => {}
                    Value::Fiber {
                        op:
                            emela_core::FiberOp::CauseToString
                            | emela_core::FiberOp::CauseKind { .. }
                            | emela_core::FiberOp::Attempt { .. }
                            | emela_core::FiberOp::Complete
                            | emela_core::FiberOp::Poll
                            | emela_core::FiberOp::Join
                            | emela_core::FiberOp::Await { .. }
                            | emela_core::FiberOp::Interrupt
                            | emela_core::FiberOp::DeferredAwait,
                        on_fail: None,
                        ..
                    } => {}
                    Value::Fiber {
                        op: emela_core::FiberOp::CauseRaise,
                        on_fail: Some(_),
                        ..
                    } => {}
                    Value::Fiber { .. } => {
                        return Err(unsupported(
                            &function.name,
                            &format!(
                                "{} in a suspend function (phase 7 follow-up)",
                                value_name(value)
                            ),
                        ));
                    }
                    _ => {}
                }
            }
        }
        // Suspended entry points have an asynchronous start ABI, not a synchronous adapter.
        for (caller_id, caller) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
            if caller_id == id {
                continue;
            }
            if !caller.suspends && directly_calls(caller, id) {
                return Err(unsupported(
                    &caller.name,
                    "suspend call from a synchronous function",
                ));
            }
        }
    }
    if enabled
        && (options.fiber_capacity == 0 || options.frame_capacity == 0 || options.frame_bytes == 0)
    {
        return Err(vec![Error { kind: ErrorKind::InvalidOptions, message: "fiber_capacity, frame_capacity, and frame_bytes must be positive for suspend functions".into() }]);
    }
    if has_tasks(flow, emitted) && (options.task_capacity == 0 || options.event_capacity == 0) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "task_capacity and event_capacity must be positive for suspend external calls"
                .into(),
        }]);
    }
    Ok(enabled)
}

pub(super) fn has_operations(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions.iter().any(|(id, function)| {
        if !emitted.contains(&id) {
            return false;
        }
        let (positions, _) = function_positions(function);
        function.stmts.iter().any(|(id, stmt)| {
            positions.contains_key(&id)
                && matches!(stmt,
            Stmt::Let { value: Value::Call { callee: Callee::Op { effect, op, .. }, .. }, .. }
                if flow.core.effects[*effect].ops[*op].suspend)
        })
    })
}

pub(super) fn has_tasks(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .any(|(id, f)| emitted.contains(&id) && f.suspends && f.external.is_some())
}

pub(super) fn has_failing_tasks(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .any(|(id, f)| emitted.contains(&id) && f.suspends && f.fails && f.external.is_some())
}

pub(super) fn emit_task_failure_ownership(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    writer.begin_block("static void emela_task_own_error(EmelaError *error)");
    writer.begin_block("switch (error->tag)");
    for &(id, ty) in &layouts.error_types {
        writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
        writer.begin_block("");
        let value = format!("error->payload.e{}", enum_index(id));
        let variant = if layouts.aggregate_ids.contains_key(&ty) {
            format!("{value}.tag")
        } else {
            value.clone()
        };
        writer.line(&format!(
            "if ((uint32_t)({variant}) >= UINT32_C({})) emela_scheduler_invalid();",
            layouts.core.enums[id].variants.len()
        ));
        layouts.emit_own_value_in_place(ty, &value, writer);
        writer.line("break;");
        writer.end_block();
    }
    writer.line("default: emela_scheduler_invalid();");
    writer.end_block();
    if layouts.has_error_notes() {
        writer.line("emela_heap_retain(error->notes);");
    }
    writer.end_block();
}

pub(super) fn has_protects(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && function.suspends
            && function
                .stmts
                .iter()
                .any(|(_, stmt)| matches!(stmt, Stmt::Protect { .. }))
    })
}

pub(super) fn external_symbol(
    function: &Function,
    options: &Options,
) -> Result<String, Vec<Error>> {
    if let Some(symbol) = peripheral::task_symbol(function) {
        return Ok(symbol.into());
    }
    let binding = function
        .external
        .as_ref()
        .and_then(|external| external.binding("c"))
        .ok_or_else(|| unsupported(&function.name, "suspend external without C binding"))?;
    let original = match &binding.host {
        emela_core::HostRef::Global(name) | emela_core::HostRef::Import { name, .. } => name,
    };
    let (_, symbol) = options
        .task_adapters
        .iter()
        .find(|(name, _)| name == original)
        .ok_or_else(|| {
            unsupported(
                &function.name,
                "suspend external needs an explicit Options::with_task_adapter mapping",
            )
        })?;
    if !is_c_identifier(symbol) {
        return Err(unsupported(&function.name, "invalid C task adapter symbol"));
    }
    Ok(symbol.clone())
}

pub(super) fn task_signature(layouts: &TypeLayouts<'_>, function: &Function, name: &str) -> String {
    let mut params = vec!["EmelaTask task".to_owned()];
    params.extend(function.params.iter().map(|id| {
        format!(
            "{} {}",
            layouts.c_type(function.vars[*id].ty),
            variable_name(*id)
        )
    }));
    format!("void {name}({})", params.join(", "))
}

// Keep each generated runtime feature explicit at this emission boundary.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_runtime(
    writer: &mut Writer,
    options: &Options,
    tasks: bool,
    resumes: bool,
    unwind: bool,
    scopes: bool,
    deferred: bool,
    failure: bool,
) {
    if failure {
        writer.line("#define EMELA_FAILURE_RUNTIME 1");
    }
    if deferred {
        writer.line(&format!(
            "#define EMELA_DEFERRED_CAPACITY {}",
            options.deferred_capacity
        ));
    }
    if scopes {
        writer.line("#define EMELA_SCOPE_RUNTIME 1");
    }
    if resumes {
        writer.line("#define EMELA_RESUME_RUNTIME 1");
    }
    if unwind {
        writer.line("#define EMELA_UNWIND_RUNTIME 1");
    }
    writer.line(&format!(
        "#define EMELA_FIBER_CAPACITY {}",
        options.fiber_capacity
    ));
    writer.line(&format!(
        "#define EMELA_FRAME_CAPACITY {}",
        options.frame_capacity
    ));
    writer.line(&format!(
        "#define EMELA_FRAME_BYTES {}",
        options.frame_bytes
    ));
    if tasks {
        writer.line(&format!(
            "#define EMELA_TASK_CAPACITY {}",
            options.task_capacity
        ));
        writer.line(&format!(
            "#define EMELA_EVENT_CAPACITY {}",
            options.event_capacity
        ));
    }
    writer.line(include_str!("runtime/scheduler.c"));
    if tasks {
        writer.line(include_str!("runtime/task.c"));
    }
}

fn parameters(layouts: &TypeLayouts<'_>, function: &Function) -> Vec<String> {
    let mut params = function
        .params
        .iter()
        .map(|id| {
            format!(
                "{} {}",
                layouts.c_type(function.vars[*id].ty),
                variable_name(*id)
            )
        })
        .collect::<Vec<_>>();
    if function.fails {
        params.push("EmelaError *emela_error_out".into());
    }
    if layouts.type_repr(function.ret) != Repr::Void {
        params.push(format!("{} *result", layouts.c_type(function.ret)));
    }
    params
}

pub(super) fn start_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
) -> String {
    let mut params = parameters(layouts, function);
    if function.fails {
        params
            .remove(params.len() - 1 - usize::from(layouts.type_repr(function.ret) != Repr::Void));
    }
    format!(
        "EmelaFiber {name}_start({})",
        if params.is_empty() {
            "void".to_owned()
        } else {
            params.join(", ")
        }
    )
}

fn push_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    environment: Option<usize>,
) -> String {
    let mut params = vec!["emela_fiber *fiber".to_owned()];
    if environment.is_some() {
        params.push("void *emela_env".to_owned());
    }
    params.extend(parameters(layouts, function));
    format!("static void {name}_push({})", params.join(", "))
}

pub(super) fn emit_declarations(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    environment: Option<usize>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    writer.line(&format!(
        "{};",
        push_signature(layouts, function, name, environment)
    ));
    Ok(())
}

enum Node {
    Statement(StmtId, usize),
    Assign(Vec<(VarId, Operand)>, usize),
    Branch(Operand, usize, usize),
    Switch(SwitchOn, Vec<(CaseKey, usize)>, Option<usize>),
    Return(Option<Operand>),
    SetError(Operand, usize, usize),
    CheckError {
        normal: usize,
        failed: usize,
        slot: usize,
    },
    Catch(VarId, usize, usize),
    Fail(usize),
    Defect(Operand),
    Unreachable,
    Protect {
        cleanup: usize,
        body: usize,
    },
    Leave {
        keep: usize,
        next: usize,
    },
    CleanupEnd,
}

#[derive(Clone)]
struct LoopTarget {
    vars: Vec<VarId>,
    entry: usize,
    depth: usize,
}

struct Graph {
    nodes: Vec<Node>,
    blocks: HashMap<LabelId, (Option<VarId>, usize, usize)>,
    loops: Vec<LoopTarget>,
    depth: usize,
    max_depth: usize,
    error_depth: usize,
    max_error_depth: usize,
}

impl Graph {
    fn add(&mut self, node: Node) -> usize {
        let slot = match &node {
            Node::SetError(_, _, slot)
            | Node::CheckError { slot, .. }
            | Node::Catch(_, _, slot)
            | Node::Fail(slot) => Some(*slot),
            _ => None,
        };
        if let Some(slot) = slot {
            self.max_error_depth = self.max_error_depth.max(slot);
        }
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    fn assign(&mut self, dst: Option<VarId>, value: &Option<Operand>, next: usize) -> usize {
        let values = dst.zip(value.clone()).into_iter().collect();
        self.add(Node::Assign(values, next))
    }

    fn leave(&mut self, keep: usize, next: usize) -> usize {
        if self.depth > keep {
            self.add(Node::Leave { keep, next })
        } else {
            next
        }
    }

    fn failure(&mut self, function: &Function, edge: OnFail) -> Result<usize, Vec<Error>> {
        match edge {
            OnFail::Propagate => {
                if !function.fails {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "suspend failure without a channel",
                    ));
                }
                let fail = self.add(Node::Fail(self.error_depth));
                Ok(self.leave(0, fail))
            }
            OnFail::Break(label) => {
                let &(dst, next, depth) = self.blocks.get(&label).ok_or_else(|| {
                    invalid_flow_ir(&function.name, "suspend failure without enclosing Block")
                })?;
                let dst = dst.ok_or_else(|| {
                    invalid_flow_ir(
                        &function.name,
                        "suspend failure Block without error destination",
                    )
                })?;
                let catch = self.add(Node::Catch(dst, next, self.error_depth));
                Ok(self.leave(depth, catch))
            }
            OnFail::Handle { .. } => Err(invalid_flow_ir(
                &function.name,
                "Heap IR failure edge in Flow IR",
            )),
        }
    }

    fn sequence(
        &mut self,
        function: &Function,
        seq: SeqId,
        dst: Option<VarId>,
        next: usize,
    ) -> Result<usize, Vec<Error>> {
        let sequence = &function.seqs[seq];
        let mut entry = match &sequence.term {
            Term::Yield(value) => self.assign(dst, value, next),
            Term::Return(value) => {
                let returned = self.add(Node::Return(value.clone()));
                self.leave(0, returned)
            }
            Term::Defect(value) => self.add(Node::Defect(value.clone())),
            Term::Fail(error) => {
                let fail = self.failure(function, OnFail::Propagate)?;
                self.add(Node::SetError(error.clone(), fail, self.error_depth))
            }
            Term::Unreachable => self.add(Node::Unreachable),
            Term::Break { label, value } => {
                let &(dst, next, depth) = self.blocks.get(label).ok_or_else(|| {
                    invalid_flow_ir(&function.name, "suspend Break without enclosing Block")
                })?;
                let assign = self.assign(dst, value, next);
                self.leave(depth, assign)
            }
            Term::Recur(values) => {
                let target = self
                    .loops
                    .last()
                    .ok_or_else(|| {
                        invalid_flow_ir(&function.name, "suspend Recur without enclosing Loop")
                    })?
                    .clone();
                let assign = self.add(Node::Assign(
                    target.vars.into_iter().zip(values.clone()).collect(),
                    target.entry,
                ));
                self.leave(target.depth, assign)
            }
            _ => {
                return Err(unsupported(
                    &function.name,
                    "failure terminator in suspend function (phase 7 follow-up)",
                ));
            }
        };
        for &id in sequence.stmts.iter().rev() {
            entry = match &function.stmts[id] {
                Stmt::Let { value, .. } => {
                    let edge = match value {
                        Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } => *on_fail,
                        _ => None,
                    };
                    let next = if let Some(edge) = edge {
                        let failed = self.failure(function, edge)?;
                        self.add(Node::CheckError {
                            normal: entry,
                            failed,
                            slot: self.error_depth,
                        })
                    } else {
                        entry
                    };
                    self.add(Node::Statement(id, next))
                }
                Stmt::If {
                    dst,
                    cond,
                    then,
                    else_,
                } => {
                    let yes = self.sequence(function, *then, *dst, entry)?;
                    let no = self.sequence(function, *else_, *dst, entry)?;
                    self.add(Node::Branch(cond.clone(), yes, no))
                }
                Stmt::Switch {
                    dst,
                    on,
                    cases,
                    default,
                } => {
                    let cases = cases
                        .iter()
                        .map(|case| {
                            Ok((
                                case.key.clone(),
                                self.sequence(function, case.body, *dst, entry)?,
                            ))
                        })
                        .collect::<Result<_, Vec<Error>>>()?;
                    let default = default
                        .map(|seq| self.sequence(function, seq, *dst, entry))
                        .transpose()?;
                    self.add(Node::Switch(*on, cases, default))
                }
                Stmt::Block { dst, label, body } => {
                    self.blocks.insert(*label, (*dst, entry, self.depth));
                    let body = self.sequence(function, *body, *dst, entry)?;
                    self.blocks.remove(label);
                    body
                }
                Stmt::Loop {
                    dst,
                    vars,
                    inits,
                    body,
                } => {
                    let head = self.add(Node::Unreachable);
                    self.loops.push(LoopTarget {
                        vars: vars.clone(),
                        entry: head,
                        depth: self.depth,
                    });
                    let body = self.sequence(function, *body, *dst, entry)?;
                    self.loops.pop();
                    self.nodes[head] = Node::Assign(vec![], body);
                    self.add(Node::Assign(
                        vars.iter().copied().zip(inits.clone()).collect(),
                        head,
                    ))
                }
                Stmt::Protect { dst, body, cleanup } => {
                    let depth = self.depth;
                    self.depth += 1;
                    self.max_depth = self.max_depth.max(self.depth);
                    let end = self.add(Node::CleanupEnd);
                    self.error_depth += 1;
                    let cleanup = self.sequence(function, *cleanup, None, end)?;
                    self.error_depth -= 1;
                    let leave = self.add(Node::Leave {
                        keep: depth,
                        next: entry,
                    });
                    let body = self.sequence(function, *body, *dst, leave)?;
                    self.depth = depth;
                    self.add(Node::Protect { cleanup, body })
                }
                _ => {
                    return Err(unsupported(
                        &function.name,
                        "unsupported compound suspend statement",
                    ));
                }
            };
        }
        Ok(entry)
    }
}

// The entry combines independent layout, ownership, and runtime plans.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_function(
    flow: &emela_lir::Module,
    layouts: &TypeLayouts<'_>,
    function_id: FnId,
    function: &Function,
    name: &str,
    names: &HashMap<FnId, String>,
    output_calls: &HashSet<FnId>,
    environment: Option<usize>,
    options: &Options,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if function.external.is_some() {
        writer.begin_block(&push_signature(layouts, function, name, None));
        let kind = task_kind(layouts, function.ret, &function.name)?;
        let result = if layouts.type_repr(function.ret) == Repr::Void {
            "NULL"
        } else {
            "result"
        };
        let error = if function.fails {
            "emela_error_out"
        } else {
            "NULL"
        };
        writer.line(&format!(
            "EmelaTask task = emela_task_begin(fiber, {result}, EMELA_TASK_{kind}, {error});"
        ));
        let mut args = vec!["task".to_owned()];
        args.extend(function.params.iter().copied().map(variable_name));
        writer.line(&format!("{name}({});", args.join(", ")));
        writer.end_block();
        return Ok(());
    }
    // A synthetic error destination reuses the synchronous call emitter's validated ABI.
    let mut lowered = function.clone();
    discard_unused_child_handles(layouts, &mut lowered);
    let call_error_var = lowered.vars.alloc(emela_lir::VarData {
        hint: "call_error".into(),
        ty: TypeId::ANY,
        repr: Repr::Ref,
    });
    let call_error_label = lowered.labels.alloc(emela_lir::LabelData {
        hint: "call_error".into(),
    });
    let function = &lowered;
    let mut graph = Graph {
        nodes: vec![],
        blocks: HashMap::new(),
        loops: vec![],
        depth: 0,
        max_depth: 0,
        error_depth: 0,
        max_error_depth: 0,
    };
    let body = function
        .body
        .ok_or_else(|| invalid_flow_ir(&function.name, "suspend function without body"))?;
    let entry = graph.sequence(function, body, None, usize::MAX)?;
    let mut active: HashSet<_> = function.params.iter().copied().collect();
    if let Some(env) = environment {
        active.extend(
            layouts.closure_environments[env]
                .captures
                .iter()
                .map(|c| c.local),
        );
    }
    for node in &graph.nodes {
        let mut operands = Vec::new();
        match node {
            Node::Statement(id, _) => {
                if let Stmt::Let { dst, value, .. } = &function.stmts[*id] {
                    active.extend(*dst);
                    value.for_each_var(|v| {
                        active.insert(v);
                    });
                    if let Value::FnRef(target) = value
                        && let Some(&env) = layouts.closure_functions.get(target)
                    {
                        active.extend(
                            layouts.closure_environments[env]
                                .captures
                                .iter()
                                .map(|c| c.source),
                        );
                    }
                }
            }
            Node::Assign(values, _) => {
                for (v, op) in values {
                    active.insert(*v);
                    operands.push(op);
                }
            }
            Node::Branch(op, _, _) | Node::Defect(op) | Node::SetError(op, _, _) => {
                operands.push(op)
            }
            Node::Switch(SwitchOn::Tag(v) | SwitchOn::Lit(v), ..) => {
                active.insert(*v);
            }
            Node::Return(value) => operands.extend(value),
            Node::Catch(dst, _, _) => {
                active.insert(*dst);
            }
            Node::Unreachable
            | Node::Protect { .. }
            | Node::Leave { .. }
            | Node::CleanupEnd
            | Node::Fail(_)
            | Node::CheckError { .. } => {}
        }
        for op in operands {
            if let Operand::Var(v) = op {
                active.insert(*v);
            }
        }
    }
    let strings = StringBufferPlan::new(
        layouts,
        function_id,
        function,
        options.string_capacity(),
        output_calls,
        false,
    );
    let has_result = layouts.type_repr(function.ret) != Repr::Void;
    writer.line(&format!("typedef struct {name}_frame {{"));
    writer.line("  emela_frame base;");
    let has_failure = function.fails
        || graph
            .nodes
            .iter()
            .any(|n| matches!(n, Node::CheckError { .. }));
    if has_failure {
        writer.line("  EmelaError call_error;");
        writer.line("  int call_error_owned;");
        writer.line(&format!(
            "  EmelaError pending_error[{}];",
            graph.max_error_depth + 1
        ));
    }
    if function.fails {
        writer.line("  EmelaError *error_out;");
    }
    if graph.max_depth != 0 {
        writer.line(&format!("  emela_protection guards[{}];", graph.max_depth));
        writer.line("  uint32_t guard_depth;");
    }
    if has_result {
        writer.line(&format!("  {} *result;", layouts.c_type(function.ret)));
    }
    for (id, var) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        writer.line(&format!(
            "  {} local_{};",
            layouts.c_type(var.ty),
            u32::from(id.into_raw())
        ));
        if layouts.is_managed_value(var.ty) {
            writer.line(&format!("  int owned_{};", u32::from(id.into_raw())));
        }
    }
    for (name, capacity) in &strings.declarations {
        writer.line(&format!("  char {name}[{capacity}];"));
    }
    writer.line(&format!("}} {name}_frame;"));
    writer.line(&format!("_Static_assert(sizeof({name}_frame) <= EMELA_FRAME_BYTES, \"increase Options::with_frame_bytes for {name}\");"));
    if has_failure {
        writer.begin_block(&format!(
            "static int {name}_has_failure(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line("(void)fiber;");
        writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
        writer.line("if (frame->call_error.tag != 0) return 1;");
        writer.line(&format!(
            "for (uint32_t i = 0; i < {}; i++) if (frame->pending_error[i].tag != 0) return 1;",
            graph.max_error_depth + 1
        ));
        writer.line("return 0;");
        writer.end_block();
    }
    if graph.max_depth != 0 {
        writer.begin_block(&format!(
            "static int {name}_unwind(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
        if layouts.has_error_abi {
            writer.line("while (frame->guard_depth != 0 && frame->guards[frame->guard_depth - 1].running) {");
            writer.line("  if (!fiber->forced) emela_scheduler_invalid();");
            writer.line("  fiber->holds = frame->guards[--frame->guard_depth].holds;");
            writer.line("}");
        }
        writer.line("if (frame->guard_depth == 0) return 0;");
        writer.line("emela_protection_leave(fiber, frame->guards, frame->guard_depth, 0, UINT32_MAX, &base->pc);");
        writer.line("return 1;");
        writer.end_block();
    }
    writer.begin_block(&format!(
        "static void {name}_step(emela_fiber *fiber, emela_frame *base)"
    ));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    if has_failure {
        writer.line(&format!(
            "#define {} (frame->call_error)",
            variable_name(call_error_var)
        ));
        writer.line(&format!(
            "#define {} (frame->call_error_owned)",
            owned_variable_name(call_error_var)
        ));
    }
    // Aliases let the shared emitter address persistent fields without stack copies.
    for (id, _) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        writer.line(&format!(
            "#define {} (frame->local_{})",
            variable_name(id),
            u32::from(id.into_raw())
        ));
        let ty = function.vars[id].ty;
        if layouts.is_managed_value(ty) {
            writer.line(&format!(
                "#define {} (frame->owned_{})",
                owned_variable_name(id),
                u32::from(id.into_raw())
            ));
        }
    }
    for (name, _) in &strings.declarations {
        writer.line(&format!("#define {name} (frame->{name})"));
    }
    writer.begin_block("for (;;)");
    if layouts.has_unwind {
        writer.line("if (emela_should_unwind(fiber)) return;");
    }
    writer.begin_block("switch (frame->base.pc)");
    let mut next_temp = 0;
    for (index, node) in graph.nodes.iter().enumerate() {
        writer.begin_block(&format!("case {index}:"));
        match node {
            Node::Statement(id, next) => {
                let statement = &function.stmts[*id];
                let failing = matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Call {
                            on_fail: Some(_),
                            ..
                        } | Value::Fiber {
                            on_fail: Some(_),
                            ..
                        },
                        ..
                    }
                );
                if failing {
                    writer.line("emela_error_clear(&frame->call_error);");
                }
                if let Stmt::Let {
                    dst,
                    value:
                        Value::Fiber {
                            op: emela_core::FiberOp::Attempt { suspends: true, .. },
                            args,
                            ..
                        },
                    ..
                } = statement
                {
                    prepare_result(layouts, function, *dst, writer);
                    writer.line(&format!("base->pc = {next};"));
                    cause::emit_attempt_call(layouts, function, *dst, args, writer)?;
                    writer.line("return;");
                } else if let Stmt::Let {
                    dst,
                    value:
                        Value::Fiber {
                            op: emela_core::FiberOp::DeferredAwait,
                            args,
                            on_fail: None,
                        },
                    ..
                } = statement
                {
                    prepare_result(layouts, function, *dst, writer);
                    writer.line(&format!("frame->base.pc = {next};"));
                    deferred::emit_await(layouts, function, *dst, args, writer)?;
                    writer.line("return;");
                } else if let Stmt::Let {
                    dst,
                    value:
                        Value::Fiber {
                            op:
                                op @ (emela_core::FiberOp::Join
                                | emela_core::FiberOp::Await { .. }
                                | emela_core::FiberOp::Interrupt),
                            args,
                            on_fail: None,
                        },
                    ..
                } = statement
                {
                    prepare_result(layouts, function, *dst, writer);
                    writer.line(&format!("frame->base.pc = {next};"));
                    child::emit_wait(layouts, function, *dst, *op, args, writer)?;
                    writer.line("return;");
                } else if let Stmt::Let {
                    dst,
                    value:
                        Value::Fiber {
                            op: emela_core::FiberOp::Scope { .. },
                            args,
                            on_fail,
                        },
                    ..
                } = statement
                {
                    let [Operand::Var(body)] = args.as_slice() else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "Scope body is not a function value",
                        ));
                    };
                    let Type::Fn(ft) = &layouts.types[function.vars[*body].ty] else {
                        return Err(invalid_flow_ir(&function.name, "Scope body type"));
                    };
                    if ft.ev || !ft.suspends || ft.params != [TypeId::FIBER_SCOPE] {
                        return Err(unsupported(&function.name, "Scope body ABI"));
                    }
                    prepare_result(layouts, function, *dst, writer);
                    writer.line(&format!("frame->base.pc = {next};"));
                    writer.line(&format!(
                        "emela_scope *scope = emela_scope_begin(fiber{});",
                        if layouts.has_error_abi {
                            if on_fail.is_some() {
                                ", &frame->call_error"
                            } else {
                                ", NULL"
                            }
                        } else {
                            ""
                        }
                    ));
                    let body = variable_name(*body);
                    writer.line(&format!(
                        "if ({body}.call == NULL) emela_scheduler_invalid();"
                    ));
                    let mut call_args = vec![
                        "fiber".to_owned(),
                        format!("{body}.env"),
                        "scope".to_owned(),
                    ];
                    if ft.fails {
                        call_args.push("&scope->frame->body_error".into());
                    }
                    if layouts.type_repr(ft.ret) != Repr::Void {
                        call_args.push(
                            dst.map(|v| format!("&{}", variable_name(v)))
                                .unwrap_or_else(|| "NULL".into()),
                        );
                    }
                    writer.line(&format!("{body}.call({}); return;", call_args.join(", ")));
                } else if let Stmt::Let {
                    dst,
                    value:
                        Value::Call {
                            callee:
                                Callee::Op {
                                    handler,
                                    effect,
                                    op,
                                },
                            args,
                            on_fail,
                            ..
                        },
                    ..
                } = statement
                    && layouts.core.effects[*effect].ops[*op].suspend
                {
                    writer.line(&format!("frame->base.pc = {next};"));
                    let result = dst
                        .map(|id| format!("&{}", variable_name(id)))
                        .unwrap_or_else(|| "NULL".to_owned());
                    emit_operation(
                        layouts,
                        function,
                        *dst,
                        *handler,
                        *effect,
                        *op,
                        args,
                        "fiber",
                        &result,
                        if on_fail.is_some() {
                            Some("&frame->call_error")
                        } else {
                            None
                        },
                        writer,
                    )?;
                    writer.begin_block("if (fiber->state == EMELA_FIBER_READY)");
                    writer.line(
                        "emela_ready_remove(fiber); fiber->state = EMELA_FIBER_RUNNING; continue;",
                    );
                    writer.end_block();
                    writer.line("return;");
                } else if let Stmt::Let {
                    dst,
                    value:
                        Value::Call {
                            callee: Callee::Indirect(callee),
                            args,
                            on_fail,
                            ..
                        },
                    ..
                } = statement
                    && matches!(&layouts.types[function.vars[*callee].ty], Type::Fn(ft) if ft.suspends)
                {
                    let Type::Fn(ft) = &layouts.types[function.vars[*callee].ty] else {
                        unreachable!()
                    };
                    prepare_result(layouts, function, *dst, writer);
                    let callee = variable_name(*callee);
                    writer.line(&format!(
                        "if ({callee}.call == NULL) emela_scheduler_invalid();"
                    ));
                    let mut arguments = vec!["fiber".to_owned(), format!("{callee}.env")];
                    arguments.extend(
                        args.iter()
                            .map(operand_expression)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|why| unsupported(&function.name, why))?,
                    );
                    if on_fail.is_some() {
                        arguments.push("&frame->call_error".into());
                    }
                    if layouts.type_repr(ft.ret) != Repr::Void {
                        arguments.push(
                            dst.map(|dst| format!("&{}", variable_name(dst)))
                                .unwrap_or_else(|| "NULL".to_owned()),
                        );
                    }
                    writer.line(&format!("frame->base.pc = {next};"));
                    writer.line(&format!("{callee}.call({}); return;", arguments.join(", ")));
                } else if let Stmt::Let {
                    dst,
                    value:
                        Value::Call {
                            callee: Callee::Direct(target),
                            args,
                            suspends: true,
                            on_fail,
                            ..
                        },
                    ..
                } = statement
                {
                    let target_fn = &flow.functions[*target];
                    if target_fn.suspends
                        && let Some(dst) = dst
                    {
                        emit_release_if_owned(layouts, function, *dst, writer);
                        if layouts.is_managed_value(function.vars[*dst].ty) {
                            writer.assign(
                                &variable_name(*dst),
                                &format!("({}){{0}}", layouts.c_type(function.vars[*dst].ty)),
                            );
                            writer.assign(&owned_variable_name(*dst), "INT32_C(1)");
                        }
                    }
                    if target_fn.suspends {
                        let mut arguments = vec!["fiber".to_owned()];
                        arguments.extend(
                            args.iter()
                                .map(operand_expression)
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|why| unsupported(&function.name, why))?,
                        );
                        if on_fail.is_some() {
                            arguments.push("&frame->call_error".into());
                        }
                        if layouts.type_repr(target_fn.ret) != Repr::Void {
                            arguments.push(
                                dst.map(|dst| format!("&{}", variable_name(dst)))
                                    .unwrap_or_else(|| "NULL".to_owned()),
                            );
                        }
                        writer.line(&format!("frame->base.pc = {next};"));
                        writer.line(&format!(
                            "{}_push({});",
                            names[target],
                            arguments.join(", ")
                        ));
                    } else {
                        let mut statement = statement.clone();
                        if let Stmt::Let {
                            value: Value::Call { suspends, .. },
                            ..
                        } = &mut statement
                        {
                            *suspends = false;
                        }
                        emit_sync_statement(
                            layouts,
                            function,
                            &statement,
                            failing,
                            call_error_var,
                            call_error_label,
                            index,
                            *next,
                            writer,
                            names,
                            &strings,
                            &mut next_temp,
                        )?;
                        writer.line(&format!("frame->base.pc = {next};"));
                    }
                    writer.line("return;");
                } else {
                    emit_sync_statement(
                        layouts,
                        function,
                        statement,
                        failing,
                        call_error_var,
                        call_error_label,
                        index,
                        *next,
                        writer,
                        names,
                        &strings,
                        &mut next_temp,
                    )?;
                }
            }
            Node::Assign(values, next) => {
                // Recur is a parallel assignment: read every old value before overwriting any.
                for (index, (dst, value)) in values.iter().enumerate() {
                    if function.vars[*dst].ty == TypeId::ANY
                        && operand_type(function, value)
                            .is_some_and(|ty| layouts.error_enum(ty).is_some())
                    {
                        writer.decl("EmelaAny", &format!("next_{index}"), Some("{0}"));
                        emit_error_assignment(
                            layouts,
                            function,
                            value,
                            &format!("next_{index}"),
                            writer,
                        )?;
                        continue;
                    }
                    writer.decl(
                        &layouts.c_type(function.vars[*dst].ty),
                        &format!("next_{index}"),
                        Some(
                            &operand_expression(value)
                                .map_err(|why| unsupported(&function.name, why))?,
                        ),
                    );
                    retain_frame_value(
                        layouts,
                        function.vars[*dst].ty,
                        &format!("next_{index}"),
                        writer,
                    );
                }
                for (index, (dst, _)) in values.iter().enumerate() {
                    emit_release_if_owned(layouts, function, *dst, writer);
                    writer.assign(&variable_name(*dst), &format!("next_{index}"));
                    if layouts.is_managed_value(function.vars[*dst].ty) {
                        writer.assign(&owned_variable_name(*dst), "INT32_C(1)");
                    }
                }
                writer.line(&format!("frame->base.pc = {next}; continue;"));
            }
            Node::Branch(cond, yes, no) => {
                writer.line(&format!(
                    "frame->base.pc = {} ? {yes} : {no}; continue;",
                    operand_expression(cond).map_err(|why| unsupported(&function.name, why))?
                ));
            }
            Node::Switch(on, cases, default) => {
                let (scrutinee, tag_switch) = match *on {
                    SwitchOn::Lit(var) => (variable_name(var), false),
                    SwitchOn::Tag(var) => {
                        let ty = function.vars[var].ty;
                        let Type::Enum(id, _) = layouts.types[ty] else {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "tag switch on a non-enum",
                            ));
                        };
                        if layouts.enum_is_i32(id) {
                            (variable_name(var), true)
                        } else if layouts.enum_fields(ty).is_some() {
                            (format!("{}.tag", variable_name(var)), true)
                        } else {
                            return Err(unsupported(
                                &function.name,
                                "suspend switch enum representation",
                            ));
                        }
                    }
                };
                writer.begin_block(&format!("switch ({scrutinee})"));
                for (key, target) in cases {
                    let label = match (key, on) {
                        (CaseKey::Lit(value), SwitchOn::Lit(_)) => constant_expression(value)
                            .map_err(|why| unsupported(&function.name, why))?
                            .ok_or_else(|| invalid_flow_ir(&function.name, "void switch case"))?,
                        (CaseKey::Variant(variant), SwitchOn::Tag(var)) => {
                            let Type::Enum(id, _) = layouts.types[function.vars[*var].ty] else {
                                unreachable!()
                            };
                            if *variant >= layouts.core.enums[id].variants.len() {
                                return Err(invalid_flow_ir(
                                    &function.name,
                                    "switch variant out of range",
                                ));
                            }
                            int32_literal(i32::try_from(*variant).map_err(|_| {
                                invalid_flow_ir(&function.name, "switch variant too large")
                            })?)
                        }
                        _ => return Err(invalid_flow_ir(&function.name, "switch key type")),
                    };
                    writer.line(&format!("case {label}: frame->base.pc = {target}; break;"));
                }
                if let Some(target) = default {
                    writer.line(&format!("default: frame->base.pc = {target}; break;"));
                } else if tag_switch {
                    writer.line("default: emela_board_defect(EMELA_DEFECT_NO_MATCH, \"no match\", UINT16_C(8));");
                } else {
                    writer.line("default: emela_scheduler_invalid();");
                }
                writer.end_block();
                writer.line("continue;");
            }
            Node::Return(value) => {
                if let Some(value) = value {
                    writer.begin_block("if (frame->result != NULL)");
                    let mut expression = operand_expression(value)
                        .map_err(|why| unsupported(&function.name, why))?;
                    if function.ret == TypeId::ANY
                        && matches!(value, Operand::Const(Const::String(s)) if s.is_empty())
                    {
                        expression = "(EmelaAny){0}".into();
                    }
                    writer.decl(&layouts.c_type(function.ret), "returned", Some(&expression));
                    retain_frame_value(layouts, function.ret, "returned", writer);
                    writer.line("*frame->result = returned;");
                    writer.end_block();
                }
                writer.line("emela_frame_return(fiber); return;");
            }
            Node::SetError(error, next, slot) => {
                writer.line(&format!(
                    "emela_error_clear(&frame->pending_error[{slot}]);"
                ));
                emit_error_assignment(
                    layouts,
                    function,
                    error,
                    &format!("frame->pending_error[{slot}]"),
                    writer,
                )?;
                writer.line(&format!("base->pc = {next}; continue;"));
            }
            Node::CheckError {
                normal,
                failed,
                slot,
            } => {
                writer.line(&format!("if (frame->call_error.tag == 0) base->pc = {normal}; else {{ emela_error_clear(&frame->pending_error[{slot}]); frame->pending_error[{slot}] = frame->call_error; frame->call_error = (EmelaError){{0}}; base->pc = {failed}; }} continue;"));
            }
            Node::Catch(dst, next, slot) => {
                if !matches!(layouts.types[function.vars[*dst].ty], Type::Any) {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "failure Block destination is not Any",
                    ));
                }
                emit_release_if_owned(layouts, function, *dst, writer);
                writer.assign(
                    &variable_name(*dst),
                    &format!("frame->pending_error[{slot}]"),
                );
                writer.line(&format!(
                    "frame->pending_error[{slot}] = (EmelaError){{0}};"
                ));
                if layouts.is_managed_value(function.vars[*dst].ty) {
                    writer.assign(&owned_variable_name(*dst), "1");
                }
                writer.line(&format!("base->pc = {next}; continue;"));
            }
            Node::Fail(slot) => {
                writer.line("if (frame->error_out == NULL) emela_board_defect(EMELA_DEFECT_UNHANDLED_ERROR, \"unhandled error\", UINT16_C(15));");
                writer.line(&format!(
                    "*frame->error_out = frame->pending_error[{slot}];"
                ));
                writer.line(&format!(
                    "frame->pending_error[{slot}] = (EmelaError){{0}};"
                ));
                writer.line("emela_frame_return(fiber); return;");
            }
            Node::Defect(value) => {
                emit_return(
                    layouts,
                    function,
                    &Term::Defect(value.clone()),
                    &strings,
                    writer,
                )?;
            }
            Node::Unreachable => writer.line("emela_scheduler_invalid();"),
            Node::Protect { cleanup, body } => {
                writer.line(&format!(
                    "if (frame->guard_depth >= {}) emela_scheduler_invalid();",
                    graph.max_depth
                ));
                writer.line(&format!("frame->guards[frame->guard_depth++] = (emela_protection){{.cleanup = {cleanup}}};"));
                writer.line(&format!("base->pc = {body}; continue;"));
            }
            Node::Leave { keep, next } => {
                writer.line(&format!("emela_protection_leave(fiber, frame->guards, frame->guard_depth, {keep}, {next}, &base->pc);"));
                writer.line("continue;");
            }
            Node::CleanupEnd => {
                writer.line("if (!emela_protection_finish(fiber, frame->guards, &frame->guard_depth, &base->pc)) return;");
                writer.line("continue;");
            }
        }
        writer.end_block();
    }
    writer.line("default: emela_scheduler_invalid();");
    writer.end_block();
    writer.end_block();
    writer.end_block();
    writer.begin_block(&format!("static void {name}_drop(emela_frame *base)"));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    writer.line("(void)frame;");
    if has_failure {
        writer.line("emela_error_clear(&frame->call_error);");
        writer.line(&format!(
            "for (uint32_t i = 0; i < {}; i++) emela_error_clear(&frame->pending_error[i]);",
            graph.max_error_depth + 1
        ));
    }
    for (id, _) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        emit_release_if_owned(layouts, function, id, writer);
    }
    writer.end_block();
    for (id, _) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        writer.line(&format!("#undef {}", variable_name(id)));
        writer.line(&format!("#undef {}", owned_variable_name(id)));
    }
    for (name, _) in &strings.declarations {
        writer.line(&format!("#undef {name}"));
    }
    if has_failure {
        writer.line(&format!("#undef {}", variable_name(call_error_var)));
        writer.line(&format!("#undef {}", owned_variable_name(call_error_var)));
    }
    writer.begin_block(&push_signature(layouts, function, name, environment));
    writer.line(&format!(
        "emela_frame *base = emela_frame_push(fiber, sizeof({name}_frame), {name}_step);"
    ));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    // Clear in place: a struct compound literal can put the entire frame on the C stack at O0.
    writer.line("for (size_t offset = sizeof(emela_frame); offset < sizeof(*frame); offset++) ((unsigned char *)frame)[offset] = 0;");
    writer.line(&format!("frame->base.pc = {entry};"));
    writer.line(&format!("frame->base.drop = {name}_drop;"));
    if has_failure {
        writer.line(&format!("frame->base.has_failure = {name}_has_failure;"));
    }
    if graph.max_depth != 0 {
        writer.line(&format!("frame->base.unwind = {name}_unwind;"));
    }
    if has_result {
        writer.line("frame->result = result;");
    }
    if function.fails {
        writer.line("if (emela_error_out != NULL) *emela_error_out = (EmelaError){0};");
        writer.line("frame->error_out = emela_error_out;");
    }
    for &id in &function.params {
        writer.line(&format!(
            "frame->local_{} = {};",
            u32::from(id.into_raw()),
            variable_name(id)
        ));
        let field = format!("frame->local_{}", u32::from(id.into_raw()));
        retain_frame_value(layouts, function.vars[id].ty, &field, writer);
        if layouts.is_managed_value(function.vars[id].ty) {
            writer.line(&format!(
                "frame->owned_{} = INT32_C(1);",
                u32::from(id.into_raw())
            ));
        }
    }
    if let Some(environment) = environment {
        writer.line("if (emela_env == NULL) emela_scheduler_invalid();");
        for (index, capture) in layouts.closure_environments[environment]
            .captures
            .iter()
            .enumerate()
        {
            if layouts.type_repr(capture.ty) == Repr::Void {
                continue;
            }
            let id = u32::from(capture.local.into_raw());
            writer.line(&format!(
                "frame->local_{id} = ((emela_closure_env_{environment} *)emela_env)->c{index};"
            ));
            retain_frame_value(layouts, capture.ty, &format!("frame->local_{id}"), writer);
            if layouts.is_managed_value(capture.ty) {
                writer.line(&format!("frame->owned_{id} = INT32_C(1);"));
            }
        }
    }
    writer.end_block();
    if function.exported {
        writer.begin_block(&start_signature(layouts, function, name));
        writer.line("EmelaFiber handle = emela_fiber_create();");
        writer.line("emela_fiber *fiber = emela_fiber_get(handle);");
        let mut args = vec!["fiber".to_owned()];
        args.extend(function.params.iter().map(|id| variable_name(*id)));
        if function.fails {
            args.push("NULL".into());
        }
        if has_result {
            args.push("result".to_owned());
        }
        writer.line(&format!("{name}_push({});", args.join(", ")));
        writer.line("emela_ready_push(fiber);");
        writer.line("return handle;");
        writer.end_block();
    }
    Ok(())
}

fn discard_unused_child_handles(layouts: &TypeLayouts<'_>, function: &mut Function) {
    let mut used = HashSet::new();
    for (_, statement) in function.stmts.iter() {
        match statement {
            Stmt::Let { value, .. } => for_each_string_plan_var(layouts, value, |id| {
                used.insert(id);
            }),
            Stmt::If { cond, .. } => count_operand(cond, &mut |id| {
                used.insert(id);
            }),
            Stmt::Switch {
                on: SwitchOn::Tag(id) | SwitchOn::Lit(id),
                ..
            }
            | Stmt::Dup(id)
            | Stmt::Drop(id) => {
                used.insert(*id);
            }
            Stmt::Loop { inits, .. } => {
                for value in inits {
                    count_operand(value, &mut |id| {
                        used.insert(id);
                    });
                }
            }
            Stmt::Block { .. } | Stmt::Protect { .. } => {}
        }
    }
    for (_, sequence) in function.seqs.iter() {
        let mut add = |id| {
            used.insert(id);
        };
        match &sequence.term {
            Term::Return(value) | Term::Yield(value) | Term::Break { value, .. } => {
                if let Some(value) = value {
                    count_operand(value, &mut add);
                }
            }
            Term::Fail(value) | Term::Defect(value) => count_operand(value, &mut add),
            Term::Recur(values) => {
                for value in values {
                    count_operand(value, &mut add);
                }
            }
            Term::Unwind | Term::Unreachable => {}
        }
    }
    for (_, statement) in function.stmts.iter_mut() {
        if let Stmt::Let {
            dst: destination @ Some(_),
            value:
                Value::Call { .. }
                | Value::Fiber {
                    op: emela_core::FiberOp::Fork,
                    ..
                },
            ..
        } = statement
        {
            let id = destination.unwrap();
            if !used.contains(&id)
                && layouts.is_named_external(function.vars[id].ty, "Std.Fiber.Fiber")
            {
                // A source-level discarded handle must not become a persistent frame owner.
                *destination = None;
            }
        }
    }
}

pub(super) fn retain_frame_value(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    value: &str,
    writer: &mut Writer,
) {
    layouts.emit_own_value_in_place(ty, value, writer);
}

fn freeze_statement_result(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    strings: &StringBufferPlan,
    writer: &mut Writer,
) {
    if let Some(dst) = statement.dst() {
        if strings.discarded_results.contains(&dst) {
            // The synchronous emitter has already released the discarded result.
            return;
        }
        layouts.emit_freeze_value_in_place(function.vars[dst].ty, &variable_name(dst), writer);
    }
}

fn task_kind(layouts: &TypeLayouts<'_>, ty: TypeId, _function: &str) -> Result<String, Vec<Error>> {
    Ok(match layouts.types[ty] {
        Type::Int | Type::Bool => "INT".into(),
        Type::Int64 => "INT64".into(),
        Type::Float => "FLOAT".into(),
        Type::Unit | Type::Never => "UNIT".into(),
        _ => format!("VALUE_{}", ty.index()),
    })
}

fn prepare_result(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    writer: &mut Writer,
) {
    if let Some(dst) = dst {
        emit_release_if_owned(layouts, function, dst, writer);
        if layouts.is_managed_value(function.vars[dst].ty) {
            writer.assign(
                &variable_name(dst),
                &format!("({}){{0}}", layouts.c_type(function.vars[dst].ty)),
            );
            writer.assign(&owned_variable_name(dst), "INT32_C(1)");
        }
    }
}

pub(super) fn emit_resume_runtime(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    writer.line(&format!(
        "#define EMELA_RESUME_KIND UINT32_C({})",
        layouts.resume_kind()
    ));
    writer.line(include_str!("runtime/resume.c"));
    if layouts.has_error_ownership() {
        writer.begin_block("static void emela_resume_enqueue_error(void *value, emela_heap_block **pending, emela_heap_block **tail)");
        writer.line("emela_error_enqueue(((emela_resume *)value)->error, pending, tail);");
        writer.end_block();
    }
    for &ty in &layouts.resume_types {
        let Type::Fn(ft) = &layouts.types[ty] else {
            unreachable!()
        };
        writer.begin_block(
            &function_ref_adapter_signature(
                layouts,
                ty,
                &format!("emela_resume_value_{}", ty.index()),
            )
            .replacen("static ", "static inline ", 1),
        );
        writer.line("emela_resume *resume = emela_resume_prepare(emela_env);");
        writer.line("if (resume == NULL) return;");
        let result = ft.params[0];
        if layouts.type_repr(result) != Repr::Void {
            writer.begin_block("if (resume->result != NULL)");
            writer.decl(&layouts.c_type(result), "returned", Some("emela_arg_0"));
            retain_frame_value(layouts, result, "returned", writer);
            writer.line(&format!(
                "*(({} *)resume->result) = returned;",
                layouts.c_type(result)
            ));
            writer.end_block();
        }
        writer.line("emela_resume_commit(resume);");
        writer.end_block();
    }
}

pub(super) fn emit_scope_runtime(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    for (name, kind) in [
        ("SCOPE", layouts.scope_kind()),
        ("CHILD", layouts.child_kind()),
        ("DEFERRED", layouts.deferred_kind()),
    ] {
        writer.line(&format!("#define EMELA_{name}_KIND UINT32_C({kind})"));
    }
    writer.line(include_str!("runtime/scope.c"));
    for &ty in &layouts.fork_types {
        let Type::Fn(ft) = &layouts.types[ty] else {
            return Err(unsupported("Fork", "child is not a function"));
        };
        if ft.ev || !ft.suspends || !ft.params.is_empty() || ft.ret != TypeId::ANY {
            return Err(unsupported(
                "Fork",
                "child must be a suspending fn() -> Any",
            ));
        }
        let index = ty.index();
        let cty = layouts.c_type(ty);
        writer.line(&format!(
            "typedef struct {{ emela_frame base; {cty} body; }} emela_fork_frame_{index};"
        ));
        writer.line(&format!("_Static_assert(sizeof(emela_fork_frame_{index}) <= EMELA_FRAME_BYTES, \"increase frame_bytes for Fork\");"));
        writer.begin_block(&format!(
            "static void emela_fork_drop_{index}(emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_fork_frame_{index} *frame = (emela_fork_frame_{index} *)base;"
        ));
        writer.line("(void)frame;");
        if layouts.is_managed_value(ty) {
            layouts.emit_release_value_expression(ty, "frame->body", writer);
        }
        writer.end_block();
        writer.begin_block(&format!(
            "static void emela_fork_step_{index}(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_fork_frame_{index} *frame = (emela_fork_frame_{index} *)base;"
        ));
        writer.begin_block("if (base->pc == 0)");
        writer.line("base->pc = 1;");
        writer.line("if (frame->body.call == NULL) emela_scheduler_invalid();");
        writer.line(&format!(
            "frame->body.call(fiber, frame->body.env{}, &fiber->child->result);",
            if ft.fails {
                ", &fiber->child->error"
            } else {
                ""
            }
        ));
        writer.line("return;");
        writer.end_block();
        writer.line("emela_frame_return(fiber);");
        writer.end_block();
        writer.begin_block(&format!(
            "static inline EmelaAny emela_fork_{index}(emela_scope *scope, {cty} body)"
        ));
        writer.line("emela_fiber *fiber = emela_child_begin(scope);");
        writer.line(&format!("emela_fork_frame_{index} *frame = (emela_fork_frame_{index} *)emela_frame_push(fiber, sizeof(*frame), emela_fork_step_{index});"));
        writer.line(&format!("frame->base.drop = emela_fork_drop_{index};"));
        if ft.fails {
            writer.line("frame->base.has_failure = emela_child_has_failure;");
        }
        writer.line("frame->body = body;");
        if layouts.is_managed_value(ty) {
            layouts.emit_retain_value_expression(ty, "body", writer);
        }
        writer.line("emela_ready_push(fiber);");
        writer.line("emela_heap_retain(fiber->child);");
        writer.line("return (EmelaAny){.payload.any_box = fiber->child};");
        writer.end_block();
    }
    Ok(())
}

// Forward the operation operands and emission context without changing their ABI.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_fiber_value(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: emela_core::FiberOp,
    args: &[Operand],
    names: &HashMap<FnId, String>,
    strings: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if op == emela_core::FiberOp::CauseToString {
        return cause::emit_to_string(layouts, function, dst, args, names, strings, writer);
    }
    if matches!(op, emela_core::FiberOp::CauseKind { .. }) {
        return cause::emit_kind(function, dst, args, writer);
    }
    if matches!(op, emela_core::FiberOp::Attempt { .. }) {
        return cause::emit_attempt_call(layouts, function, dst, args, writer);
    }
    if op == emela_core::FiberOp::Complete {
        return deferred::emit_complete(layouts, function, dst, args, writer);
    }
    if op == emela_core::FiberOp::Poll {
        return deferred::emit_poll(layouts, function, dst, args, writer);
    }
    let expression = match (op, args) {
        (emela_core::FiberOp::Fork, [scope, Operand::Var(body)]) => format!(
            "emela_fork_{}({}, {})",
            function.vars[*body].ty.index(),
            operand_expression(scope).map_err(|w| unsupported(&function.name, w))?,
            variable_name(*body)
        ),
        (emela_core::FiberOp::Deferred, [scope]) => format!(
            "emela_deferred_create({})",
            operand_expression(scope).map_err(|w| unsupported(&function.name, w))?
        ),
        _ => {
            return Err(unsupported(
                &function.name,
                &format!("Fiber::{} (phase 7 follow-up)", op.name()),
            ));
        }
    };
    if let Some(dst) = dst {
        writer.assign(&variable_name(dst), &expression);
    } else {
        writer.begin_block("");
        writer.line(&format!("EmelaAny discarded = {expression};"));
        writer.line("emela_heap_release(discarded.payload.any_box);");
        writer.end_block();
    }
    Ok(())
}

// The suspension boundary needs both operation state and the caller emission context.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_operation(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    handler: VarId,
    effect: EffectId,
    operation: usize,
    args: &[Operand],
    caller: &str,
    result: &str,
    error_out: Option<&str>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let index = *layouts.handler_layout_ids.get(&effect).ok_or_else(|| {
        invalid_flow_ir(&function.name, "suspend operation without handler layout")
    })?;
    let ty = layouts.handler_layouts[index].operations[operation];
    let Type::Fn(ft) = &layouts.types[ty] else {
        unreachable!()
    };
    if ft.ev || !ft.suspends {
        return Err(unsupported(
            &function.name,
            "suspend operation with failure or unsupported effect ABI",
        ));
    }
    let resume_ty = *ft.params.last().expect("resume parameter");
    let pointer = format!(
        "(({} *){})",
        TypeLayouts::handler_type_name(effect),
        variable_name(handler)
    );
    writer.line(&format!(
        "if ({pointer} == NULL) emela_scheduler_invalid();"
    ));
    writer.line(&format!("if (!{pointer}->active) emela_board_defect(EMELA_DEFECT_PANIC, {pointer}->released_message, {pointer}->released_message_length);"));
    if let Some(dst) = dst {
        prepare_result(layouts, function, Some(dst), writer);
    }
    let error_arg = if layouts.has_error_abi {
        format!(", {}", error_out.unwrap_or("NULL"))
    } else {
        String::new()
    };
    writer.line(&format!("emela_fiber *implementation = emela_operation_begin({caller}, {result}, {pointer}->resume_error_{operation}, {pointer}->resume_error_length_{operation}{error_arg});"));
    let mut arguments = vec![
        "implementation".to_owned(),
        format!("{pointer}->op_{operation}.env"),
    ];
    arguments.extend(
        args.iter()
            .map(operand_expression)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|why| unsupported(&function.name, why))?,
    );
    arguments.push(format!(
        "({}){{emela_resume_value_{}, implementation->perform}}",
        layouts.c_type(resume_ty),
        resume_ty.index()
    ));
    if ft.fails {
        arguments.push("&implementation->perform->error".into());
    }
    writer.line(&format!(
        "if ({pointer}->op_{operation}.call == NULL) emela_scheduler_invalid();"
    ));
    writer.line("emela_fiber *saved_context = emela_current_fiber;");
    writer.line("emela_current_fiber = implementation;");
    writer.line(&format!(
        "{pointer}->op_{operation}.call({});",
        arguments.join(", ")
    ));
    writer.line("emela_current_fiber = saved_context;");
    writer.begin_block(
        "if (implementation->top == NULL && implementation->state == EMELA_FIBER_RUNNING)",
    );
    writer.line("implementation->state = EMELA_FIBER_DONE;");
    writer.line("emela_resume_after_step(implementation);");
    writer.end_block();
    writer.begin_block("else if (implementation->state == EMELA_FIBER_RUNNING)");
    writer.line("emela_ready_push(implementation);");
    writer.end_block();
    Ok(())
}

// The synchronous adapter shares the suspension emission context.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_sync_operation(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    handler: VarId,
    effect: EffectId,
    operation: usize,
    args: &[Operand],
    error_out: Option<&str>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    writer.begin_block("");
    writer.line("EmelaFiber sync_handle = emela_fiber_create();");
    writer.line("emela_fiber *sync_caller = emela_fiber_get(sync_handle);");
    writer.line("if (emela_current_fiber != NULL) sync_caller->root = emela_current_fiber->root;");
    let result = dst
        .map(|id| format!("&{}", variable_name(id)))
        .unwrap_or_else(|| "NULL".to_owned());
    emit_operation(
        layouts,
        function,
        dst,
        handler,
        effect,
        operation,
        args,
        "sync_caller",
        &result,
        error_out,
        writer,
    )?;
    writer.begin_block("if (implementation->state == EMELA_FIBER_READY)");
    writer.line("emela_ready_remove(implementation); emela_resume_drive(implementation);");
    writer.end_block();
    writer.begin_block("if (sync_caller->state != EMELA_FIBER_READY)");
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"immediate operation did not resume\", UINT16_C(34));");
    writer.end_block();
    writer.line("emela_ready_remove(sync_caller); sync_caller->finishing = 1; sync_caller->state = EMELA_FIBER_WAITING;");
    writer.line("emela_unwind_sweep();");
    writer.line("if (sync_caller->state != EMELA_FIBER_DONE) emela_scheduler_invalid();");
    writer.line("emela_fiber_release(sync_handle);");
    writer.end_block();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_sync_statement(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    failing: bool,
    error: VarId,
    label: LabelId,
    index: usize,
    next: usize,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    strings: &StringBufferPlan,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let mut statement = statement.clone();
    if let Stmt::Let {
        value: Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. },
        ..
    } = &mut statement
        && on_fail.is_some()
    {
        *on_fail = Some(OnFail::Break(label));
    }
    let mut blocks = if failing {
        vec![BlockTarget {
            label,
            dst: Some(error),
            exit: format!("emela_call_failed_{index}"),
            used: false,
            protect_depth: 0,
        }]
    } else {
        vec![]
    };
    emit_statement(
        layouts,
        function,
        &statement,
        writer,
        names,
        strings,
        &mut vec![],
        &mut blocks,
        &mut vec![],
        next_temp,
        &mut 0,
    )?;
    freeze_statement_result(layouts, function, &statement, strings, writer);
    writer.line(&format!("frame->base.pc = {next}; continue;"));
    if failing {
        writer.line(&format!(
            "emela_call_failed_{index}: frame->base.pc = {next}; continue;"
        ));
    }
    Ok(())
}
