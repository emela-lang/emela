use emela_core::{External, ExternalBinding, HostRef, Roots};
use emela_lir::Module;

// Source external declarations are nonfailing by language contract. This fixture replaces a
// source-defined failing wrapper with a verified Flow external without changing Core or syntax.
pub fn lower() -> Module {
    let core = super::support::try_lower(
        &[("main.emel", include_str!("external_task_failure.emel"))],
        Roots::Exported,
    )
    .expect("lower task failure fixture");
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
    flow
}
