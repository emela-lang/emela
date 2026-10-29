//! Generate standalone synchronous external ABI resource probes.
use emela_codegen_emb::{ExternalAdapter, ExternalResult, Options, compile_flow};
use emela_core::{External, ExternalBinding, HostRef, Type, TypeId};
use emela_lir::{FnBuilder, Module};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let directory = args.get(1).expect("usage: external_abi_probe OUTPUT MODE");
    let mode = args.get(2).map(String::as_str).unwrap_or("buffer");
    let mut core = emela_core::Module::new();
    let ty = match mode {
        "scalar" => TypeId::FLOAT,
        "packet" => {
            let list = core.types.intern(Type::List(TypeId::STRING));
            core.types.intern(Type::Tuple(vec![TypeId::STRING, list]))
        }
        "buffer" | "borrowed" | "owned" => TypeId::STRING,
        _ => panic!("mode must be scalar, buffer, borrowed, owned, or packet"),
    };
    let mut flow = Module::new(core);
    let mut builder = FnBuilder::new(&flow.core, "entry");
    builder.set_exported().set_ret(ty);
    builder.param("input", ty);
    flow.add(builder.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_probe".into()),
        }],
    }));
    let result = match mode {
        "buffer" => ExternalResult::StringBuffer,
        "scalar" | "owned" => ExternalResult::Owned,
        _ => ExternalResult::Borrowed,
    };
    let options = Options::default()
        .with_string_capacity(64)
        .with_heap_bytes(if matches!(mode, "borrowed" | "packet") {
            1024
        } else {
            0
        })
        .with_external_adapter("host_probe", ExternalAdapter::new("board_probe", result));
    let output = compile_flow(&flow, &options).expect("compile external ABI probe");
    std::fs::create_dir_all(directory).expect("create output directory");
    for file in output.files {
        std::fs::write(
            std::path::Path::new(directory).join(file.path),
            file.contents,
        )
        .expect("write generated C file");
    }
}
