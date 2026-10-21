use emela_codegen_emb::{Options, compile_flow, flow_from_core};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "support/gc_anchor.rs"]
mod gc_anchor;
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let dir = args.next().map(PathBuf::from).expect("output directory");
    let mode = args.next().expect("scalar, managed, or gc");
    let scalar = mode == "scalar";
    let cycles = mode == "gc";
    let source = if scalar {
        "const VALUE = if True { 42 } else { 0 }\npub fn read() -> Int { VALUE }"
    } else {
        include_str!("support/const_runtime.emel")
    };
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut flow = flow_from_core(&core).unwrap();
    if cycles {
        gc_anchor::add(&mut flow);
    }
    let output = compile_flow(
        &flow,
        &Options::default().with_heap_bytes(if scalar { 0 } else { 4096 }),
    )
    .unwrap();
    fs::create_dir_all(&dir).unwrap();
    for mut file in output.files {
        if file.path == "emela_embedded.c" {
            if scalar {
                file.contents
                    .push_str("size_t const_probe_heap(void) { return 0; }\n");
            } else {
                file.contents.push_str("size_t const_probe_heap(void) {\n");
                if cycles {
                    file.contents.push_str("emela_heap_collect_cycles();\n");
                }
                file.contents.push_str("size_t used = 0; if (!emela_heap_initialized) return 0; for (unsigned char *p = emela_heap_storage; p < emela_heap_end();) { emela_heap_block *b = (emela_heap_block *)(void *)p; if (!b->is_free) used += b->size; p += b->size; } return used; }\n");
            }
        }
        fs::write(dir.join(file.path), file.contents).unwrap();
    }
}
