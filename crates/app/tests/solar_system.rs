use mundaris_app::solar_system::*;
use mundaris_math::*;
use mundaris_simulation::{GRAVITATIONAL_CONSTANT_M3_KG_S2 as G, IntegrationWorkspace};
use mundaris_world::terrain::*;
use std::num::NonZeroU64;

fn ns(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}

#[test]
fn analytic_authored_periods_preserve_original_setup_pacing_and_spin() {
    use mundaris_app::motion_session::AnalyticSession;
    for preset in [
        SolarSystemPreset::gameplay(),
        SolarSystemPreset::real_scale(),
    ] {
        let legacy = preset.create(ns(81)).unwrap();
        let legacy_bodies = bodies(&legacy);
        let (mut analytic, definition) = preset.create_analytic(ns(82)).unwrap();
        let session = AnalyticSession::new(&mut analytic, definition).unwrap();
        for (i, content) in SOLAR_SYSTEM_CONTENT.iter().enumerate().skip(1) {
            let parent = if content.identity == SolarBody::Moon {
                3
            } else {
                0
            };
            let mut mass = legacy_bodies[parent].properties().mass_kg()
                + legacy_bodies[i].properties().mass_kg();
            if content.identity == SolarBody::Earth {
                mass += legacy_bodies[4].properties().mass_kg();
            }
            let distance = content.real_orbital_distance_m * preset.orbital_distance_scale;
            let expected = std::f64::consts::TAU * distance / (G * mass / distance).sqrt();
            let orbit = match session.definition().definitions()[i].translation {
                mundaris_world::CelestialTranslation::Elliptic(orbit) => orbit,
                _ => panic!("non-central solar body must orbit"),
            };
            assert!(
                (orbit.period_seconds() / expected - 1.0).abs() < 1e-14,
                "{} pacing",
                content.name
            );
            assert_eq!(
                orbit.mean_anomaly_at_epoch_radians(),
                content.orbital_phase_rad
            );
        }
        for (legacy, current) in legacy.bodies().zip(analytic.bodies()) {
            assert_eq!(legacy.1.name(), current.1.name());
            assert_eq!(legacy.1.properties(), current.1.properties());
            assert_eq!(legacy.1.terrain(), current.1.terrain());
            // The prescribed producer normalizes composed rotations, including
            // the zero-angle epoch spin. Compare the convention geometrically;
            // return-to-T bitwise reproducibility is a separate regression.
            for axis in [glam::DVec3::X, glam::DVec3::Y, glam::DVec3::Z] {
                assert!(
                    (legacy.1.state().body_to_system().quaternion() * axis
                        - current.1.state().body_to_system().quaternion() * axis)
                        .length()
                        < 1e-14
                );
            }
            assert!(
                (legacy
                    .1
                    .state()
                    .angular_velocity_in_system()
                    .radians_per_second()
                    - current
                        .1
                        .state()
                        .angular_velocity_in_system()
                        .radians_per_second())
                .length()
                    < 1e-18
            );
        }
    }
}
fn bodies(system: &mundaris_world::CelestialSystem) -> Vec<&mundaris_world::CelestialBody> {
    system.bodies().map(|(_, body)| body).collect()
}

