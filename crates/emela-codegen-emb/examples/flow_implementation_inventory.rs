#[allow(dead_code)]
#[path = "support/feature_inventory.rs"]
mod definitions;
#[path = "support/implementation_inventory.rs"]
mod implementation;

fn main() {
    if let Err(error) = implementation::run(std::env::args().skip(1)) {
        eprintln!("flow-implementation-inventory: {error}");
        std::process::exit(1);
    }
}
