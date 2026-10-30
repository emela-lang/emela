//! Exhaustive projection from upstream representations to the independent backend contract.
use crate::contract::ir as d;
use la_arena::{Arena, Idx};

pub(super) fn id<T, U>(value: Idx<T>) -> Idx<U> {
    Idx::from_raw(value.into_raw())
}
fn arena<T, U>(input: &Arena<T>, convert: fn(&T) -> U) -> Arena<U> {
    let mut result = Arena::new();
    for (_, value) in input.iter() {
        result.alloc(convert(value));
    }
    result
}

pub(super) fn definitions(source: &emela_core::Module) -> d::Definitions {
    let mut types = d::TypeTable::default();
    for (index, ty) in source.types.iter().enumerate() {
        let actual = types.intern(adapt_type(ty));
        assert_eq!(
            index,
            actual.index(),
            "upstream types must preserve canonical identity"
        );
    }
    d::Definitions {
        types,
        enums: arena(&source.enums, adapt_enum_def),
        effects: arena(&source.effects, adapt_effect_def),
        expanding_enums: crate::input::upstream::type_expansion::expanding_enums(source)
            .into_iter()
            .map(id)
            .collect(),
    }
}

pub(super) fn program(
    source: &emela_lir::Module,
    const_function_groups: &[Vec<emela_lir::FnId>],
) -> d::Program {
    let emela_lir::Module {
        core,
        functions,
        stage: _,
        error_shows,
    } = source;
    d::Program {
        has_scopes: super::fiber_normalize::has_scopes(source, &super::runtime_reachable(source)),
        core: definitions(core),
        functions: arena(functions, adapt_function),
        error_shows: error_shows.iter().map(|(e, f)| (id(*e), id(*f))).collect(),
        const_function_groups: const_function_groups
            .iter()
            .map(|group| group.iter().copied().map(id).collect())
            .collect(),
    }
}

