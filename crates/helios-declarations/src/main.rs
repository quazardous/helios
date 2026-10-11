//! Prints Helios's declarations as JSON, for `cargo xtask i18n extract`.

fn main() {
    let declarations = helios_declarations::collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&declarations).expect("declarations serialize")
    );
}
