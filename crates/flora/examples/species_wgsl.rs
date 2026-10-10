//! Regenerate the built-in niche/rock table the renderer compiles in:
//! `cargo run -p astrum_flora --example species_wgsl` (from the checkout root).
//! The renderer test `flora_species_table_matches_the_content` fails until
//! this is rerun after a species niche, colour or planet rock changes.

fn main() {
    let species_dir = std::path::Path::new("content/flora/species");
    let planets = std::path::Path::new("content/flora/planets");
    let species = astrum_flora::load_species_for_body(species_dir, planets, "rust").unwrap();
    let planet = astrum_flora::palette::load_planet(planets, "rust").unwrap();
    let out = "crates/renderer/src/shaders/scatter_species.wgsl";
    std::fs::write(out, astrum_flora::scatter::species_wgsl(&species, &planet.rocks)).unwrap();
    println!("wrote {out} ({} species, {} rocks)", species.len(), planet.rocks.len());
}