fn adapt_builtin(source: &emela_core::Builtin) -> d::Builtin {
    match source {
        emela_core::Builtin::StringLength => d::Builtin::StringLength,
        emela_core::Builtin::StringByteSize => d::Builtin::StringByteSize,
        emela_core::Builtin::StringAppend => d::Builtin::StringAppend,
        emela_core::Builtin::StringContains => d::Builtin::StringContains,
        emela_core::Builtin::StringStartsWith => d::Builtin::StringStartsWith,
        emela_core::Builtin::StringEndsWith => d::Builtin::StringEndsWith,
        emela_core::Builtin::StringSplit => d::Builtin::StringSplit,
        emela_core::Builtin::StringChars => d::Builtin::StringChars,
        emela_core::Builtin::StringJoin => d::Builtin::StringJoin,
        emela_core::Builtin::StringTrim => d::Builtin::StringTrim,
        emela_core::Builtin::StringCodePoints => d::Builtin::StringCodePoints,
        emela_core::Builtin::StringFromCodePoint => d::Builtin::StringFromCodePoint,
        emela_core::Builtin::ShowNested => d::Builtin::ShowNested,
        emela_core::Builtin::IntToString => d::Builtin::IntToString,
        emela_core::Builtin::IntParse => d::Builtin::IntParse,
        emela_core::Builtin::IntCheckedAdd => d::Builtin::IntCheckedAdd,
        emela_core::Builtin::IntBitAnd => d::Builtin::IntBitAnd,
        emela_core::Builtin::IntBitOr => d::Builtin::IntBitOr,
        emela_core::Builtin::IntBitXor => d::Builtin::IntBitXor,
        emela_core::Builtin::IntBitNot => d::Builtin::IntBitNot,
        emela_core::Builtin::IntShiftLeft => d::Builtin::IntShiftLeft,
        emela_core::Builtin::IntShiftRight => d::Builtin::IntShiftRight,
        emela_core::Builtin::IntShiftRightUnsigned => d::Builtin::IntShiftRightUnsigned,
        emela_core::Builtin::Int64FromInt => d::Builtin::Int64FromInt,
        emela_core::Builtin::Int64ToString => d::Builtin::Int64ToString,
        emela_core::Builtin::Int64ToInt => d::Builtin::Int64ToInt,
        emela_core::Builtin::Int64BitAnd => d::Builtin::Int64BitAnd,
        emela_core::Builtin::Int64BitOr => d::Builtin::Int64BitOr,
        emela_core::Builtin::Int64BitXor => d::Builtin::Int64BitXor,
        emela_core::Builtin::Int64BitNot => d::Builtin::Int64BitNot,
        emela_core::Builtin::Int64ShiftLeft => d::Builtin::Int64ShiftLeft,
        emela_core::Builtin::Int64ShiftRight => d::Builtin::Int64ShiftRight,
        emela_core::Builtin::Int64ShiftRightUnsigned => d::Builtin::Int64ShiftRightUnsigned,
        emela_core::Builtin::FloatToString => d::Builtin::FloatToString,
        emela_core::Builtin::FloatFromInt => d::Builtin::FloatFromInt,
        emela_core::Builtin::FloatParse => d::Builtin::FloatParse,
        emela_core::Builtin::FloatFloor => d::Builtin::FloatFloor,
        emela_core::Builtin::FloatCeil => d::Builtin::FloatCeil,
        emela_core::Builtin::FloatRound => d::Builtin::FloatRound,
        emela_core::Builtin::FloatTruncate => d::Builtin::FloatTruncate,
        emela_core::Builtin::BytesFromList => d::Builtin::BytesFromList,
        emela_core::Builtin::BytesToList => d::Builtin::BytesToList,
        emela_core::Builtin::BytesSize => d::Builtin::BytesSize,
        emela_core::Builtin::BytesGet => d::Builtin::BytesGet,
        emela_core::Builtin::BytesSlice => d::Builtin::BytesSlice,
        emela_core::Builtin::BytesConcat => d::Builtin::BytesConcat,
        emela_core::Builtin::BytesEncodeUtf8 => d::Builtin::BytesEncodeUtf8,
        emela_core::Builtin::BytesDecodeValidUtf8 => d::Builtin::BytesDecodeValidUtf8,
        emela_core::Builtin::Panic => d::Builtin::Panic,
        emela_core::Builtin::Todo => d::Builtin::Todo,
        emela_core::Builtin::Dbg => d::Builtin::Dbg,
        emela_core::Builtin::Inspect => d::Builtin::Inspect,
        emela_core::Builtin::CellNew => d::Builtin::CellNew,
        emela_core::Builtin::CellGet => d::Builtin::CellGet,
        emela_core::Builtin::CellSet => d::Builtin::CellSet,
        emela_core::Builtin::ShowString => d::Builtin::ShowString,
        emela_core::Builtin::DebugPrint => d::Builtin::DebugPrint,
        emela_core::Builtin::AssertFail => d::Builtin::AssertFail,
    }
}

fn adapt_ctor_ref(source: &emela_core::CtorRef) -> d::CtorRef {
    let emela_core::CtorRef { enum_id, variant } = source;
    d::CtorRef {
        enum_id: id(*enum_id),
        variant: *variant,
    }
}

fn adapt_effect_def(source: &emela_core::EffectDef) -> d::EffectDef {
    let emela_core::EffectDef {
        name,
        decl,
        args,
        ops,
        label_of,
    } = source;
    d::EffectDef {
        name: name.clone(),
        decl: decl.clone(),
        args: args
            .iter()
            .map(|value| d::TypeId::from_index(value.index()))
            .collect(),
        ops: ops.iter().map(adapt_op_def).collect(),
        label_of: label_of.as_ref().map(|value| id(*value)),
    }
}

