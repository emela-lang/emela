use super::*;
use emela_core::HostModule;

#[test]
fn imported_c_bindings_preserve_symbols_for_named_file_and_bundled_modules() {
    let mut flow = Module::new(emela_core::Module::new());
    let hosts = [
        HostRef::Global("read_number".into()),
        HostRef::Import {
            module: HostModule::Named("package".into()),
            name: "read_number".into(),
        },
        HostRef::Import {
            module: HostModule::File("binding.c".into()),
            name: "read_number".into(),
        },
        HostRef::Import {
            module: HostModule::Bundled("embedded".into()),
            name: "read_number".into(),
        },
    ];
    for (index, host) in hosts.into_iter().enumerate() {
        let mut external = FnBuilder::new(&flow.core, &format!("binding_{index}"));
        external.set_ret(TypeId::INT);
        let target = flow.add(external.finish_external(External {
            bindings: vec![ExternalBinding {
                target: "c".into(),
                host,
            }],
        }));
        let mut entry = FnBuilder::new(&flow.core, &format!("read_{index}"));
        entry.set_exported().set_ret(TypeId::INT);
        let value = entry.assign(
            "value",
            TypeId::INT,
            Value::Call {
                callee: Callee::Direct(target),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        entry.term(Term::Return(Some(value.into())));
        flow.add(entry.finish());
    }
    let output = compile_flow(&flow, &Options::default()).unwrap();
    let calls = output
        .exports
        .iter()
        .map(|entry| format!("if ({}() != 42) return 1;", entry.c_symbol))
        .collect::<String>();
    compile_and_run(
        output,
        &format!(
            "#include \"emela_embedded.h\"\nint32_t read_number(void) {{ return 42; }}\nint main(void) {{ {calls} return 0; }}"
        ),
    );
}

#[test]
fn named_enum_fields_preserve_payload_order_in_generated_c() {
    let mut core = emela_core::Module::new();
    let mut variant = VariantDef::positional("Pair", vec![TypeId::INT, TypeId::INT64]);
    variant.fields = emela_core::VariantFields::Named(vec!["left".into(), "right".into()]);
    let id = core.add_enum(EnumDef {
        name: "Named".into(),
        params: 0,
        variants: vec![variant],
        error: false,
        external: false,
    });
    let ty = core.enum_type(id, vec![]);
    let mut function = FnBuilder::new(&core, "pair");
    function.set_exported().set_ret(ty);
    let value = function.assign(
        "value",
        ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: id,
                variant: 0,
            },
            args: vec![Const::Int(7).into(), Const::Int64(42).into()],
        },
    );
    function.term(Term::Return(Some(value.into())));
    let function = function.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).unwrap();
    let name = output.exports[0].c_symbol.clone();
    compile_and_run(
        output,
        &format!(
            "#include \"emela_embedded.h\"\nint main(void) {{ emela_type_{} result = {name}(); return result.payload.v0.f0 == 7 && result.payload.v0.f1 == 42 ? 0 : 1; }}",
            ty.index()
        ),
    );
}
