//! Regenerate a biome LUT PNG from its RON recipe (pipeline §10.3).
//!
//!   cargo run -p astrum_world --example biome_lut [content/lut/terra_whittaker.ron]
//!
//! Writes the PNG named by the metadata, relative to the content root.
use astrum_world::terrain::biome_lut::{LutMetadata, encode_png};
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let metadata_path = std::env::args()
        .nth(1)
        .map_or_else(|| root.join("lut/terra_whittaker.ron"), Into::into);
    let metadata: LutMetadata =
        ron::from_str(&std::fs::read_to_string(&metadata_path).expect("read LUT metadata"))
            .expect("parse LUT metadata");
    let texels = metadata.generate().expect("LUT recipe");
    let size = metadata.recipe.as_ref().expect("recipe").size;
    let png = encode_png(size, &texels).expect("encode PNG");
    let out = root.join(&metadata.image);
    std::fs::write(&out, png).expect("write PNG");
    println!("wrote {} ({size}x{size})", out.display());
}