fn adapt_enum_def(source: &emela_core::EnumDef) -> d::EnumDef {
    let emela_core::EnumDef {
        name,
        params,
        variants,
        error,
        external,
    } = source;
    d::EnumDef {
        name: name.clone(),
        params: *params,
        variants: variants.iter().map(adapt_variant_def).collect(),
        error: *error,
        external: *external,
    }
}

fn adapt_external(source: &emela_core::External) -> d::External {
    let emela_core::External { bindings } = source;
    d::External {
        bindings: bindings.iter().map(adapt_external_binding).collect(),
    }
}

fn adapt_external_binding(source: &emela_core::ExternalBinding) -> d::ExternalBinding {
    let emela_core::ExternalBinding { target, host } = source;
    d::ExternalBinding {
        target: target.clone(),
        host: adapt_host_ref(host),
    }
}

fn adapt_fiber_op(source: &emela_core::FiberOp) -> d::FiberOp {
    match source {
        emela_core::FiberOp::Scope { fails } => d::FiberOp::Scope { fails: *fails },
        emela_core::FiberOp::Fork => d::FiberOp::Fork,
        emela_core::FiberOp::Deferred => d::FiberOp::Deferred,
        emela_core::FiberOp::Join => d::FiberOp::Join,
        emela_core::FiberOp::Await { exit } => d::FiberOp::Await { exit: id(*exit) },
        emela_core::FiberOp::Interrupt => d::FiberOp::Interrupt,
        emela_core::FiberOp::Attempt { exit, suspends } => d::FiberOp::Attempt {
            exit: id(*exit),
            suspends: *suspends,
        },
        emela_core::FiberOp::Complete => d::FiberOp::Complete,
        emela_core::FiberOp::DeferredAwait => d::FiberOp::DeferredAwait,
        emela_core::FiberOp::Poll => d::FiberOp::Poll,
        emela_core::FiberOp::CauseKind { kind } => d::FiberOp::CauseKind { kind: id(*kind) },
        emela_core::FiberOp::CauseToString => d::FiberOp::CauseToString,
        emela_core::FiberOp::CauseRaise { .. } => d::FiberOp::CauseRaise,
    }
}

fn adapt_fn_sets(source: &emela_core::FnSets) -> d::FnSets {
    let emela_core::FnSets { errors, effects } = source;
    d::FnSets {
        errors: d::TypeId::from_index(errors.index()),
        effects: d::TypeId::from_index(effects.index()),
    }
}

fn adapt_fn_type(source: &emela_core::FnType) -> d::FnType {
    let emela_core::FnType {
        params,
        ret,
        fails,
        suspends,
        ev,
        sets,
    } = source;
    d::FnType {
        params: params
            .iter()
            .map(|value| d::TypeId::from_index(value.index()))
            .collect(),
        ret: d::TypeId::from_index(ret.index()),
        fails: *fails,
        suspends: *suspends,
        ev: *ev,
        sets: sets.as_ref().map(adapt_fn_sets),
    }
}

fn adapt_host_module(source: &emela_core::HostModule) -> d::HostModule {
    match source {
        emela_core::HostModule::Named(value0) => d::HostModule::Named(value0.clone()),
        emela_core::HostModule::File(value0) => d::HostModule::File(value0.clone()),
        emela_core::HostModule::Bundled(value0) => d::HostModule::Bundled(value0.clone()),
    }
}

fn adapt_host_ref(source: &emela_core::HostRef) -> d::HostRef {
    match source {
        emela_core::HostRef::Global(value0) => d::HostRef::Global(value0.clone()),
        emela_core::HostRef::Import { module, name } => d::HostRef::Import {
            module: adapt_host_module(module),
            name: name.clone(),
        },
    }
}

