//! Generated species: deterministic per planet, all body plans grow within
//! budget, and two seeds give different floras.

use astrum_flora::bodyplan::PLAN_BUDGET;
use astrum_flora::palette::{apply, load_planet};
use astrum_flora::{Kit, grow_meshes, load_species_dir, species_gen, variant_seed};

fn setup(seed: u64) -> Vec<astrum_flora::SpeciesFile> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora");
    let mut planet = load_planet(&root.join("planets"), "rust").unwrap();
    planet.seed = seed;
    let mut templates = load_species_dir(&root.join("species")).unwrap();
    apply(&mut templates, &planet);
    species_gen::generate(&planet, &templates, 10)
}

#[test]
fn generation_is_deterministic_and_seed_dependent() {
    let a = setup(7);
    let b = setup(7);
    assert_eq!(a, b);
    let c = setup(2);
    let names_a: Vec<_> = a.iter().map(|s| s.name.clone()).collect();
    let names_c: Vec<_> = c.iter().map(|s| s.name.clone()).collect();
    assert_ne!(names_a, names_c);
}

#[test]
fn generated_species_grow_within_budget() {
    let kit = Kit::builtin();
    for seed in [7, 2] {
        for sp in setup(seed) {
            for v in 0..2 {
                let lods = grow_meshes(&sp, &kit, variant_seed(&sp, v)).1;
                let tris: Vec<usize> = lods.iter().map(|m| m.triangles()).collect();
                assert!(tris[2] > 0 && tris[1] <= tris[0] && tris[2] <= tris[1], "{} {tris:?}", sp.name);
                if sp.plan.is_some() {
                    assert!(tris[0] <= PLAN_BUDGET[0], "{} {tris:?}", sp.name);
                }
                assert!(lods[0].vertices.iter().all(|v| v.position.iter().all(|c| c.is_finite())), "{}", sp.name);
                assert_eq!(lods[0].to_bytes(), grow_meshes(&sp, &kit, variant_seed(&sp, v)).1[0].to_bytes());
            }
        }
    }
}
