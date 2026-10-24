use emela_codegen_emb::flow_from_core;
use emela_core::Roots;

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let core = support::try_lower(
        &[("main.emel", include_str!("support/const_polymorphic.emel"))],
        Roots::Exported,
    )
    .expect("lower polymorphic const fixture");
    let flow = flow_from_core(&core).expect("verify polymorphic const Flow");
    for (_, f) in flow.functions.iter() {
        if f.name.starts_with("$emb_const_origins$") {
            println!("{}", f.name);
        }
    }
}
