//! The program a frontend checked: the HIR with the type checking results, which lowering and
//! the IDE queries read.

use emela_resolve::hir::Program;
use emela_types::ProgramTypes;

use crate::test_fns::TestDef;

/// Name resolution and type checking results, which lowering reads (see [`ProgramTypes`]).
pub struct TypedProgram {
    pub hir: Program,
    pub types: ProgramTypes,
    /// `@test` functions in source, in module order and then declaration order.
    pub tests: Vec<TestDef>,
}
