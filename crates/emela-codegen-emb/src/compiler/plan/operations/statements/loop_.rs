use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    _id: StmtId,
    statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let entry = next;
    let Stmt::Loop {
        dst,
        vars,
        inits,
        body,
    } = statement
    else {
        unreachable!("graph dispatch")
    };
    Ok({
        let head = graph.add(Node::Unreachable);
        graph.loops.push(LoopTarget {
            vars: vars.clone(),
            entry: head,
            depth: graph.depth,
        });
        let body = graph.sequence(function, *body, *dst, entry)?;
        graph.loops.pop();
        graph.nodes[head] = Node::Assign(vec![], body);
        graph.add(Node::Assign(
            vars.iter().copied().zip(inits.clone()).collect(),
            head,
        ))
    })
}