fn adapt_op_def(source: &emela_core::OpDef) -> d::OpDef {
    let emela_core::OpDef {
        name,
        suspend,
        type_params,
        params,
        ret,
        fails,
    } = source;
    d::OpDef {
        name: name.clone(),
        suspend: *suspend,
        type_params: *type_params,
        params: params
            .iter()
            .map(|value| d::TypeId::from_index(value.index()))
            .collect(),
        ret: d::TypeId::from_index(ret.index()),
        fails: *fails,
    }
}

fn adapt_set_summary(source: &emela_core::SetSummary) -> d::SetSummary {
    let emela_core::SetSummary {
        fails,
        suspends,
        ev,
        params,
    } = source;
    d::SetSummary {
        fails: *fails,
        suspends: *suspends,
        ev: *ev,
        params: params
            .iter()
            .map(|value| d::TypeId::from_index(value.index()))
            .collect(),
    }
}

fn adapt_type(source: &emela_core::Type) -> d::Type {
    match source {
        emela_core::Type::Int => d::Type::Int,
        emela_core::Type::Int64 => d::Type::Int64,
        emela_core::Type::Float => d::Type::Float,
        emela_core::Type::Bool => d::Type::Bool,
        emela_core::Type::String => d::Type::String,
        emela_core::Type::Bytes => d::Type::Bytes,
        emela_core::Type::Unit => d::Type::Unit,
        emela_core::Type::Never => d::Type::Never,
        emela_core::Type::Tuple(value0) => d::Type::Tuple(
            value0
                .iter()
                .map(|value| d::TypeId::from_index(value.index()))
                .collect(),
        ),
        emela_core::Type::List(value0) => d::Type::List(d::TypeId::from_index(value0.index())),
        emela_core::Type::Enum(value0, value1) => d::Type::Enum(
            id(*value0),
            value1
                .iter()
                .map(|value| d::TypeId::from_index(value.index()))
                .collect(),
        ),
        emela_core::Type::Param(value0) => d::Type::Param(*value0),
        emela_core::Type::Fn(value0) => d::Type::Fn(adapt_fn_type(value0)),
        emela_core::Type::Handler(value0) => d::Type::Handler(id(*value0)),
        emela_core::Type::Evidence => d::Type::Evidence,
        emela_core::Type::Cell(value0) => d::Type::Cell(d::TypeId::from_index(value0.index())),
        emela_core::Type::FiberScope => d::Type::FiberScope,
        emela_core::Type::Any => d::Type::Any,
        emela_core::Type::Set(value0) => d::Type::Set(adapt_set_summary(value0)),
    }
}

fn adapt_variant_def(source: &emela_core::VariantDef) -> d::VariantDef {
    let emela_core::VariantDef { name, fields, tys } = source;
    d::VariantDef {
        name: name.clone(),
        fields: adapt_variant_fields(fields),
        tys: tys
            .iter()
            .map(|value| d::TypeId::from_index(value.index()))
            .collect(),
    }
}

fn adapt_variant_fields(source: &emela_core::VariantFields) -> d::VariantFields {
    match source {
        emela_core::VariantFields::Unit => d::VariantFields::Unit,
        emela_core::VariantFields::Positional(value0) => d::VariantFields::Positional(*value0),
        emela_core::VariantFields::Named(value0) => d::VariantFields::Named(value0.to_vec()),
    }
}

fn adapt_callee(source: &emela_lir::Callee) -> d::Callee {
    match source {
        emela_lir::Callee::Direct(value0) => d::Callee::Direct(id(*value0)),
        emela_lir::Callee::Indirect(value0) => d::Callee::Indirect(id(*value0)),
        emela_lir::Callee::Op {
            handler,
            effect,
            op,
        } => d::Callee::Op {
            handler: id(*handler),
            effect: id(*effect),
            op: *op,
        },
    }
}

fn adapt_case(source: &emela_lir::Case) -> d::Case {
    let emela_lir::Case { key, body } = source;
    d::Case {
        key: adapt_case_key(key),
        body: id(*body),
    }
}

