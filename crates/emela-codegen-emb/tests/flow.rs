use emela_codegen_emb::{ErrorKind, Options, compile, compile_flow, flow_from_core, verify_flow};
use emela_core::{Builtin, CtorRef, EnumDef, Type, TypeId, VariantDef};
use emela_lir::{
    Callee, Const, FnBuilder, Module, OnFail, RuntimeOp, Stage, Stmt, Suspend, Term, Value,
};

#[test]
fn emits_c_files_for_an_empty_module() {
    let output = compile(&emela_core::Module::new(), &Options::default())
        .expect("an empty Core module should produce an empty C translation unit");

    assert_eq!(output.files.len(), 5);
    assert_eq!(output.files[0].path, "emela_embedded.h");
    assert_eq!(output.files[1].path, "emela_board.h");
    assert!(
        output.files[1]
            .contents
            .contains("EMELA_DEFECT_DIVISION_BY_ZERO")
    );
    assert!(!output.files[1].contents.contains("emela_board_defect"));
    assert_eq!(output.files[2].path, "emela_embedded.c");
    assert!(output.files[2].contents.contains("emela_embedded.h"));
    assert!(output.files[2].contents.contains("emela_board.h"));
}

#[test]
fn lowers_and_verifies_core_as_flow_ir() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    core.define(
        main,
        Vec::new(),
        emela_core::TypeId::UNIT,
        emela_core::Expr::Lit(emela_core::Lit::Unit),
    );

    let flow = flow_from_core(&core).expect("Core should lower to valid Flow IR");

    assert_eq!(flow.stage, emela_lir::Stage::Flow);
    assert!(
        flow.functions
            .iter()
            .any(|(_, function)| function.name == "main")
    );
    verify_flow(&flow).expect("the Flow IR verifier should accept the result");
}

fn rejects_flow(module: &Module, expected: &str) {
    for errors in [
        verify_flow(module).expect_err("reject the non-Flow construct at the public verifier"),
        compile_flow(module, &Options::default())
            .expect_err("reject the non-Flow construct before backend lowering"),
    ] {
        assert!(!errors.is_empty());
        assert!(errors.iter().all(|e| e.kind == ErrorKind::InvalidFlowIr));
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{errors:?}"
        );
    }
}

#[test]
fn rejects_every_heap_stage_before_codegen() {
    for stage in [
        Stage::ClosuresConverted,
        Stage::DefectsExplicit,
        Stage::StateMachines,
        Stage::RefCounted,
    ] {
        let mut module = Module::new(emela_core::Module::new());
        module.stage = stage;
        rejects_flow(&module, "expected Flow IR");
    }
}

#[test]
fn rejects_heap_only_values_and_reference_count_statements_at_flow_stage() {
    for variant in ["MakeClosure", "EnvGet", "Dup", "Drop"] {
        let mut module = Module::new(emela_core::Module::new());
        let mut target = FnBuilder::new(&module.core, "target");
        target.param("env", TypeId::ANY);
        target.term(Term::Return(None));
        let mut target = target.finish();
        target.env = Some(vec![]);
        let target = module.add(target);
        let mut builder = FnBuilder::new(&module.core, variant);
        builder.set_exported();
        let env = builder.param("env", TypeId::ANY);
        match variant {
            "MakeClosure" => builder.let_(
                None,
                Value::MakeClosure {
                    function: target,
                    captures: vec![],
                },
            ),
            "EnvGet" => {
                builder.assign("capture", TypeId::INT, Value::EnvGet { env, index: 0 });
            }
            "Dup" => {
                builder.stmt(Stmt::Dup(env));
            }
            "Drop" => {
                builder.stmt(Stmt::Drop(env));
            }
            _ => unreachable!(),
        }
        builder.term(Term::Return(None));
        let mut function = builder.finish();
        if variant == "EnvGet" {
            function.env = Some(vec![TypeId::INT]);
        }
        module.add(function);
        rejects_flow(
            &module,
            match variant {
                "MakeClosure" => "closure record",
                "EnvGet" => "env_get",
                _ => "dup and drop",
            },
        );
    }
}