#[test]
fn catalog_identity_seeds_and_scale_controls() {
    let identities: Vec<_> = SOLAR_SYSTEM_CONTENT
        .iter()
        .map(|entry| entry.identity)
        .collect();
    assert_eq!(
        identities,
        [
            SolarBody::Sun,
            SolarBody::Mercury,
            SolarBody::Venus,
            SolarBody::Earth,
            SolarBody::Moon,
            SolarBody::Mars,
            SolarBody::Jupiter,
            SolarBody::Saturn,
            SolarBody::Uranus,
            SolarBody::Neptune
        ]
    );
    assert_eq!(
        content(SolarBody::Mercury).orbit_parent,
        Some(SolarBody::Sun)
    );
    assert_eq!(content(SolarBody::Earth).orbit_parent, Some(SolarBody::Sun));
    assert_eq!(
        content(SolarBody::Moon).orbit_parent,
        Some(SolarBody::Earth)
    );
    assert_eq!(content(SolarBody::Sun).orbit_parent, None);
    let seeds: Vec<_> = SOLAR_SYSTEM_CONTENT
        .iter()
        .filter_map(|b| b.rocky_terrain_seed)
        .collect();
    let unique: std::collections::HashSet<_> = seeds.iter().collect();
    assert_eq!(seeds.len(), 5);
    assert_eq!(unique.len(), seeds.len());

    for (namespace, diameter) in [(1, 600_000.0), (2, 800_000.0), (3, 1_000_000.0)] {
        let preset = SolarSystemPreset {
            body_radius_scale: diameter / (2.0 * 6_371_000.0),
            orbital_distance_scale: 1.0,
        };
        let system = preset.create(ns(namespace)).unwrap();
        assert!(
            (bodies(&system)[3].properties().reference_radius_m() * 2.0 - diameter).abs() < 1e-8
        );
    }
    let gameplay = SolarSystemPreset::gameplay();
    assert_eq!(gameplay.body_radius_scale, 400_000.0 / 6_371_000.0);
    assert_eq!(gameplay.orbital_distance_scale, gameplay.body_radius_scale);
    let real = SolarSystemPreset::real_scale().create(ns(4)).unwrap();
    assert_eq!(
        bodies(&real)[3].properties().reference_radius_m(),
        6_371_000.0
    );

    let radius_only = SolarSystemPreset {
        body_radius_scale: 2.0,
        orbital_distance_scale: 1.0,
    }
    .create(ns(5))
    .unwrap();
    let distance_only = SolarSystemPreset {
        body_radius_scale: 1.0,
        orbital_distance_scale: 2.0,
    }
    .create(ns(6))
    .unwrap();
    let normal = SolarSystemPreset::real_scale().create(ns(7)).unwrap();
    let (n, r, d) = (
        bodies(&normal),
        bodies(&radius_only),
        bodies(&distance_only),
    );
    assert_eq!(
        r[3].properties().reference_radius_m(),
        2.0 * n[3].properties().reference_radius_m()
    );
    assert_eq!(
        r[3].state().center_in_system().metres(),
        n[3].state().center_in_system().metres()
    );
    assert_eq!(
        d[3].properties().reference_radius_m(),
        n[3].properties().reference_radius_m()
    );
    let scaled_earth_moon = (d[4].state().center_in_system().metres()
        - d[3].state().center_in_system().metres())
    .length();
    assert!((scaled_earth_moon - 2.0 * 3.844e8).abs() < 1e-5);
    assert!(
        SolarSystemPreset {
            body_radius_scale: f64::INFINITY,
            orbital_distance_scale: 1.0
        }
        .create(ns(8))
        .is_err()
    );
}

