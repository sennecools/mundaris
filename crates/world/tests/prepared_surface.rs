use glam::DVec3;
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_world::terrain::{
    PreparedSurface, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

fn source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/prepared/composition.native.json")
}
fn generator() -> SurfaceGenerator {
    let source = Arc::new(PreparedSurface::load(source()).unwrap());
    SurfaceGenerator::new(
        &SurfaceDefinition::from_prepared(TerrainIdentity(81273), TerrainSeed(81273), source),
        100_000.,
    )
    .unwrap()
}
#[test]
fn prepared_matches_published_reference_and_material_roles() {
    let g = generator();
    let vectors: serde_json::Value = serde_json::from_slice(
        &std::fs::read(source().parent().unwrap().join("reference-queries.json")).unwrap(),
    )
    .unwrap();
    for v in vectors.as_array().unwrap() {
        let values = |key: &str| -> DVec3 {
            DVec3::from_array(std::array::from_fn(|i| v[key][i].as_f64().unwrap()))
        };
        let n = values("direction");
        let s = g
            .evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
            .unwrap();
        assert!((s.terrain().height_m() - v["height_m"].as_f64().unwrap()).abs() < 1e-10);
        assert!(
            (s.terrain().tangent_gradient_m_per_unit_direction() - values("gradient")).length()
                < 1e-7
        );
        assert!((s.normal() - values("normal")).length() < 1e-12);
        let expected = values("material").to_array();
        let weights = s.material_weights();
        for (a, b) in [weights[0], weights[1], weights[3]]
            .into_iter()
            .zip(expected)
        {
            assert!((a - b).abs() < 1e-12);
        }
        assert_eq!(weights[2], 0.);
        assert!(
            (g.conservative_radius_envelope_m()[0]..=g.conservative_radius_envelope_m()[1])
                .contains(&s.radius_m())
        );
    }
}
#[test]
fn prepared_complete_gradient_includes_control_and_blend_gradients() {
    let g = generator();
    let epsilon = 1e-7;
    for p in [[0.0024317, 0.0010131, 1.], [0.0057317, 0.0008311, 1.]] {
        let n = DVec3::from_array(p).normalize();
        let t = n.cross(DVec3::Y).normalize();
        let query = |n| {
            g.evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
                .unwrap()
        };
        let s = query(n);
        let plus = query((n + t * epsilon).normalize());
        let minus = query((n - t * epsilon).normalize());
        let derivative = (plus.terrain().height_m() - minus.terrain().height_m()) / (2. * epsilon);
        assert!(
            (derivative - s.terrain().tangent_gradient_m_per_unit_direction().dot(t)).abs() < 2e-4
        );
    }
}
#[test]
fn prepared_identity_and_body_local_reuse() {
    let a = generator();
    let b = generator();
    assert_eq!(a.definition(), b.definition());
    let p = a.definition().prepared().unwrap();
    assert!(p.resident_bytes() <= mundaris_world::terrain::PREPARED_SOURCE_CAP_BYTES);
    assert_eq!(a.resident_heap_bytes(), b.resident_heap_bytes());
    let n = DVec3::new(0.00243, 0.00101, 1.).normalize();
    let location = SurfaceLocation::new(Direction3::try_new(n).unwrap());
    assert_eq!(
        a.evaluate_point(location).unwrap(),
        b.evaluate_point(location).unwrap()
    );
    assert!(p.sample(DVec3::ZERO, 100000.).is_err());
    assert!(p.validate_radius(100.).is_err());
}
#[test]
fn prepared_canonical_faces_share_values_and_cached_queries() {
    let g = generator();
    let mut context = g.prepared_query_context();
    for t in [-1., -0.75, 0., 0.5, 1.] {
        let a = SurfaceLocation::new(Direction3::try_new(DVec3::new(1., 1., t)).unwrap());
        let exact = g.evaluate_point(a).unwrap();
        assert_eq!(exact, context.evaluate_point(a).unwrap());
    }
    // No hierarchy/crater discovery or simulation is invoked by the prepared path.
    assert_eq!(context.stats().feature_misses, 0);
}

const RADIUS: f64 = 100_000.;
type Edit = fn(&mut serde_json::Value);

fn surface() -> PreparedSurface {
    PreparedSurface::load(source()).unwrap()
}
fn direction(x: f64, y: f64) -> DVec3 {
    DVec3::new(x, y, 1.).normalize()
}