#[test]
fn rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage() {
    let mut module = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&module.core, "unwind");
    builder.set_exported();
    builder.term(Term::Unwind);
    let mut function = builder.finish();
    function.defects = true;
    module.add(function);
    rejects_flow(&module, "unwind");

    for suspend in [false, true] {
        let mut module = Module::new(emela_core::Module::new());
        let mut builder = FnBuilder::new(&module.core, "landing");
        builder.set_exported().set_suspends();
        let landing = builder.seq(|b| b.term(Term::Return(None)));
        builder.stmt(Stmt::Let {
            dst: None,
            value: Value::Const(Const::Unit),
            on_defect: (!suspend).then_some(landing),
            on_suspend: suspend.then_some(Suspend {
                state: 0,
                saved: vec![],
            }),
        });
        builder.term(Term::Return(None));
        module.add(builder.finish());
        rejects_flow(
            &module,
            if suspend {
                "suspension edge"
            } else {
                "defect landing"
            },
        );
    }
}

#[test]
fn rejects_heap_only_failure_handle_at_flow_stage() {
    let mut module = Module::new(emela_core::Module::new());
    let mut callee = FnBuilder::new(&module.core, "callee");
    callee.set_fails();
    let error = callee.param("error", TypeId::ANY);
    callee.term(Term::Fail(error.into()));
    let callee = module.add(callee.finish());
    let mut caller = FnBuilder::new(&module.core, "caller");
    caller.set_exported();
    let error = caller.param("error", TypeId::ANY);
    let caught = caller.var("caught", TypeId::ANY);
    let landing = caller.seq(|b| b.term(Term::Return(None)));
    caller.let_(
        None,
        Value::Call {
            callee: Callee::Direct(callee),
            args: vec![error.into()],
            suspends: false,
            on_fail: Some(OnFail::Handle {
                error: caught,
                body: landing,
            }),
        },
    );
    caller.term(Term::Return(None));
    module.add(caller.finish());
    rejects_flow(&module, "failure landing");
}

#[test]
fn rejects_heap_only_runtime_operations_at_flow_stage() {
    for op in [
        RuntimeOp::Defect,
        RuntimeOp::DefectAt,
        RuntimeOp::DefectTake,
        RuntimeOp::DefectResume,
        RuntimeOp::Yield,
        RuntimeOp::YieldPerform,
    ] {
        let mut module = Module::new(emela_core::Module::new());
        let mut builder = FnBuilder::new(&module.core, op.name());
        builder.set_exported().set_suspends();
        let args = match op {
            RuntimeOp::Defect => vec![Const::String("defect".into()).into()],
            RuntimeOp::DefectAt => vec![
                Const::String("defect".into()).into(),
                Const::String("main.emel:1:1".into()).into(),
            ],
            RuntimeOp::DefectResume => vec![builder.param("pending", TypeId::ANY).into()],
            _ => vec![],
        };
        builder.let_(None, Value::Runtime { op, args });
        builder.term(Term::Return(None));
        module.add(builder.finish());
        rejects_flow(
            &module,
            if matches!(op, RuntimeOp::Yield | RuntimeOp::YieldPerform) {
                "fairness yield"
            } else {
                "Heap IR runtime"
            },
        );
    }
}

#[test]
fn rejects_source_builtins_that_must_lower_to_flow_control_or_runtime() {
    for op in [
        Builtin::Panic,
        Builtin::Todo,
        Builtin::AssertFail,
        Builtin::Dbg,
        Builtin::Inspect,
        Builtin::ShowNested,
        Builtin::ShowString,
        Builtin::DebugPrint,
    ] {
        let mut module = Module::new(emela_core::Module::new());
        let mut builder = FnBuilder::new(&module.core, "unlowered_builtin");
        builder.set_exported();
        let args = match op {
            Builtin::Panic => vec![Const::String("panic".into()).into()],
            Builtin::Todo => vec![],
            Builtin::AssertFail => vec![
                Const::String("assert".into()).into(),
                Const::String("main.emel:1:1".into()).into(),
            ],
            Builtin::Dbg | Builtin::Inspect | Builtin::ShowNested => vec![Const::Int(42).into()],
            Builtin::ShowString | Builtin::DebugPrint => {
                vec![Const::String("debug".into()).into()]
            }
            _ => unreachable!(),
        };
        builder.let_(None, Value::Builtin { op, args });
        builder.term(Term::Return(None));
        module.add(builder.finish());
        rejects_flow(&module, "unlowered builtin");
    }
}

