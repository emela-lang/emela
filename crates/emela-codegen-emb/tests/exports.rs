use emela_codegen_emb::{Options, compile_flow};
use emela_core::TypeId;
use emela_lir::{Const, FnBuilder, Module, Term};

#[test]
fn export_metadata_matches_public_symbols_and_suspend_start_entries() {
    let mut flow = Module::new(emela_core::Module::new());
    for (name, exported, suspends) in [
        ("hidden", false, false),
        ("answer-value", true, false),
        ("waiting", true, true),
    ] {
        let mut function = FnBuilder::new(&flow.core, name);
        function.set_ret(TypeId::INT);
        if exported {
            function.set_exported();
        }
        if suspends {
            function.set_suspends();
        }
        function.term(Term::Return(Some(Const::Int(42).into())));
        flow.add(function.finish());
    }
    let output = compile_flow(&flow, &Options::default()).expect("emit scalar public entries");
    let header = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.h")
        .unwrap()
        .contents;
    assert_eq!(output.exports.len(), 2);
    let answer = &output.exports[0];
    assert_eq!(answer.source_name, "answer-value");
    assert_eq!(answer.c_symbol, "emela_export_answer_value_1");
    assert!(!answer.suspends);
    assert!(header.contains(&format!("int32_t {}(void);", answer.c_symbol)));
    let waiting = &output.exports[1];
    assert_eq!(waiting.source_name, "waiting");
    assert_eq!(waiting.c_symbol, "emela_export_waiting_2_start");
    assert!(waiting.suspends);
    assert!(header.contains(&format!(
        "EmelaFiber {}(int32_t *result);",
        waiting.c_symbol
    )));
}

#[test]
fn empty_modules_have_no_public_export_metadata() {
    let flow = Module::new(emela_core::Module::new());
    let output = compile_flow(&flow, &Options::default()).expect("emit an empty module");
    assert!(output.exports.is_empty());
}
