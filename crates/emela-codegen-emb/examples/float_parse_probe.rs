use std::fs;
use std::path::PathBuf;

use emela_codegen_emb::{Options, compile_flow};
use emela_core::{Builtin, TypeId};
use emela_lir::{FnBuilder, Module, Term, Value};

fn main() {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/float-parse-probe/generated"));
    let mut core = emela_core::Module::new();
    let option = core.option_enum();
    let option_float = core.enum_type(option, vec![TypeId::FLOAT]);
    let mut builder = FnBuilder::new(&core, "float_parse_probe");
    builder.set_ret(option_float).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let result = builder.assign(
        "result",
        option_float,
        Value::Builtin {
            op: Builtin::FloatParse,
            args: vec![input.into()],
        },
    );
    builder.term(Term::Return(Some(result.into())));

    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    fs::create_dir_all(&output_dir).expect("create generated C directory");
    let output = compile_flow(&flow, &Options::default()).expect("compile Float.parse probe");
    for file in output.files {
        fs::write(output_dir.join(file.path), file.contents).expect("write generated C file");
    }
}