#[test]
fn rejects_recur_crossing_protect_and_return_leaving_cleanup() {
    let mut module = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&module.core, "recur_across_protect");
    builder.set_exported();
    let n = builder.var("n", TypeId::INT);
    builder.loop_(None, vec![(n, Const::Int(0).into())], |b| {
        b.protect(None, |b| b.term(Term::Recur(vec![n.into()])), |_| {});
    });
    builder.term(Term::Return(None));
    module.add(builder.finish());
    rejects_flow(
        &module,
        "recur outside a loop of the same body, or across protect",
    );

    let mut module = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&module.core, "return_from_cleanup");
    builder.set_exported();
    builder.protect(None, |_| {}, |b| b.term(Term::Return(None)));
    builder.term(Term::Return(None));
    module.add(builder.finish());
    rejects_flow(&module, "return leaves a cleanup");
}

#[test]
fn isr_entries_reject_reachable_hold_and_unhold_in_direct_and_nested_calls() {
    for op in [RuntimeOp::Hold, RuntimeOp::Unhold] {
        for via_callee in [false, true] {
            for with_unwind in [false, true] {
                let mut module = Module::new(emela_core::Module::new());
                let mut action = FnBuilder::new(&module.core, "action");
                action.set_exported();
                action.let_(None, Value::Runtime { op, args: vec![] });
                action.term(Term::Return(None));
                let action = module.add(action.finish());
                if via_callee {
                    let mut entry = FnBuilder::new(&module.core, "entry");
                    entry.set_exported();
                    entry.let_(
                        None,
                        Value::Call {
                            callee: Callee::Direct(action),
                            args: vec![],
                            suspends: false,
                            on_fail: None,
                        },
                    );
                    entry.term(Term::Return(None));
                    module.add(entry.finish());
                }
                if with_unwind {
                    let mut other = FnBuilder::new(&module.core, "other");
                    other.set_exported().set_suspends();
                    other.protect(None, |_| {}, |_| {});
                    other.term(Term::Return(None));
                    module.add(other.finish());
                }
                verify_flow(&module).unwrap();
                compile_flow(&module, &Options::default())
                    .expect("normal context accepts interrupt controls");
                let entry = if via_callee { "entry" } else { "action" };
                let errors = compile_flow(&module, &Options::default().with_isr_entry(entry))
                    .expect_err("an ISR cannot change a main fiber's interrupt state");
                assert!(
                    errors.iter().any(|e| e.kind == ErrorKind::Unsupported
                        && e.message.contains("fiber interrupt control")
                        && e.message.contains("ISR entry")),
                    "{errors:?}"
                );
            }
        }
    }
}

#[test]
fn isr_entries_reject_discarded_cell_allocations_even_without_cell_variables() {
    for via_callee in [false, true] {
        let mut module = Module::new(emela_core::Module::new());
        let mut action = FnBuilder::new(&module.core, "action");
        action.set_exported();
        action.let_(
            None,
            Value::Builtin {
                op: Builtin::CellNew,
                args: vec![Const::Int(42).into()],
            },
        );
        action.term(Term::Return(None));
        let action = module.add(action.finish());
        assert!(module.functions[action].vars.is_empty());
        if via_callee {
            let mut entry = FnBuilder::new(&module.core, "entry");
            entry.set_exported();
            entry.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(action),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            entry.term(Term::Return(None));
            module.add(entry.finish());
        }
        verify_flow(&module).unwrap();
        compile_flow(&module, &Options::default().with_heap_bytes(256)).unwrap();
        let entry = if via_callee { "entry" } else { "action" };
        let errors = compile_flow(
            &module,
            &Options::default()
                .with_heap_bytes(256)
                .with_isr_entry(entry),
        )
        .expect_err("ISR validation must inspect allocations without a represented destination");
        assert!(
            errors.iter().any(|e| e.kind == ErrorKind::Unsupported
                && e.message.contains("heap-backed")
                && e.message.contains("ISR entry")),
            "{errors:?}"
        );
    }
}

