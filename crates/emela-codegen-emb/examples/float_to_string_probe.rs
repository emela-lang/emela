use std::fs;
use std::path::PathBuf;

use emela_codegen_emb::{Options, compile_flow};
use emela_core::{Builtin, TypeId};
use emela_lir::{FnBuilder, Module, Term, Value};

fn main() {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/float-to-string-probe/generated"));
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "float_to_string_probe");
    builder.set_ret(TypeId::STRING).set_exported();
    let value = builder.param("value", TypeId::FLOAT);
    let text = builder.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::FloatToString,
            args: vec![value.into()],
        },
    );
    builder.term(Term::Return(Some(text.into())));
    let function = builder.finish();

    let mut flow = Module::new(core);
    flow.add(function);
    fs::create_dir_all(&output_dir).expect("create generated C directory");
    let output = compile_flow(&flow, &Options::default()).expect("compile Float.to_string probe");
    for file in output.files {
        fs::write(output_dir.join(file.path), file.contents).expect("write generated C file");
    }
}
