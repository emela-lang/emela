use super::*;

#[test]
fn zero_transfer_capacity_is_rejected() {
    let flow = Module::new(emela_core::Module::new());
    emela_codegen_emb::verify_flow(&flow).expect("the options probe is valid Flow IR");

    assert_eq!(Options::default().peripheral_buffer_capacity(), 256);
    for capacity in [1, 256, u16::MAX] {
        let options = Options::default().with_peripheral_buffer_capacity(capacity);
        assert_eq!(options.peripheral_buffer_capacity(), capacity);
        compile_flow(&flow, &options).expect("positive transfer capacities are valid");
    }

    let options = Options::default().with_peripheral_buffer_capacity(0);
    assert_eq!(options.peripheral_buffer_capacity(), 0);
    let errors = compile_flow(&flow, &options).expect_err("zero transfer capacity is invalid");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].kind, ErrorKind::InvalidOptions);
    assert!(
        errors[0].message.contains("peripheral_buffer_capacity")
            && errors[0].message.contains("positive"),
        "{errors:?}"
    );
}

fn scalar_external_flow(params: &[TypeId], ret: TypeId) -> (Module, emela_lir::FnId) {
    let mut flow = Module::new(emela_core::Module::new());
    let mut external = FnBuilder::new(&flow.core, "fixture_external");
    external.set_ret(ret);
    for (index, &ty) in params.iter().enumerate() {
        external.param(&format!("arg{index}"), ty);
    }
    let target = flow.add(external.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("fixture_external".into()),
        }],
    }));

    let mut entry = FnBuilder::new(&flow.core, "work");
    entry.set_exported().set_ret(ret);
    let args = params
        .iter()
        .enumerate()
        .map(|(index, &ty)| entry.param(&format!("arg{index}"), ty).into())
        .collect();
    let value = entry.assign(
        "result",
        ret,
        Value::Call {
            callee: Callee::Direct(target),
            args,
            suspends: false,
            on_fail: None,
        },
    );
    entry.term(Term::Return(Some(value.into())));
    flow.add(entry.finish());
    emela_codegen_emb::verify_flow(&flow).expect("the ordinary scalar external is valid Flow");
    compile_flow(&flow, &Options::default()).expect("the ordinary scalar external compiles");
    (flow, target)
}

fn standard_clock_binding(params: &[TypeId], ret: TypeId, symbol: &str) -> Module {
    let (mut flow, target) = scalar_external_flow(params, ret);
    let function = &mut flow.functions[target];
    function.name = "Std.Embedded.Board.clock_now".into();
    function.external.as_mut().unwrap().bindings[0].host = HostRef::Global(symbol.into());
    // Names and host bindings do not change the well-typed call/result relationship.
    emela_codegen_emb::verify_flow(&flow).expect("the renamed external remains valid Flow");
    flow
}

#[test]
fn malformed_standard_bindings_are_rejected_before_c_emission() {
    const CLOCK_NOW: &str = "__emela_peripheral_clock_now";
    let valid = standard_clock_binding(&[], TypeId::INT64, CLOCK_NOW);
    compile_flow(&valid, &Options::default()).expect("the matching clock binding is accepted");

    for (params, ret, symbol, diagnostic) in [
        (
            &[][..],
            TypeId::INT64,
            "__emela_peripheral_console_print_line",
            "invalid standard board binding identity",
        ),
        (
            &[][..],
            TypeId::INT,
            CLOCK_NOW,
            "invalid standard board binding signature",
        ),
        (
            &[TypeId::INT][..],
            TypeId::INT64,
            CLOCK_NOW,
            "invalid standard board binding signature",
        ),
    ] {
        let flow = standard_clock_binding(params, ret, symbol);
        let errors = compile_flow(&flow, &Options::default())
            .expect_err("malformed standard bindings must not emit C");
        assert!(
            errors.iter().any(|error| {
                error.kind == ErrorKind::Unsupported
                    && error.message.contains("Std.Embedded.Board.clock_now")
                    && error.message.contains(diagnostic)
            }),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|error| error.kind != ErrorKind::InvalidFlowIr),
            "the standard binding contract, rather than Flow verification, rejects this input: {errors:?}"
        );
    }
}