#[test]
fn isr_entries_reject_owned_string_aggregate_producers_and_direct_callers() {
    for is_enum in [false, true] {
        for via_callee in [false, true] {
            let mut module = Module::new(emela_core::Module::new());
            let packed = module.core.add_enum(EnumDef {
                name: "Packed".into(),
                params: 0,
                variants: vec![VariantDef::positional("Packed", vec![TypeId::STRING])],
                error: false,
                external: false,
            });
            let result_type = if is_enum {
                module.core.enum_type(packed, vec![])
            } else {
                module.core.types.intern(Type::Tuple(vec![TypeId::STRING]))
            };
            let mut producer = FnBuilder::new(&module.core, "producer");
            producer.set_exported().set_ret(result_type);
            let n = producer.param("n", TypeId::INT);
            let text = producer.assign(
                "text",
                TypeId::STRING,
                Value::Builtin {
                    op: Builtin::IntToString,
                    args: vec![n.into()],
                },
            );
            let value = if is_enum {
                Value::Make {
                    ctor: CtorRef {
                        enum_id: packed,
                        variant: 0,
                    },
                    args: vec![text.into()],
                }
            } else {
                Value::Tuple(vec![text.into()])
            };
            let result = producer.assign("result", result_type, value);
            producer.term(Term::Return(Some(result.into())));
            let producer = module.add(producer.finish());
            if via_callee {
                let mut entry = FnBuilder::new(&module.core, "entry");
                entry.set_exported();
                let n = entry.param("n", TypeId::INT);
                entry.let_(
                    None,
                    Value::Call {
                        callee: Callee::Direct(producer),
                        args: vec![n.into()],
                        suspends: false,
                        on_fail: None,
                    },
                );
                entry.term(Term::Return(None));
                module.add(entry.finish());
            }
            verify_flow(&module).unwrap();
            compile_flow(&module, &Options::default().with_heap_bytes(1024)).unwrap();
            let entry = if via_callee { "entry" } else { "producer" };
            let errors = compile_flow(
                &module,
                &Options::default()
                    .with_heap_bytes(1024)
                    .with_isr_entry(entry),
            )
            .expect_err("ISR validation must inspect owned backing promotion");
            assert!(
                errors.iter().any(|e| e.kind == ErrorKind::Unsupported
                    && e.message.contains("heap-backed")
                    && e.message.contains("ISR entry")),
                "{errors:?}"
            );
        }
    }
}

#[test]
fn isr_validation_preserves_static_constants_and_heapless_borrowed_string_aggregates() {
    let mut module = Module::new(emela_core::Module::new());
    let mut constant = FnBuilder::new(&module.core, "constant");
    constant.set_exported().set_constant().set_ret(TypeId::INT);
    constant.term(Term::Return(Some(Const::Int(42).into())));
    module.add(constant.finish());
    let tuple = module.core.types.intern(Type::Tuple(vec![TypeId::STRING]));
    let mut borrowed = FnBuilder::new(&module.core, "borrowed");
    borrowed.set_exported().set_ret(tuple);
    let text = borrowed.param("text", TypeId::STRING);
    let value = borrowed.assign("value", tuple, Value::Tuple(vec![text.into()]));
    borrowed.term(Term::Return(Some(value.into())));
    module.add(borrowed.finish());
    let output = compile_flow(
        &module,
        &Options::default()
            .with_isr_entry("constant")
            .with_isr_entry("borrowed"),
    )
    .unwrap();
    assert!(
        output
            .files
            .iter()
            .all(|file| !file.contents.contains("emela_heap_storage"))
    );
}
