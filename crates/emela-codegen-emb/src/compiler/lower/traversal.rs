//! Shared structural traversal and active runtime statements.
use emela_lir::{Function, SeqId, Stmt, StmtId, Term, Value};
use std::collections::{HashMap, HashSet};

pub(crate) fn function_positions(
    function: &Function,
) -> (HashMap<StmtId, usize>, HashMap<SeqId, usize>) {
    fn visit(
        function: &Function,
        sequence: SeqId,
        next: &mut usize,
        visited: &mut HashSet<SeqId>,
        statement_positions: &mut HashMap<StmtId, usize>,
        sequence_positions: &mut HashMap<SeqId, usize>,
    ) {
        if !visited.insert(sequence) {
            return;
        }
        for &statement_id in &function.seqs[sequence].stmts {
            *next += 1;
            statement_positions.insert(statement_id, *next);
            match &function.stmts[statement_id] {
                Stmt::If { then, else_, .. } => {
                    visit(
                        function,
                        *then,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                    visit(
                        function,
                        *else_,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in cases {
                        visit(
                            function,
                            case.body,
                            next,
                            visited,
                            statement_positions,
                            sequence_positions,
                        );
                    }
                    if let Some(default) = default {
                        visit(
                            function,
                            *default,
                            next,
                            visited,
                            statement_positions,
                            sequence_positions,
                        );
                    }
                }
                Stmt::Loop { body, .. } | Stmt::Block { body, .. } => visit(
                    function,
                    *body,
                    next,
                    visited,
                    statement_positions,
                    sequence_positions,
                ),
                Stmt::Protect { body, cleanup, .. } => {
                    visit(
                        function,
                        *body,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                    visit(
                        function,
                        *cleanup,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                }
                Stmt::Let { .. } | Stmt::Dup(_) | Stmt::Drop(_) => {}
            }
        }
        *next += 1;
        sequence_positions.insert(sequence, *next);
    }

    let mut statement_positions = HashMap::new();
    let mut sequence_positions = HashMap::new();
    let mut visited = HashSet::new();
    let mut next = 0;
    if let Some(body) = function.body {
        visit(
            function,
            body,
            &mut next,
            &mut visited,
            &mut statement_positions,
            &mut sequence_positions,
        );
    }
    (statement_positions, sequence_positions)
}

// A literal terminal defect cannot run cleanup under this backend's immediate-stop policy.
pub(crate) fn runtime_statements(function: &Function) -> HashSet<StmtId> {
    fn visit(
        f: &Function,
        seq: SeqId,
        visited: &mut HashSet<SeqId>,
        result: &mut HashSet<StmtId>,
        suspends: bool,
    ) {
        if !visited.insert(seq) {
            return;
        }
        for &id in &f.seqs[seq].stmts {
            result.insert(id);
            match &f.stmts[id] {
                Stmt::Let {
                    value: Value::Closure(c),
                    ..
                } => visit(f, c.body, visited, result, c.suspends),
                Stmt::If { then, else_, .. } => {
                    visit(f, *then, visited, result, suspends);
                    visit(f, *else_, visited, result, suspends);
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in cases {
                        visit(f, case.body, visited, result, suspends);
                    }
                    if let Some(seq) = default {
                        visit(f, *seq, visited, result, suspends);
                    }
                }
                Stmt::Block { body, .. } | Stmt::Loop { body, .. } => {
                    visit(f, *body, visited, result, suspends)
                }
                Stmt::Protect { body, cleanup, .. } => {
                    visit(f, *body, visited, result, suspends);
                    let body = &f.seqs[*body];
                    if suspends
                        || !body.stmts.is_empty()
                        || !matches!(body.term, Term::Defect(_, _))
                    {
                        visit(f, *cleanup, visited, result, suspends);
                    }
                }
                _ => {}
            }
        }
    }
    let mut result = HashSet::new();
    if let Some(body) = function.body {
        visit(
            function,
            body,
            &mut HashSet::new(),
            &mut result,
            function.suspends,
        );
    }
    result
}
