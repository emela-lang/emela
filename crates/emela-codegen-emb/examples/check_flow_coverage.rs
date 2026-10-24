#[path = "support/flow_audit_inventory.rs"]
mod inventory;

fn main() {
    let counts = inventory::check(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("Flow audit inventory must match current IR definitions");
    for (name, count) in counts {
        println!("{name}: {count}");
    }
}
