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
    let Stmt::Switch {
        dst,
        on,
        cases,
        default,
    } = statement
    else {
        unreachable!("graph dispatch")
    };
    Ok({
        let cases = cases
            .iter()
            .map(|case| {
                Ok((
                    case.key.clone(),
                    graph.sequence(function, case.body, *dst, entry)?,
                ))
            })
            .collect::<Result<_, Vec<Error>>>()?;
        let default = default
            .map(|seq| graph.sequence(function, seq, *dst, entry))
            .transpose()?;
        graph.add(Node::Switch(*on, cases, default))
    })
}

pub(in crate::compiler::plan) fn require_runtime(
    statement: &Stmt,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.defects |= matches!(
        statement,
        Stmt::Switch {
            on: SwitchOn::Tag(_),
            default: None,
            ..
        }
    );
}
