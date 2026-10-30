use super::*;

pub(in crate::compiler::plan) fn lower_break(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let OnFail::Break(label) = edge else {
        unreachable!("failure graph entry")
    };
    {
        let &(dst, next, depth) = graph.blocks.get(&label).ok_or_else(|| {
            invalid_flow_ir(&function.name, "suspend failure without enclosing Block")
        })?;
        let dst = dst.ok_or_else(|| {
            invalid_flow_ir(
                &function.name,
                "suspend failure Block without error destination",
            )
        })?;
        let catch = graph.add(Node::Catch(dst, next, graph.error_depth));
        Ok(graph.leave(depth, catch))
    }
}

pub(in crate::compiler::plan) fn lower_propagate(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let OnFail::Propagate = edge else {
        unreachable!("failure graph entry")
    };
    {
        if !function.fails {
            return Err(invalid_flow_ir(
                &function.name,
                "suspend failure without a channel",
            ));
        }
        let fail = graph.add(Node::Fail(graph.error_depth));
        Ok(graph.leave(0, fail))
    }
}

pub(in crate::compiler::plan) fn lower_handle(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    let _graph = &mut *context.graph;
    let function = context.function;
    let OnFail::Handle { .. } = edge else {
        unreachable!("failure graph entry")
    };
    Err(invalid_flow_ir(
        &function.name,
        "Heap IR failure edge in Flow IR",
    ))
}

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    match edge {
        OnFail::Break(..) => lower_break(context, edge),
        OnFail::Propagate => lower_propagate(context, edge),
        OnFail::Handle { .. } => lower_handle(context, edge),
    }
}
