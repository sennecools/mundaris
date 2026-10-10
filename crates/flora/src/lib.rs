//! Procedural flora: species genome → grown plant meshes (genesis design §4,
//! flora lane packet `ai/tasks/flora-grower/PLAN.md`).
//!
//! `grow_plant(species, kit, seed)` is pure, deterministic CPU code: the same
//! inputs give the same bytes on every machine (golden-seed test). Growth runs
//! in f64 with a fixed iteration order; randomness is counter-based
//! (`hash.rs`) on the engine's one lattice hash.

#![forbid(unsafe_code)]

pub mod bodyplan;
pub mod genome;
pub mod grow;
pub mod hash;
pub mod kit;
pub mod mesh;
pub mod niche;
pub mod palette;
pub mod metrics;
pub mod params;
pub mod raster;
pub mod rock;
pub mod scatter;
pub mod species_gen;

pub use genome::{Genome, SpeciesFile};
pub use kit::Kit;
pub use mesh::{FloraVertex, LOD_COUNT, Mesh};
pub use metrics::PlantMetrics;

/// One grown individual (a species variant).
#[derive(Debug, Clone, PartialEq)]
pub struct GrownPlant {
    pub skeleton: grow::Skeleton,
    pub lods: [Mesh; LOD_COUNT],
    pub metrics: PlantMetrics,
}

/// Seed of variant `variant` of a species: derived from the species name so
/// renaming a file is the only way to reshuffle it.
pub fn variant_seed(species: &SpeciesFile, variant: u32) -> u64 {
    hash::derive(hash::name_key(&species.name), variant as u64)
}

/// Grow one individual: skeleton and LOD meshes, no scoring (runtime path).
pub fn grow_meshes(
    species: &SpeciesFile,
    kit: &Kit,
    seed: u64,
) -> (grow::Skeleton, [Mesh; LOD_COUNT]) {
    if let Some(plan) = &species.plan {
        return (grow::Skeleton::default(), bodyplan::grow_plan(plan, &species.look, seed));
    }
    let skeleton = grow::grow(&species.genome, seed);
    let lods = mesh::build_lods(&skeleton, species, kit, seed);
    (skeleton, lods)
}

/// Grow one individual and score it.
pub fn grow_plant(species: &SpeciesFile, kit: &Kit, seed: u64) -> GrownPlant {
    let (skeleton, lods) = grow_meshes(species, kit, seed);
    let metrics = PlantMetrics {
        structure: metrics::structure(&skeleton),
        silhouette: metrics::silhouette(&lods[0]),
        triangles: std::array::from_fn(|l| lods[l].triangles()),
        vertices: std::array::from_fn(|l| lods[l].vertices.len()),
        bytes: std::array::from_fn(|l| lods[l].bytes()),
    };
    GrownPlant {
        skeleton,
        lods,
        metrics,
    }
}

/// Load every `*.ron` species in a directory (skips `kit.ron`), sorted by
/// file name.
pub fn load_species_dir(dir: &std::path::Path) -> Result<Vec<SpeciesFile>, String> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == "ron")
                && p.file_name().is_some_and(|n| n != "kit.ron")
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            SpeciesFile::from_ron(&text).map_err(|e| format!("{}: {e}", p.display()))
        })
        .collect()
}

/// Species of `species_dir` coloured by the palette of `body`'s planet file
/// in `planets_dir` (species keep the neutral look when it is missing).
pub fn load_species_for_body(
    species_dir: &std::path::Path,
    planets_dir: &std::path::Path,
    body: &str,
) -> Result<Vec<SpeciesFile>, String> {
    let mut species = load_species_dir(species_dir)?;
    let planet = palette::load_planet(planets_dir, body)?;
    palette::apply(&mut species, &planet);
    if planet.generated_species > 0 {
        let mut generated = species_gen::generate(&planet, &species, planet.generated_species as usize);
        // Woody species take palette colours like authored ones; alien
        // species keep their generated looks.
        for sp in generated.iter_mut().filter(|s| s.plan.is_none()) {
            sp.look = palette::Palette::for_planet(&planet).species_look(&planet, sp);
        }
        species = generated;
    }
    Ok(species)
}