fn adapt_case_key(source: &emela_lir::CaseKey) -> d::CaseKey {
    match source {
        emela_lir::CaseKey::Variant(value0) => d::CaseKey::Variant(*value0),
        emela_lir::CaseKey::Lit(value0) => d::CaseKey::Lit(adapt_const(value0)),
    }
}

fn adapt_closure(source: &emela_lir::Closure) -> d::Closure {
    let emela_lir::Closure {
        params,
        captures,
        body,
        suspends,
    } = source;
    d::Closure {
        params: params.iter().map(|value| id(*value)).collect(),
        captures: captures.iter().map(|value| id(*value)).collect(),
        body: id(*body),
        suspends: *suspends,
    }
}

fn adapt_const(source: &emela_lir::Const) -> d::Const {
    match source {
        emela_lir::Const::Int(value0) => d::Const::Int(*value0),
        emela_lir::Const::Int64(value0) => d::Const::Int64(*value0),
        emela_lir::Const::Float(value0) => d::Const::Float(*value0),
        emela_lir::Const::Bool(value0) => d::Const::Bool(*value0),
        emela_lir::Const::String(value0) => d::Const::String(value0.clone()),
        emela_lir::Const::Unit => d::Const::Unit,
    }
}

pub(super) fn adapt_function(source: &emela_lir::Function) -> d::Function {
    let emela_lir::Function {
        name,
        exported,
        params,
        ret,
        fails,
        suspends,
        constant,
        external,
        env,
        defects: _,
        body,
        vars,
        stmts,
        seqs,
        labels,
    } = source;
    d::Function {
        name: name.clone(),
        exported: *exported,
        params: params.iter().map(|value| id(*value)).collect(),
        ret: d::TypeId::from_index(ret.index()),
        fails: *fails,
        suspends: *suspends,
        constant: *constant,
        external: external.as_ref().map(adapt_external),
        env: env.as_ref().map(|value| {
            value
                .iter()
                .map(|value| d::TypeId::from_index(value.index()))
                .collect()
        }),
        body: body.as_ref().map(|value| id(*value)),
        vars: arena(vars, adapt_var_data),
        stmts: arena(stmts, adapt_stmt),
        seqs: arena(seqs, adapt_seq),
        labels: arena(labels, adapt_label_data),
        derivation: crate::input::upstream::derived::kind(source),
        infallible_cause_fail: super::fiber_normalize::is_infallible_cause_fail(source),
        statement_positions: super::function_positions(source)
            .0
            .into_iter()
            .map(|(i, p)| (id(i), p))
            .collect(),
        sequence_positions: super::function_positions(source)
            .1
            .into_iter()
            .map(|(i, p)| (id(i), p))
            .collect(),
        active_statements: super::runtime_statements(source)
            .into_iter()
            .map(id)
            .collect(),
    }
}

fn adapt_label_data(source: &emela_lir::LabelData) -> d::LabelData {
    let emela_lir::LabelData { hint } = source;
    d::LabelData { hint: hint.clone() }
}

fn adapt_on_fail(source: &emela_lir::OnFail) -> d::OnFail {
    match source {
        emela_lir::OnFail::Break(value0) => d::OnFail::Break(id(*value0)),
        emela_lir::OnFail::Propagate => d::OnFail::Propagate,
        emela_lir::OnFail::Handle { error, body } => d::OnFail::Handle {
            error: id(*error),
            body: id(*body),
        },
    }
}

fn adapt_operand(source: &emela_lir::Operand) -> d::Operand {
    match source {
        emela_lir::Operand::Var(value0) => d::Operand::Var(id(*value0)),
        emela_lir::Operand::Const(value0) => d::Operand::Const(adapt_const(value0)),
    }
}

