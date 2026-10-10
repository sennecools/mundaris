use super::*;
use crate::terrain::{
    tier_a::{bake_full, erosion::drainage, tests::inputs},
    world_map::{CubeMap, texel_direction},
};
use glam::DVec3;

fn baked(n: usize, seed: u64) -> (CubeMap<f32>, CubeMap<f32>, f64) {
    let input = inputs(n, seed);
    let output = bake_full(&input).unwrap();
    (
        output.fields.elevation,
        output.fields.moisture,
        input.radius_m,
    )
}

fn run_on(elevation: &CubeMap<f32>, moisture: &CubeMap<f32>, radius_m: f64) -> Hydrology {
    run(&HydrologyInput {
        elevation,
        moisture,
        radius_m,
        params: HydrologyParams::prototype(),
    })
}

/// A land cap around +Z (h = 1000 m falling to the coast) with a bowl
/// 300 m deep in its middle; ocean elsewhere.
fn bowl(n: usize) -> CubeMap<f32> {
    let mut map = CubeMap::new(n, -1000.0f32);
    for face in 0..6 {
        for j in 0..n {
            for i in 0..n {
                let d = texel_direction(face, i, j, n);
                let angle = d.angle_between(DVec3::Z);
                let k = map.index(face, i, j);
                if angle < 0.8 {
                    let ring = 1000.0 * (angle / 0.8 * std::f64::consts::PI).sin();
                    let pit = -300.0 * (-((angle - 0.4) / 0.08).powi(2)).exp();
                    map.data_mut()[k] = (ring + pit + 1.0) as f32;
                }
            }
        }
    }
    map
}

#[test]
fn every_land_texel_drains_and_a_bowl_becomes_a_lake() {
    let n = 32;
    let elevation = bowl(n);
    // The bowl's lake covers most of its own catchment; a low evaporation
    // ratio keeps it open.
    let moisture = CubeMap::new(n, 1.0f32);
    let hydrology = run(&HydrologyInput {
        elevation: &elevation,
        moisture: &moisture,
        radius_m: 300_000.0,
        params: HydrologyParams {
            endorheic_ratio: 0.1,
            ..HydrologyParams::prototype()
        },
    });
    let fill = &hydrology.fill;
    assert!(fill.pits_before > 0, "the bowl has pits");
    // Following receivers from any land texel reaches the ocean with
    // strictly falling filled height.
    for start in 0..6 * n * n {
        let mut k = start;
        let mut steps = 0;
        while fill.receiver[k] != NO_RECEIVER {
            let r = fill.receiver[k] as usize;
            assert!(fill.filled[r] < fill.filled[k]);
            k = r;
            steps += 1;
            assert!(steps <= 6 * n * n);
        }
        assert!(fill.ocean[k]);
    }
    // The ring's inner moat (angle 0.4) holds water.
    assert!(!fill.lakes.is_empty(), "bowl lake");
    let lake = &fill.lakes[0];
    assert!(lake.spill_m - lake.bottom_m > 100.0, "{lake:?}");
    assert!(!lake.endorheic, "wet catchment overflows");
}

#[test]
fn dry_basins_become_endorheic_with_a_lower_level() {
    let n = 32;
    let elevation = bowl(n);
    let moisture = CubeMap::new(n, 0.001f32);
    let hydrology = run_on(&elevation, &moisture, 300_000.0);
    let lake = &hydrology.fill.lakes[0];
    assert!(lake.endorheic);
    assert!(lake.level_m < lake.spill_m && lake.level_m >= lake.bottom_m);
    // The outlet keeps its water: nothing flows past it.
    let r = hydrology.fill.receiver[lake.outlet as usize] as usize;
    let outlet_q = hydrology.discharge.data()[lake.outlet as usize];
    assert!(hydrology.discharge.data()[r] < outlet_q + 1e-3 || outlet_q <= 0.0);
}

#[test]
fn baked_world_has_draining_rivers_with_descending_beds() {
    let n = 64;
    let (elevation, moisture, radius_m) = baked(n, 7);
    let a = run_on(&elevation, &moisture, radius_m);
    let b = run_on(&elevation, &moisture, radius_m);
    assert_eq!(a.rivers, b.rivers, "deterministic");
    let before = drainage(&elevation, radius_m);
    assert_eq!(before.land_pits, a.fill.pits_before, "same pit definition");
    let graph = &a.rivers;
    assert!(
        graph.segment_count() > 20,
        "{} segments",
        graph.segment_count()
    );
    assert!(graph.mouth_count() > 0);
    for (s, d) in graph.segments() {
        let (up, down) = (&graph.vertices[s as usize], &graph.vertices[d as usize]);
        assert!(
            down.bed_m < up.bed_m,
            "bed rises {} -> {}",
            up.bed_m,
            down.bed_m
        );
        assert!(down.discharge_km2 >= up.discharge_km2 - 1e-6);
    }
    // Every chain ends at a mouth.
    for v in &graph.vertices {
        if v.downstream == NO_RECEIVER {
            assert_ne!(v.mouth, rivers::Mouth::None);
        }
    }
}

#[test]
fn carving_only_lowers_and_reaches_the_bed_on_the_channel() {
    let n = 64;
    let (elevation, moisture, radius_m) = baked(n, 7);
    let hydrology = run_on(&elevation, &moisture, radius_m);
    let graph = &hydrology.rivers;
    let mut checked = 0;
    for (s, d) in graph.segments().take(200) {
        let (a, b) = (&graph.vertices[s as usize], &graph.vertices[d as usize]);
        let mid = (a.pos + b.pos).normalize();
        assert!(
            hydrology.hash.near(mid).contains(&s),
            "hash finds its segment"
        );
        let h = 0.5 * (a.bed_m + b.bed_m) + 200.0;
        let c = hydrology.carve(mid, h);
        assert!(c.height_m <= h);
        // On the centreline the carve reaches the bed (within the chord sag).
        assert!(c.height_m <= 0.5 * (a.bed_m + b.bed_m) + 1.0, "{c:?}");
        assert!(c.water_m.is_some());
        checked += 1;
    }
    assert!(checked > 0);
    // Far from every river nothing changes.
    for k in (0..6 * n * n).step_by(97) {
        let d = texel_direction(k / (n * n), k % n, (k / n) % n, n);
        let c = hydrology.carve(d, 5_000.0);
        assert!(c.height_m <= 5_000.0);
        if c.distance_m > hydrology.params.valley_max_m + 1.0 {
            assert_eq!(c.height_m, 5_000.0);
        }
    }
}

#[test]
fn profiles_are_monotone_and_meet_the_terrain() {
    let mut last = (0.0, 0.0);
    for i in 0..=100 {
        let x = f64::from(i) / 100.0;
        let (v, f) = (carve::v_profile(x), carve::floodplain_profile(x));
        assert!(v >= last.0 && f >= last.1);
        last = (v, f);
    }
    assert_eq!(carve::v_profile(0.0), 0.0);
    assert!((carve::v_profile(1.0) - 1.0).abs() < 1e-12);
    assert!((carve::floodplain_profile(1.0) - 1.0).abs() < 1e-12);
    assert!(carve::floodplain_profile(0.4) < 0.05);
}