#[test]
fn reference_gravity_initial_orbits_barycenter_and_advance() {
    let mut system = SolarSystemPreset::gameplay().create(ns(20)).unwrap();
    let initial = bodies(&system);
    for (body, content) in initial.iter().zip(SOLAR_SYSTEM_CONTENT) {
        let radius = body.properties().reference_radius_m();
        let mu = G * body.properties().mass_kg();
        assert!((mu / radius.powi(2) - content.reference_gravity_m_s2).abs() < 1e-12);
        assert!((radius / 400_000.0 - content.real_mean_radius_m / 6_371_000.0).abs() < 1e-12);
        let spin = body
            .state()
            .angular_velocity_in_system()
            .radians_per_second()
            .normalize();
        assert!((spin.dot(glam::DVec3::Z) - content.axial_tilt_rad.cos()).abs() < 1e-14);
    }
    let earth_radius = initial[3].properties().reference_radius_m();
    let earth_mu = G * initial[3].properties().mass_kg();
    assert!((earth_mu - 1.569064e12).abs() < 1e-3);
    assert!(((2.0 * earth_mu / earth_radius).sqrt() - 2800.95).abs() < 0.02);

    let earth = initial[3].state().center_in_system().metres();
    let moon = initial[4].state().center_in_system().metres();
    let earth_moon_vector = moon - earth;
    assert!(
        (earth_moon_vector.length()
            - 3.844e8 * SolarSystemPreset::gameplay().orbital_distance_scale)
            .abs()
            < 1e-5
    );
    let expected_moon_speed = (G
        * (initial[3].properties().mass_kg() + initial[4].properties().mass_kg())
        / earth_moon_vector.length())
    .sqrt();
    let relative_velocity = initial[4]
        .state()
        .center_velocity_in_system()
        .metres_per_second()
        - initial[3]
            .state()
            .center_velocity_in_system()
            .metres_per_second();
    assert!((relative_velocity.length() - expected_moon_speed).abs() < 1e-10);

    // The Earth-Moon mass-weighted state follows Earth's Sun-centered circular orbit.
    let inner_mass = initial[3].properties().mass_kg() + initial[4].properties().mass_kg();
    let inner_position = (earth * initial[3].properties().mass_kg()
        + moon * initial[4].properties().mass_kg())
        / inner_mass;
    let sun = initial[0].state().center_in_system().metres();
    let earth_distance = (inner_position - sun).length();
    let expected_earth_distance = content(SolarBody::Earth).real_orbital_distance_m
        * SolarSystemPreset::gameplay().orbital_distance_scale;
    assert!((earth_distance - expected_earth_distance).abs() < 1e-4);
    let sun_mass = initial[0].properties().mass_kg();
    let expected_outer_speed = (G * (sun_mass + inner_mass) / earth_distance).sqrt();
    let barycenter_velocity = (initial[3]
        .state()
        .center_velocity_in_system()
        .metres_per_second()
        * initial[3].properties().mass_kg()
        + initial[4]
            .state()
            .center_velocity_in_system()
            .metres_per_second()
            * initial[4].properties().mass_kg())
        / inner_mass;
    let sun_velocity = initial[0]
        .state()
        .center_velocity_in_system()
        .metres_per_second();
    assert!(((barycenter_velocity - sun_velocity).length() - expected_outer_speed).abs() < 1e-10);

    let total_mass: f64 = initial.iter().map(|b| b.properties().mass_kg()).sum();
    let com = initial.iter().fold(glam::DVec3::ZERO, |sum, b| {
        sum + b.state().center_in_system().metres() * b.properties().mass_kg()
    }) / total_mass;
    let momentum = initial.iter().fold(glam::DVec3::ZERO, |sum, b| {
        sum + b.state().center_velocity_in_system().metres_per_second() * b.properties().mass_kg()
    });
    assert!(com.length() < 1e-3);
    assert!(momentum.length() / (total_mass * 1.0e4) < 1e-14);

    let earth_initial_position = initial[3].state().center_in_system();
    let mut workspace = IntegrationWorkspace::new(&system, 60.0).unwrap();
    workspace.prepare_step().unwrap();
    let instant = SimulationInstant::try_seconds_since_epoch(60.0).unwrap();
    workspace.commit_candidate(&mut system, instant).unwrap();
    assert_eq!(system.sample_time(), instant);
    assert_ne!(
        bodies(&system)[3].state().center_in_system(),
        earth_initial_position
    );
}

#[test]
fn both_presets_advance_with_the_unchanged_resolved_newtonian_integrator() {
    for preset in [
        SolarSystemPreset::gameplay(),
        SolarSystemPreset::real_scale(),
    ] {
        let mut system = preset.create(ns(40)).unwrap();
        let baseline = mundaris_simulation::DiagnosticBaseline(
            mundaris_simulation::system_diagnostics(&system).unwrap(),
        );
        let mut workspace = IntegrationWorkspace::new(&system, 60.0).unwrap();
        for tick in 1..=1000 {
            let report = workspace.prepare_step().unwrap();
            assert_eq!(report.pair_evaluations, 45);
            assert!(report.max_chi <= 0.02);
            workspace
                .commit_candidate(
                    &mut system,
                    SimulationInstant::try_seconds_since_epoch(tick as f64 * 60.0).unwrap(),
                )
                .unwrap();
        }
        let drift = baseline.drift(mundaris_simulation::system_diagnostics(&system).unwrap());
        assert!(drift.relative_energy.abs() < 1e-7, "{drift:?}");
        assert!(drift.normalized_momentum < 1e-12, "{drift:?}");
    }
}

