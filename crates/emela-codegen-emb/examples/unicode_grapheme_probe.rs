use std::fs;
use std::path::PathBuf;

use emela_codegen_emb::{Options, compile_flow};
use emela_core::{Builtin, Type, TypeId};
use emela_lir::{FnBuilder, Module, Operand, Term, Value};

fn main() {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/unicode-grapheme-probe/generated"));
    let mut core = emela_core::Module::new();
    let list_string = core.types.intern(Type::List(TypeId::STRING));

    let length = {
        let mut builder = FnBuilder::new(&core, "unicode_length_probe");
        builder.set_ret(TypeId::INT).set_exported();
        let input = builder.param("input", TypeId::STRING);
        let result = builder.assign(
            "result",
            TypeId::INT,
            Value::Builtin {
                op: Builtin::StringLength,
                args: vec![input.into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        builder.finish()
    };
    let chars = {
        let mut builder = FnBuilder::new(&core, "unicode_chars_probe");
        builder.set_ret(list_string).set_exported();
        let input = builder.param("input", TypeId::STRING);
        let result = builder.assign(
            "result",
            list_string,
            Value::Builtin {
                op: Builtin::StringChars,
                args: vec![input.into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        builder.finish()
    };
    let split = {
        let mut builder = FnBuilder::new(&core, "unicode_split_probe");
        builder.set_ret(list_string).set_exported();
        let input = builder.param("input", TypeId::STRING);
        let separator = builder.param("separator", TypeId::STRING);
        let result = builder.assign(
            "result",
            list_string,
            Value::Builtin {
                op: Builtin::StringSplit,
                args: vec![input.into(), separator.into()],
            },
        );
        builder.term(Term::Return(Some(Operand::Var(result))));
        builder.finish()
    };

    let mut flow = Module::new(core);
    flow.add(length);
    flow.add(chars);
    flow.add(split);

    fs::create_dir_all(&output_dir).expect("create generated C directory");
    let output = compile_flow(&flow, &Options::default().with_heap_bytes(2048))
        .expect("compile Unicode grapheme probe");
    for file in output.files {
        fs::write(output_dir.join(file.path), file.contents).expect("write generated C file");
    }
}
