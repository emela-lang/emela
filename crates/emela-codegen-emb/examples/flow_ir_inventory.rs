#[path = "support/feature_inventory.rs"]
mod inventory;

fn main() {
    if let Err(error) = inventory::run(std::env::args().skip(1)) {
        eprintln!("flow-ir-inventory: {error}");
        std::process::exit(1);
    }
}