fn adapt_prim_op(source: &emela_lir::PrimOp) -> d::PrimOp {
    match source {
        emela_lir::PrimOp::Add => d::PrimOp::Add,
        emela_lir::PrimOp::Sub => d::PrimOp::Sub,
        emela_lir::PrimOp::Mul => d::PrimOp::Mul,
        emela_lir::PrimOp::Div => d::PrimOp::Div,
        emela_lir::PrimOp::Rem => d::PrimOp::Rem,
        emela_lir::PrimOp::Neg => d::PrimOp::Neg,
        emela_lir::PrimOp::Not => d::PrimOp::Not,
        emela_lir::PrimOp::Eq => d::PrimOp::Eq,
        emela_lir::PrimOp::Ne => d::PrimOp::Ne,
        emela_lir::PrimOp::Lt => d::PrimOp::Lt,
        emela_lir::PrimOp::Le => d::PrimOp::Le,
        emela_lir::PrimOp::Gt => d::PrimOp::Gt,
        emela_lir::PrimOp::Ge => d::PrimOp::Ge,
    }
}

fn adapt_prim_ty(source: &emela_lir::PrimTy) -> d::PrimTy {
    match source {
        emela_lir::PrimTy::I32 => d::PrimTy::I32,
        emela_lir::PrimTy::I64 => d::PrimTy::I64,
        emela_lir::PrimTy::F64 => d::PrimTy::F64,
        emela_lir::PrimTy::String => d::PrimTy::String,
    }
}

fn adapt_runtime_op(source: &emela_lir::RuntimeOp) -> d::RuntimeOp {
    match source {
        emela_lir::RuntimeOp::ShowString => d::RuntimeOp::ShowString,
        emela_lir::RuntimeOp::DebugPrint => d::RuntimeOp::DebugPrint,
        emela_lir::RuntimeOp::Retire => d::RuntimeOp::Retire,
        emela_lir::RuntimeOp::Defect => d::RuntimeOp::Defect,
        emela_lir::RuntimeOp::DefectAt => d::RuntimeOp::DefectAt,
        emela_lir::RuntimeOp::DefectTake => d::RuntimeOp::DefectTake,
        emela_lir::RuntimeOp::DefectResume => d::RuntimeOp::DefectResume,
        emela_lir::RuntimeOp::Hold => d::RuntimeOp::Hold,
        emela_lir::RuntimeOp::Unhold => d::RuntimeOp::Unhold,
        emela_lir::RuntimeOp::Yield => d::RuntimeOp::Yield,
        emela_lir::RuntimeOp::YieldPerform => d::RuntimeOp::YieldPerform,
    }
}

fn adapt_seq(source: &emela_lir::Seq) -> d::Seq {
    let emela_lir::Seq { stmts, term } = source;
    d::Seq {
        stmts: stmts.iter().map(|value| id(*value)).collect(),
        term: adapt_term(term),
    }
}

fn adapt_stmt(source: &emela_lir::Stmt) -> d::Stmt {
    match source {
        emela_lir::Stmt::Let {
            dst,
            value,
            on_defect,
            on_suspend,
        } => d::Stmt::Let {
            dst: dst.as_ref().map(|value| id(*value)),
            value: adapt_value(value),
            on_defect: on_defect.as_ref().map(|value| id(*value)),
            on_suspend: on_suspend.as_ref().map(adapt_suspend),
        },
        emela_lir::Stmt::If {
            dst,
            cond,
            then,
            else_,
        } => d::Stmt::If {
            dst: dst.as_ref().map(|value| id(*value)),
            cond: adapt_operand(cond),
            then: id(*then),
            else_: id(*else_),
        },
        emela_lir::Stmt::Switch {
            dst,
            on,
            cases,
            default,
        } => d::Stmt::Switch {
            dst: dst.as_ref().map(|value| id(*value)),
            on: adapt_switch_on(on),
            cases: cases.iter().map(adapt_case).collect(),
            default: default.as_ref().map(|value| id(*value)),
        },
        emela_lir::Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        } => d::Stmt::Loop {
            dst: dst.as_ref().map(|value| id(*value)),
            vars: vars.iter().map(|value| id(*value)).collect(),
            inits: inits.iter().map(adapt_operand).collect(),
            body: id(*body),
        },
        emela_lir::Stmt::Block { dst, label, body } => d::Stmt::Block {
            dst: dst.as_ref().map(|value| id(*value)),
            label: id(*label),
            body: id(*body),
        },
        emela_lir::Stmt::Protect { dst, body, cleanup } => d::Stmt::Protect {
            dst: dst.as_ref().map(|value| id(*value)),
            body: id(*body),
            cleanup: id(*cleanup),
        },
        emela_lir::Stmt::Dup(value0) => d::Stmt::Dup(id(*value0)),
        emela_lir::Stmt::Drop(value0) => d::Stmt::Drop(id(*value0)),
    }
}

