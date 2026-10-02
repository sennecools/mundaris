mod common;
use common::*;
use glam::DVec3;
use mundaris_simulation::{FixedStepRunner, SimulationConfig};
use mundaris_world::terrain::*;

#[test]
fn terrain_definition_changes_do_not_rebranch_or_stale_orbital_replay() {
    let mut world = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut reference = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut runner =
        FixedStepRunner::new(&world, SimulationConfig::try_new(10.0).unwrap()).unwrap();
    let mut oracle =
        FixedStepRunner::new(&reference, SimulationConfig::try_new(10.0).unwrap()).unwrap();
    let id = world.bodies().next().unwrap().0;
    let band = TerrainBandConfig::new(
        0.0,
        TerrainScale::Metres {
            longest_wavelength_m: 64.0,
        },
        4,
    )
    .unwrap();
    let config = TerrainConfig::new(
        [band; 5],
        TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap(),
    )
    .unwrap();
    for target in [10, 20, 5, 25] {
        let revision = world.revision();
        world
            .edit_terrain(
                id,
                Some(TerrainDefinition::new(
                    TerrainIdentity(1),
                    TerrainSeed(target),
                    TerrainGeneratorVersion::V1,
                    config.clone(),
                )),
            )
            .unwrap();
        assert_eq!(world.revision(), revision);
        runner.seek_tick(target).unwrap();
        oracle.seek_tick(target).unwrap();
        loop {
            let a = runner.pump(&mut world, |_, _| {}).unwrap();
            let b = oracle.pump(&mut reference, |_, _| {}).unwrap();
            assert_eq!(a.retained_ticks, b.retained_ticks);
            if a.backlog_ticks == 0 && a.replay_remaining.is_none() {
                break;
            }
        }
        assert_eq!(world.sample_time(), reference.sample_time());
        for ((_, a), (_, b)) in world.bodies().zip(reference.bodies()) {
            assert_eq!(a.state(), b.state());
        }
        assert_eq!(
            world.body(id).unwrap().terrain().unwrap().seed(),
            TerrainSeed(target)
        );
    }
}