#[test]
fn rocky_terrain_definitions_are_distinct_and_attached() {
    let system = SolarSystemPreset::gameplay().create(ns(30)).unwrap();
    let world_bodies = bodies(&system);
    let mut definitions = Vec::new();
    for (index, body) in world_bodies.iter().enumerate() {
        match SOLAR_SYSTEM_CONTENT[index].rocky_terrain_seed {
            Some(seed) => {
                if SOLAR_SYSTEM_CONTENT[index].identity == SolarBody::Moon {
                    assert!(body.terrain().is_none());
                    assert_eq!(body.surface_definition().unwrap().seed(), TerrainSeed(seed));
                    assert_eq!(
                        body.surface_definition().unwrap().terrain().algorithm(),
                        SurfaceAlgorithm::RockyV5
                    );
                    continue;
                }
                let definition = body.terrain().expect("rocky bodies receive terrain");
                assert_eq!(definition.seed(), TerrainSeed(seed));
                assert_eq!(definition.version(), TerrainGeneratorVersion::V2);
                assert!(
                    definition.config().absolute_height_bound_m()
                        < 0.1 * body.properties().reference_radius_m()
                );
                definitions.push(definition);
            }
            None => assert!(body.terrain().is_none(), "Sun/gas giants have no terrain"),
        }
    }
    assert_eq!(definitions.len(), 4);
    assert!(definitions.windows(2).all(|pair| pair[0] != pair[1]));

    let small = terrain_definition(SolarBody::Earth, 50_000.0)
        .unwrap()
        .unwrap();
    let medium = terrain_definition(SolarBody::Earth, 100_000.0)
        .unwrap()
        .unwrap();
    let large = terrain_definition(SolarBody::Earth, 6_371_000.0)
        .unwrap()
        .unwrap();
    assert_ne!(
        small.config().band(TerrainBand::Range).scale(),
        large.config().band(TerrainBand::Range).scale()
    );
    assert_eq!(
        small.config().band(TerrainBand::Regional).scale(),
        large.config().band(TerrainBand::Regional).scale()
    );
    assert!(small.config().absolute_height_bound_m() < 5_000.0);
    assert!(medium.config().absolute_height_bound_m() < 10_000.0);
    assert_ne!(
        terrain_definition(SolarBody::Earth, 6.371e6).unwrap(),
        terrain_definition(SolarBody::Mars, 3.3895e6).unwrap()
    );
    for radius in [
        500.0,
        50_000.0,
        100_000.0,
        400_000.0,
        1_000_000.0,
        6_371_000.0,
        12_742_000.0,
    ] {
        let definition = terrain_definition(SolarBody::Earth, radius)
            .unwrap()
            .unwrap();
        let generator = TerrainGenerator::new(&definition, radius).unwrap();
        let location = mundaris_math::surface::SurfaceLocation::new(
            Direction3::try_new(glam::DVec3::new(1.0, 2.0, 3.0)).unwrap(),
        );
        let mut samples = [TerrainSample::default(); 1];
        generator
            .evaluate_batch(&[location], TerrainFootprint::COMPLETE, &mut samples)
            .unwrap();
        assert!(samples[0].height_m().is_finite());
        assert!(
            samples[0]
                .normal_body(location, radius)
                .unwrap()
                .unit()
                .dot(location.direction().unit())
                > 0.0
        );
    }
}

#[test]
fn moon_surface_is_native_rocky_v5_and_clearance_queries_the_same_field() {
    for (namespace, preset) in [
        (ns(31), SolarSystemPreset::gameplay()),
        (ns(32), SolarSystemPreset::real_scale()),
    ] {
        let mut system = preset.create(namespace).unwrap();
        let (moon_id, moon) = system
            .bodies()
            .find(|(_, body)| body.name() == "Moon")
            .expect("Moon body exists");
        let definition = moon.surface_definition().expect("native Moon surface");
        let original_surface = definition.clone();
        assert!(moon.terrain().is_none());
        assert_eq!(definition.terrain().algorithm(), SurfaceAlgorithm::RockyV5);

        let radius_m = moon.properties().reference_radius_m();
        let location = mundaris_math::surface::SurfaceLocation::new(
            Direction3::try_new(glam::DVec3::new(1.0, 2.0, 3.0)).unwrap(),
        );
        let sample = SurfaceGenerator::new(definition, radius_m)
            .unwrap()
            .evaluate_point(location)
            .unwrap();
        let body_position = location.direction().unit() * (radius_m + 500.0);
        let clearance = mundaris_app::terrain_inspection::clearance_at_body_position(
            moon,
            body_position,
            moon_id,
        )
        .unwrap()
        .expect("surface clearance");
        assert!(
            (clearance.location.direction().unit() - location.direction().unit()).length() < 1e-14
        );
        assert!(
            (clearance.terrain_elevation_m - sample.terrain().height_m()).abs() < 1e-10 * radius_m
        );
        assert!((clearance.surface_radius_m - sample.radius_m()).abs() < 1e-10 * radius_m);

        // Keep the historical CrateredV1 recipe independently constructible and
        // replayable even though the native preset now selects RockyV5.
        let seed = definition.seed();
        let historical =
            cratered_terrain_definition(definition.identity(), seed, radius_m).unwrap();
        assert_eq!(historical.version(), TerrainGeneratorVersion::CrateredV1);
        let first = TerrainGenerator::new(&historical, radius_m).unwrap();
        let replay = cratered_terrain_definition(definition.identity(), seed, radius_m).unwrap();
        assert_eq!(
            first.crater_features(),
            TerrainGenerator::new(&replay, radius_m)
                .unwrap()
                .crater_features()
        );
        assert_eq!(first.crater_features().len(), 128);

        // The UI radius command publishes body properties through this world
        // edit path. It must retain the selected authority and refresh its radius.
        let mass_kg = moon.properties().mass_kg();
        let edited_radius_m = radius_m * 1.01;
        system
            .edit_properties(
                moon_id,
                mundaris_world::BodyProperties::new(mass_kg, edited_radius_m).unwrap(),
            )
            .unwrap();
        let edited_moon = system.body(moon_id).unwrap();
        assert_eq!(edited_moon.surface_definition(), Some(&original_surface));
    }
}