fn adapt_suspend(source: &emela_lir::Suspend) -> d::Suspend {
    let emela_lir::Suspend { state, saved } = source;
    d::Suspend {
        state: *state,
        saved: saved.iter().map(|value| id(*value)).collect(),
    }
}

fn adapt_switch_on(source: &emela_lir::SwitchOn) -> d::SwitchOn {
    match source {
        emela_lir::SwitchOn::Tag(value0) => d::SwitchOn::Tag(id(*value0)),
        emela_lir::SwitchOn::Lit(value0) => d::SwitchOn::Lit(id(*value0)),
    }
}

fn adapt_term(source: &emela_lir::Term) -> d::Term {
    match source {
        emela_lir::Term::Yield(value0) => d::Term::Yield(value0.as_ref().map(adapt_operand)),
        emela_lir::Term::Return(value0) => d::Term::Return(value0.as_ref().map(adapt_operand)),
        emela_lir::Term::Fail(value0) => d::Term::Fail(adapt_operand(value0)),
        emela_lir::Term::Break { label, value } => d::Term::Break {
            label: id(*label),
            value: value.as_ref().map(adapt_operand),
        },
        emela_lir::Term::Recur(value0) => {
            d::Term::Recur(value0.iter().map(adapt_operand).collect())
        }
        emela_lir::Term::Defect(value0, _) => d::Term::Defect(adapt_operand(value0)),
        emela_lir::Term::Unwind => d::Term::Unwind,
        emela_lir::Term::Unreachable => d::Term::Unreachable,
    }
}

