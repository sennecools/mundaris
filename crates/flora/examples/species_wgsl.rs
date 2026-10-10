//! Regenerate the built-in niche table the renderer compiles in:
//! `cargo run -p astrum_flora --example species_wgsl` (from the checkout root).
//! The renderer test `flora_species_table` fails until this is rerun after a
//! species niche or colour changes.

fn main() {
    let species = astrum_flora::load_species_for_body(
        std::path::Path::new("content/flora/species"),
        std::path::Path::new("content/flora/planets"),
        "rust",
    )
    .unwrap();
    let out = "crates/renderer/src/shaders/scatter_species.wgsl";
    std::fs::write(out, astrum_flora::scatter::species_wgsl(&species)).unwrap();
    println!("wrote {out} ({} species)", species.len());
}