#[test]
fn cratered_recipe_is_seeded_reusable_and_bounded_for_arbitrary_bodies() {
    let fixtures = [
        (TerrainIdentity(0xdecafbad), TerrainSeed(71), 80_000.0),
        (TerrainIdentity(0x1234_5678), TerrainSeed(92), 1_200_000.0),
    ];

    for (identity, seed, radius_m) in fixtures {
        let definition = cratered_terrain_definition(identity, seed, radius_m).unwrap();
        assert_eq!(definition.identity(), identity);
        assert_eq!(definition.seed(), seed);
        assert_eq!(definition.version(), TerrainGeneratorVersion::CrateredV1);
        assert_eq!(
            TerrainGeneratorVersion::from_code(3).unwrap(),
            definition.version()
        );
        definition.validate_radius(radius_m).unwrap();

        let generator = TerrainGenerator::new(&definition, radius_m).unwrap();
        let repeated = cratered_terrain_definition(identity, seed, radius_m).unwrap();
        let repeated_generator = TerrainGenerator::new(&repeated, radius_m).unwrap();
        assert_eq!(
            generator.crater_features(),
            repeated_generator.crater_features()
        );
        let features = generator.crater_features();
        assert_eq!(features.len(), 128);
        let minimum_radius_m = 64.0;
        let maximum_radius_m = (0.13 * radius_m).min(24_000.0);
        for feature in features {
            let center = feature.center().unit();
            assert!(center.is_finite());
            assert!((center.length() - 1.0).abs() < 1e-12);
            assert!((minimum_radius_m..=maximum_radius_m).contains(&feature.radius_m()));
            assert!(feature.depth_m().is_finite() && feature.depth_m() > 0.0);
            assert!(feature.rim_height_m().is_finite() && feature.rim_height_m() >= 0.0);
        }

        let changed_seed =
            cratered_terrain_definition(identity, TerrainSeed(seed.0 + 1), radius_m).unwrap();
        let changed_generator = TerrainGenerator::new(&changed_seed, radius_m).unwrap();
        assert_ne!(features, changed_generator.crater_features());

        let locations: Vec<_> = [
            glam::DVec3::X,
            glam::DVec3::Y,
            glam::DVec3::Z,
            -glam::DVec3::X,
            -glam::DVec3::Y,
            -glam::DVec3::Z,
            glam::DVec3::new(1.0, 2.0, 3.0),
            glam::DVec3::new(-4.0, 1.0, 2.0),
            glam::DVec3::new(2.0, -3.0, 1.0),
        ]
        .into_iter()
        .map(|direction| {
            mundaris_math::surface::SurfaceLocation::new(Direction3::try_new(direction).unwrap())
        })
        .collect();
        let mut samples = vec![TerrainSample::default(); locations.len()];
        generator
            .evaluate_batch(&locations, TerrainFootprint::COMPLETE, &mut samples)
            .unwrap();
        let height_bound_m = definition.config().absolute_height_bound_m();
        for (location, sample) in locations.iter().zip(samples) {
            assert!(sample.height_m().is_finite());
            assert!(sample.height_m().abs() <= height_bound_m);
            assert!(
                sample
                    .normal_body(*location, radius_m)
                    .unwrap()
                    .unit()
                    .is_finite()
            );
        }
    }
}
