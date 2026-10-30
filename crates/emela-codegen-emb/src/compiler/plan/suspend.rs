use super::*;
pub(super) use crate::contract::control::ControlNode as Node;

#[derive(Clone)]
pub(in crate::compiler::plan) struct LoopTarget {
    pub(in crate::compiler::plan) vars: Vec<VarId>,
    pub(in crate::compiler::plan) entry: usize,
    pub(in crate::compiler::plan) depth: usize,
}

pub(in crate::compiler::plan) struct Graph {
    pub(in crate::compiler::plan) nodes: Vec<Node>,
    pub(in crate::compiler::plan) blocks: HashMap<LabelId, (Option<VarId>, usize, usize)>,
    pub(in crate::compiler::plan) loops: Vec<LoopTarget>,
    pub(in crate::compiler::plan) depth: usize,
    pub(in crate::compiler::plan) max_depth: usize,
    pub(in crate::compiler::plan) error_depth: usize,
    pub(in crate::compiler::plan) max_error_depth: usize,
}

impl Graph {
    pub(in crate::compiler::plan) fn add(&mut self, node: Node) -> usize {
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

    pub(in crate::compiler::plan) fn assign(
        &mut self,
        dst: Option<VarId>,
        value: &Option<Operand>,
        next: usize,
    ) -> usize {
        let values = dst.zip(value.clone()).into_iter().collect();
        self.add(Node::Assign(values, next))
    }

    pub(in crate::compiler::plan) fn leave(&mut self, keep: usize, next: usize) -> usize {
        if self.depth > keep {
            self.add(Node::Leave { keep, next })
        } else {
            next
        }
    }

    pub(in crate::compiler::plan) fn failure(
        &mut self,
        function: &Function,
        edge: OnFail,
    ) -> Result<usize, Vec<Error>> {
        super::features::failures::lower(
            &mut super::features::LoweringContext {
                graph: self,
                function,
            },
            edge,
        )
    }

    pub(in crate::compiler::plan) fn sequence(
        &mut self,
        function: &Function,
        seq: SeqId,
        dst: Option<VarId>,
        next: usize,
    ) -> Result<usize, Vec<Error>> {
        let sequence = &function.seqs[seq];
        let mut entry = super::features::terms::lower(
            &mut super::features::LoweringContext {
                graph: self,
                function,
            },
            &sequence.term,
            dst,
            next,
        )?;
        for &id in sequence.stmts.iter().rev() {
            entry = super::features::statements::lower(
                &mut super::features::LoweringContext {
                    graph: self,
                    function,
                },
                id,
                &function.stmts[id],
                entry,
            )?;
        }
        Ok(entry)
    }
}

pub(in crate::compiler::plan) fn has_failing_tasks(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
) -> bool {
    flow.functions
        .iter()
        .any(|(id, f)| emitted.contains(&id) && f.suspends && f.fails && f.external.is_some())
}

pub(in crate::compiler::plan) fn external_symbol(
    function: &Function,
    options: &Options,
) -> Result<String, Vec<Error>> {
    if let Some(symbol) = super::services::task_symbol(function) {
        return Ok(symbol.into());
    }
    let binding = function
        .external
        .as_ref()
        .and_then(|external| external.binding("c"))
        .ok_or_else(|| unsupported(&function.name, "suspend external without C binding"))?;
    let original = match &binding.host {
        crate::contract::ir::HostRef::Global(name)
        | crate::contract::ir::HostRef::Import { name, .. } => name,
    };
    let (_, symbol) = options
        .task_adapters()
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

pub(in crate::compiler::plan) fn has_protects(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && function.suspends
            && function
                .stmts
                .iter()
                .any(|(_, stmt)| matches!(stmt, Stmt::Protect { .. }))
    })
}

pub(in crate::compiler::plan) fn has_tasks(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
) -> bool {
    flow.functions
        .iter()
        .any(|(id, f)| emitted.contains(&id) && f.suspends && f.external.is_some())
}

pub(in crate::compiler::plan) fn validate(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
    options: &Options,
) -> Result<bool, Vec<Error>> {
    let mut enabled = has_operations(flow, emitted) || flow.has_scopes;
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
                            crate::contract::ir::FiberOp::Scope { .. }
                            | crate::contract::ir::FiberOp::Fork
                            | crate::contract::ir::FiberOp::Deferred,
                        ..
                    } => {}
                    Value::Fiber {
                        op:
                            crate::contract::ir::FiberOp::CauseToString
                            | crate::contract::ir::FiberOp::CauseKind { .. }
                            | crate::contract::ir::FiberOp::Attempt { .. }
                            | crate::contract::ir::FiberOp::Complete
                            | crate::contract::ir::FiberOp::Poll
                            | crate::contract::ir::FiberOp::Join
                            | crate::contract::ir::FiberOp::Await { .. }
                            | crate::contract::ir::FiberOp::Interrupt
                            | crate::contract::ir::FiberOp::DeferredAwait,
                        on_fail: None,
                        ..
                    } => {}
                    Value::Fiber {
                        op: crate::contract::ir::FiberOp::CauseRaise,
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
        && (options.fiber_capacity() == 0
            || options.frame_capacity() == 0
            || options.frame_bytes() == 0)
    {
        return Err(vec![Error { kind: ErrorKind::InvalidOptions, message: "fiber_capacity, frame_capacity, and frame_bytes must be positive for suspend functions".into() }]);
    }
    if has_tasks(flow, emitted) && (options.task_capacity() == 0 || options.event_capacity() == 0) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "task_capacity and event_capacity must be positive for suspend external calls"
                .into(),
        }]);
    }
    Ok(enabled)
}

pub(in crate::compiler::plan) fn has_operations(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
) -> bool {
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

pub(in crate::compiler::plan) use crate::semantics::abi::scalar;

use crate::contract::{
    control::{ControlGraph, SuspensionPlan},
    lowered::SuspensionInput,
};
pub(super) fn prepare_frame<'a>(
    input: &'a SuspensionInput,
    function_id: FnId,
    layouts: &TypeLayouts<'_>,
    output_calls: &HashSet<FnId>,
    environment: Option<usize>,
    options: &Options,
) -> Result<SuspensionPlan<'a>, Vec<Error>> {
    let function = &input.function;
    debug_assert_eq!(function.labels[input.error_label].hint, "call_error");
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
            layouts.functions.closure_environments[env]
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
                        && let Some(&env) = layouts.functions.closure_functions.get(target)
                    {
                        active.extend(
                            layouts.functions.closure_environments[env]
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
    let has_failure = function.fails
        || graph
            .nodes
            .iter()
            .any(|node| matches!(node, Node::CheckError { .. }));
    let statements = function
        .stmts
        .iter()
        .map(|(id, statement)| {
            let mut statement = statement.clone();
            if let Stmt::Let {
                value: Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. },
                ..
            } = &mut statement
                && on_fail.is_some()
            {
                *on_fail = Some(OnFail::Break(input.error_label));
            }
            (id, statement)
        })
        .collect();
    Ok(SuspensionPlan {
        function,
        graph: ControlGraph {
            nodes: graph.nodes,
            max_depth: graph.max_depth,
            max_error_depth: graph.max_error_depth,
            entry,
        },
        active,
        strings,
        has_result,
        has_failure,
        error_variable: input.error_variable,
        error_label: input.error_label,
        statements,
    })
}