/// A copy of the published prepared tree with an edited manifest. Loading resolves
/// every asset relative to the manifest, so variants need their own tree; only the
/// files the loader reads are copied, and the published assets stay untouched.
struct Variant(PathBuf);
impl Variant {
    fn new(name: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Self {
        let root =
            std::env::temp_dir().join(format!("mundaris-prepared-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let from = source().parent().unwrap().to_path_buf();
        copy_loader_files(&from, &root);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(source()).unwrap()).unwrap();
        edit(&mut manifest);
        fs::write(
            root.join("composition.native.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        Self(root)
    }
    fn load(&self) -> Result<PreparedSurface, impl std::fmt::Debug> {
        PreparedSurface::load(self.0.join("composition.native.json"))
    }
}
impl Drop for Variant {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn copy_loader_files(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if entry.file_type().unwrap().is_dir() {
            copy_loader_files(&entry.path(), &to.join(&name));
        } else if name_str == "metadata.json"
            || name_str == "controls.f32"
            || name_str.starts_with("level-0-")
        {
            fs::copy(entry.path(), to.join(&name)).unwrap();
        }
    }
}
fn scaled_amplitudes(amplitude: f64) -> impl FnOnce(&mut serde_json::Value) {
    move |m| {
        for p in m["placements"].as_array_mut().unwrap() {
            p["amplitude"] = amplitude.into();
        }
    }
}

#[test]
fn prepared_crater_reaches_full_authored_depth() {
    let full = surface();
    // Near-zero amplitudes leave the control base height to within 1e-7 m, which
    // isolates the crater term without needing a base-only accessor.
    let tiny = Variant::new("depth", scaled_amplitudes(1e-9));
    let base = tiny.load().unwrap();
    let n = DVec3::Z;
    let depth = full.sample(n, RADIUS).unwrap().height_m - base.sample(n, RADIUS).unwrap().height_m;
    // Stored profile floor is -25.6 m (float32 payload) times vertical scale 5.
    assert!(
        (depth - 5. * -25.6).abs() < 1e-5,
        "centre depth {depth} m, expected {}",
        5. * -25.6
    );
}

#[test]
fn prepared_complete_gradient_matches_finite_differences_in_both_craters() {
    let g = generator();
    let epsilon = 1e-7;
    // Crater-A wall, crater-B wall (centre offset ~0.00594) and the overlap.
    let probes = [
        [0.0030, 0.0011],
        [-0.0041, 0.0017],
        [0.0024317, 0.0010131],
        [0.00594 + 0.0012, 0.0007],
        [0.00594 - 0.0011, -0.0009],
        [0.00594, 0.0014],
        [0.00522, 0.0003],
        [0.0046, -0.0006],
    ];
    for [x, y] in probes {
        let n = direction(x, y);
        let t1 = n.cross(DVec3::Y).normalize();
        let t2 = n.cross(t1);
        let query = |n: DVec3| {
            g.evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
                .unwrap()
        };
        let s = query(n);
        for t in [t1, t2] {
            let plus = query((n + t * epsilon).normalize());
            let minus = query((n - t * epsilon).normalize());
            let derivative =
                (plus.terrain().height_m() - minus.terrain().height_m()) / (2. * epsilon);
            let analytic = s.terrain().tangent_gradient_m_per_unit_direction().dot(t);
            assert!(
                (derivative - analytic).abs() < 2e-4,
                "probe {x},{y}: finite {derivative} analytic {analytic}"
            );
        }
    }
}

#[test]
fn prepared_materials_are_normalized_and_substrate_outside_craters() {
    let p = surface();
    let mut crater_material = false;
    for i in -20..=20 {
        for j in -10..=10 {
            let n = direction(i as f64 * 6e-4, j as f64 * 6e-4);
            let w = p.sample(n, RADIUS).unwrap().weights;
            assert!(w.iter().all(|v| (0.0..=1.0).contains(v)), "{w:?}");
            assert!((w.iter().sum::<f64>() - 1.).abs() < 1e-12, "{w:?}");
            crater_material |= w[0] < 1.;
        }
    }
    assert!(crater_material, "craters must alter the substrate material");
    // Beyond both footprints (and on the far hemisphere) only the substrate remains.
    for n in [
        direction(0.05, 0.),
        direction(-0.03, 0.02),
        DVec3::new(0.6, 0.3, -0.7).normalize(),
        DVec3::X,
    ] {
        assert_eq!(p.sample(n, RADIUS).unwrap().weights, [1., 0., 0., 0.]);
    }
}

#[test]
fn prepared_rejects_invalid_v2_manifests() {
    assert!(Variant::new("ok", |_| {}).load().is_ok());
    let cases: [(&str, Edit); 7] = [
        ("gain", |m| m["placements"][0]["gain"] = 1.0.into()),
        ("zero", |m| m["placements"][0]["amplitude"] = 0.0.into()),
        ("high", |m| m["placements"][1]["amplitude"] = 1.5.into()),
        ("negative", |m| {
            m["placements"][0]["amplitude"] = (-0.5).into()
        }),
        ("schema", |m| m["schema_version"] = 1.into()),
        ("algorithm", |m| {
            m["algorithm"] = "mundaris.prepared-composition/1".into()
        }),
        ("missing", |m| {
            m["placements"][0]
                .as_object_mut()
                .unwrap()
                .remove("amplitude");
        }),
    ];
    for (name, edit) in cases {
        assert!(Variant::new(name, edit).load().is_err(), "{name} accepted");
    }
}

#[test]
fn prepared_envelope_contains_sampled_heights_and_radii() {
    let g = generator();
    let p = surface();
    let [lo, hi] = p.displacement_bounds_m();
    let [r_lo, r_hi] = g.conservative_radius_envelope_m();
    let mut deepest = 0f64;
    let mut highest = 0f64;
    for i in -40..=40 {
        for j in -20..=20 {
            let n = direction(i as f64 * 3.5e-4 + 0.003, j as f64 * 3.5e-4);
            let h = p.sample(n, RADIUS).unwrap().height_m;
            assert!((lo..=hi).contains(&h), "{h} outside [{lo}, {hi}]");
            let s = g
                .evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
                .unwrap();
            assert!((r_lo..=r_hi).contains(&s.radius_m()));
            deepest = deepest.min(h);
            highest = highest.max(h);
        }
    }
    // The grid must span a bowl floor and a rim (heights include the base).
    assert!(highest - deepest > 150. && lo < deepest && highest < hi);
}