fn adapt_value(source: &emela_lir::Value) -> d::Value {
    match source {
        emela_lir::Value::Const(value0) => d::Value::Const(adapt_const(value0)),
        emela_lir::Value::Prim { op, ty, args } => d::Value::Prim {
            op: adapt_prim_op(op),
            ty: adapt_prim_ty(ty),
            args: args.iter().map(adapt_operand).collect(),
        },
        emela_lir::Value::Call {
            callee,
            args,
            suspends,
            on_fail,
        } => d::Value::Call {
            callee: adapt_callee(callee),
            args: args.iter().map(adapt_operand).collect(),
            suspends: *suspends,
            on_fail: on_fail.as_ref().map(adapt_on_fail),
        },
        emela_lir::Value::Builtin { op, args } => d::Value::Builtin {
            op: adapt_builtin(op),
            args: args.iter().map(adapt_operand).collect(),
        },
        emela_lir::Value::Make { ctor, args } => d::Value::Make {
            ctor: adapt_ctor_ref(ctor),
            args: args.iter().map(adapt_operand).collect(),
        },
        emela_lir::Value::Field { base, ctor, index } => d::Value::Field {
            base: id(*base),
            ctor: adapt_ctor_ref(ctor),
            index: *index,
        },
        emela_lir::Value::Tag(value0) => d::Value::Tag(id(*value0)),
        emela_lir::Value::Tuple(value0) => {
            d::Value::Tuple(value0.iter().map(adapt_operand).collect())
        }
        emela_lir::Value::TupleGet { base, index } => d::Value::TupleGet {
            base: id(*base),
            index: *index,
        },
        emela_lir::Value::List { elems, tail } => d::Value::List {
            elems: elems.iter().map(adapt_operand).collect(),
            tail: tail.as_ref().map(adapt_operand),
        },
        emela_lir::Value::ListIsEmpty(value0) => d::Value::ListIsEmpty(id(*value0)),
        emela_lir::Value::ListHead(value0) => d::Value::ListHead(id(*value0)),
        emela_lir::Value::ListTail(value0) => d::Value::ListTail(id(*value0)),
        emela_lir::Value::FnRef(value0) => d::Value::FnRef(id(*value0)),
        emela_lir::Value::Closure(value0) => d::Value::Closure(Box::new(adapt_closure(value0))),
        emela_lir::Value::MakeClosure { function, captures } => d::Value::MakeClosure {
            function: id(*function),
            captures: captures.iter().map(|value| id(*value)).collect(),
        },
        emela_lir::Value::EnvGet { env, index } => d::Value::EnvGet {
            env: id(*env),
            index: *index,
        },
        emela_lir::Value::EvLookup { ev, effect } => d::Value::EvLookup {
            ev: id(*ev),
            effect: id(*effect),
        },
        emela_lir::Value::EvExtend { base, bindings } => d::Value::EvExtend {
            base: base.as_ref().map(|value| id(*value)),
            bindings: bindings
                .iter()
                .map(|value| (id(value.0), id(value.1)))
                .collect(),
        },
        emela_lir::Value::Handler { name, effect, ops } => d::Value::Handler {
            name: name.clone(),
            effect: id(*effect),
            ops: ops.iter().map(|value| id(*value)).collect(),
        },
        emela_lir::Value::Fiber { op, args, on_fail } => d::Value::Fiber {
            op: adapt_fiber_op(op),
            args: args.iter().map(adapt_operand).collect(),
            on_fail: on_fail.as_ref().map(adapt_on_fail),
        },
        emela_lir::Value::IsError { error, enum_id } => d::Value::IsError {
            error: id(*error),
            enum_id: id(*enum_id),
        },
        emela_lir::Value::AsError { error, enum_id } => d::Value::AsError {
            error: id(*error),
            enum_id: id(*enum_id),
        },
        emela_lir::Value::Runtime { op, args } => d::Value::Runtime {
            op: adapt_runtime_op(op),
            args: args.iter().map(adapt_operand).collect(),
        },
        emela_lir::Value::Coerce(value0) => d::Value::Coerce(id(*value0)),
    }
}

fn adapt_var_data(source: &emela_lir::VarData) -> d::VarData {
    let emela_lir::VarData { hint, repr, ty } = source;
    d::VarData {
        hint: hint.clone(),
        repr: adapt_repr(repr),
        ty: d::TypeId::from_index(ty.index()),
    }
}

fn adapt_repr(source: &emela_lir::Repr) -> d::Repr {
    match source {
        emela_lir::Repr::I32 => d::Repr::I32,
        emela_lir::Repr::I64 => d::Repr::I64,
        emela_lir::Repr::F64 => d::Repr::F64,
        emela_lir::Repr::Ref => d::Repr::Ref,
        emela_lir::Repr::Void => d::Repr::Void,
    }
}

pub(super) fn outcome(
    source: &crate::input::upstream::const_eval::Outcome,
) -> crate::contract::lowered::ConstOutcome {
    use crate::contract::lowered::ConstOutcome;
    use crate::input::upstream::const_eval::Outcome;
    match source {
        Outcome::Value(value) => ConstOutcome::Value(adapt_const(value)),
        Outcome::Defect { kind, message } => ConstOutcome::Defect {
            kind,
            message: message.clone(),
        },
        Outcome::SelfReference { message } => ConstOutcome::SelfReference {
            message: message.clone(),
        },
    }
}
