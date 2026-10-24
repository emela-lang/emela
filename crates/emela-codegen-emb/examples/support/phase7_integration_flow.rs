use emela_core::{External, ExternalBinding, HostRef, Roots};
use emela_lir::Module;

use super::gc_anchor;

pub fn lower(with_collector: bool) -> Module {
    lower_value(with_collector, false)
}

pub fn lower_managed(with_collector: bool) -> Module {
    lower_value(with_collector, true)
}

fn lower_value(with_collector: bool, managed_success: bool) -> Module {
    let mut source = include_str!("phase7_integration.emel").to_owned();
    if managed_success {
        source = source
            .replace(
                "fn request(n: Int) -> Int fails",
                "fn request(n: Int) -> String fails",
            )
            .replace("state.set(Int.to_string(value))", "state.set(value)");
    }
    let core = super::support::try_lower(&[("main.emel", &source)], Roots::Exported)
        .expect("lower phase 7 integration fixture");
    let mut flow = emela_codegen_emb::flow_from_core(&core).expect("verified source Flow");
    let (_, request) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "request")
        .unwrap();
    assert!(request.suspends && request.fails);
    request.body = None;
    request.external = Some(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_request".into()),
        }],
    });
    if with_collector {
        gc_anchor::add(&mut flow);
    }
    flow
}
