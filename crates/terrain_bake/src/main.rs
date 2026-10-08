//! `terrain_bake bake <recipe.json> <library_dir>` | `terrain_bake validate <bundle_dir>`

use anyhow::{Result, bail};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, recipe, library] if command == "bake" => {
            let dir =
                mundaris_terrain_bake::bake_and_publish(Path::new(recipe), Path::new(library))?;
            let metadata = mundaris_terrain_bake::bundle::validate(&dir)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "published": dir,
                    "height": metadata.height,
                    "seam": metadata.seam,
                    "bake": metadata.bake,
                    "channels": metadata.channels.iter().map(|c| (&c.file, &c.sha256)).collect::<Vec<_>>(),
                }))?
            );
        }
        [command, bundle] if command == "validate" => {
            let metadata = mundaris_terrain_bake::bundle::validate(Path::new(bundle))?;
            println!(
                "valid {} ({}², {} channels)",
                metadata.bundle_id,
                metadata.resolution,
                metadata.channels.len()
            );
        }
        _ => bail!(
            "usage: terrain_bake bake <recipe.json> <library_dir> | terrain_bake validate <bundle_dir>"
        ),
    }
    Ok(())
}
