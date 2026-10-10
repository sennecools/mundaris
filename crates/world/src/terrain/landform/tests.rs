//! Landform tests. They check properties only, never exact noise values, so
//! they hold for any ladder noise implementation.
use super::eval::{LANE_RELIEF, gully};
use super::schema::{Input, Node};
use super::*;
use crate::terrain::noise::octave_weight;
use crate::terrain::surface::ladder::{LadderOctave, octave_noise3, octave_seed};
use glam::DVec3;
use std::f64::consts::{FRAC_2_PI, TAU};
use std::path::{Path, PathBuf};

const RADIUS_M: f64 = 338_950.0;

fn content_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/landforms")
}

fn load_recipe(path: &str) -> Result<RecipeFile, LandformError> {
    let text =
        std::fs::read_to_string(content_dir().join(path)).map_err(|e| LandformError::Load {
            path: path.into(),
            message: e.to_string(),
        })?;
    ron::from_str(&text).map_err(|e| LandformError::Load {
        path: path.into(),
        message: e.to_string(),
    })
}

fn terra() -> LandformSet {
    let text = std::fs::read_to_string(content_dir().join("terra.ron")).unwrap();
    let file: LandformSetFile = ron::from_str(&text).unwrap();
    LandformSet::compile(&file, load_recipe).unwrap()
}

fn recipe(text: &str) -> RecipeFile {
    ron::from_str(text).unwrap()
}

fn options() -> CompileOptions {
    CompileOptions {
        relief_band_edge_m: 2600.0,
    }
}

fn compile(text: &str) -> Result<Program, LandformError> {
    Program::compile("test", &recipe(text), &options())
}

/// Smooth synthetic Tier A fields with exact gradients; `δ` is the signed
/// distance to the plane through the body centre with normal `axis` (a
/// straight boundary along a great circle).
struct Synthetic {
    axis: DVec3,
}

impl Synthetic {
    fn new() -> Self {
        Self {
            axis: DVec3::new(0.3, -0.5, 0.81).normalize(),
        }
    }
}

fn wave(p: DVec3, k: DVec3, mean: f64, amplitude: f64) -> Dual {
    let phase = p.dot(k);
    Dual::new(
        mean + amplitude * phase.sin(),
        k * (amplitude * phase.cos()),
    )
}

impl FieldSource for Synthetic {
    fn field(&self, field: RecipeField, p: DVec3) -> Dual {
        let k = |a: f64, b: f64, c: f64| DVec3::new(a, b, c) * TAU;
        match field {
            RecipeField::BoundaryCoord => Dual::new(p.dot(self.axis), self.axis),
            RecipeField::Uplift => wave(p, k(1.0 / 31e3, 1.0 / 43e3, 1.0 / 27e3), 0.5, 0.45),
            RecipeField::Flow => wave(p, k(1.0 / 9e3, -1.0 / 13e3, 1.0 / 17e3), 0.5, 0.48),
            RecipeField::Hardness => wave(p, k(-1.0 / 21e3, 1.0 / 11e3, 1.0 / 37e3), 0.5, 0.4),
            RecipeField::Sediment => wave(p, k(1.0 / 15e3, 1.0 / 25e3, -1.0 / 19e3), 0.5, 0.45),
            RecipeField::Moisture => wave(p, k(1.0 / 45e3, 1.0 / 35e3, 1.0 / 55e3), 0.5, 0.45),
            RecipeField::Volcanic => wave(p, k(1.0 / 65e3, -1.0 / 35e3, 1.0 / 25e3), 0.5, 0.45),
            RecipeField::Temperature => wave(p, k(1.0 / 95e3, 1.0 / 75e3, 1.0 / 85e3), 15.0, 20.0),
            RecipeField::Elevation => {
                wave(p, k(1.0 / 105e3, 1.0 / 65e3, 1.0 / 85e3), 500.0, 1500.0)
            }
        }
    }
}

/// Deterministic SplitMix64 stream.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((x ^ (x >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform point on the reference sphere.
    fn sphere(&mut self) -> DVec3 {
        let z = 2.0 * self.unit() - 1.0;
        let phi = TAU * self.unit();
        let r = (1.0 - z * z).sqrt();
        DVec3::new(r * phi.cos(), r * phi.sin(), z) * RADIUS_M
    }

    /// Point on the sphere within `band_m` of the great circle normal to
    /// `axis`.
    fn near_boundary(&mut self, axis: DVec3, band_m: f64) -> DVec3 {
        let u = axis.any_orthonormal_vector();
        let v = axis.cross(u);
        let angle = TAU * self.unit();
        let offset = (2.0 * self.unit() - 1.0) * band_m / RADIUS_M;
        (u * angle.cos() + v * angle.sin() + axis * offset).normalize() * RADIUS_M
    }
}

fn params(amplitude_m: f64) -> LandformParams {
    LandformParams {
        amplitude_m,
        seed: 0x00c0_ffee,
    }
}

fn fd_gradient(
    program: &Program,
    p: DVec3,
    params: &LandformParams,
    texel_m: Option<f64>,
    fields: &dyn FieldSource,
) -> DVec3 {
    let h = 1e-3;
    let e = |axis: DVec3| {
        (program
            .evaluate(p + axis * h, params, texel_m, fields)
            .value
            - program
                .evaluate(p - axis * h, params, texel_m, fields)
                .value)
            / (2.0 * h)
    };
    DVec3::new(e(DVec3::X), e(DVec3::Y), e(DVec3::Z))
}

fn tangential(g: DVec3, p: DVec3) -> DVec3 {
    let n = p.normalize();
    g - n * n.dot(g)
}

// ------------------------------------------------------------- content

#[test]
fn terra_landforms_load_validate_and_produce_normalised_weights() {
    let set = terra();
    let names: Vec<&str> = set.landforms().iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["mountains", "hills", "plains"]);
    assert_eq!(set.fallback(), 2);
    let mountains = &set.landforms()[0].program;
    let Some(Op::Stack(stack)) = mountains.ops().iter().find(|op| matches!(op, Op::Stack(_)))
    else {
        panic!("mountains have no stack");
    };
    assert!(stack.anisotropy.is_some() && stack.warp.is_some());
    assert!(stack.damp.is_some() && stack.erosion.is_some());
    assert_eq!(stack.base.wavelength_m(), 4096.0);
    assert_eq!(stack.octaves, 8);
    // Weights from the bytecode: normalised, plains where nothing else
    // applies, mountains on high uplift.
    let mut fields = [0.0; expr::field::COUNT];
    for (uplift, sediment, moisture) in [(0.0, 0.0, 0.0), (0.4, 0.5, 0.8), (0.95, 0.1, 0.3)] {
        fields[expr::field::UPLIFT as usize] = uplift;
        fields[expr::field::SEDIMENT as usize] = sediment;
        fields[expr::field::MOISTURE as usize] = moisture;
        let w = set.weights(&fields);
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12, "{w:?}");
        match uplift {
            0.0 => assert!(w[2] > 0.99, "{w:?}"),
            0.95 => assert!(w[0] > 0.9, "{w:?}"),
            _ => assert!(w[1] > 0.3, "{w:?}"),
        }
    }
    let p = set.sample_params(7);
    assert_eq!(p, set.sample_params(7));
    assert_ne!(p, set.sample_params(8));
    for (p, l) in p.iter().zip(set.landforms()) {
        assert!((l.amplitude_m.0..=l.amplitude_m.1).contains(&p.amplitude_m));
    }
    let bound = set.bound_m(&p);
    assert!(
        bound.is_finite() && bound > 0.0 && bound < 5_000.0,
        "{bound}"
    );
    assert_eq!(set.identity(), terra().identity());
}

#[test]
fn set_validation_rejects_bad_files() {
    let text = std::fs::read_to_string(content_dir().join("terra.ron")).unwrap();
    let file: LandformSetFile = ron::from_str(&text).unwrap();
    let mut bad = file.clone();
    bad.fallback = "dunes".into();
    assert!(LandformSet::compile(&bad, load_recipe).is_err());
    let mut bad = file.clone();
    bad.landforms[1].weight = "uplift +".into();
    let err = LandformSet::compile(&bad, load_recipe).unwrap_err();
    assert!(matches!(err, LandformError::InLandform { ref landform, .. } if landform == "hills"));
    let mut bad = file.clone();
    bad.landforms[0].amplitude_m = (900.0, 500.0);
    assert!(LandformSet::compile(&bad, load_recipe).is_err());
    let mut bad = file.clone();
    bad.relief_band_edge_m = 1000.0;
    assert!(LandformSet::compile(&bad, load_recipe).is_err());
    let mut bad = file;
    bad.landforms[2].recipe = "recipes/missing.ron".into();
    assert!(LandformSet::compile(&bad, load_recipe).is_err());
}

// ------------------------------------------------------- IR validation

const RIDGE: &str = r#"RidgedFbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, sharpness: 2.0, salt: 1)"#;

#[test]
fn ir_rejects_invalid_graphs_with_the_node_name() {
    let node_error = |text: &str, node: &str| match compile(text) {
        Err(LandformError::Node { node: n, message }) => {
            assert_eq!(n, node, "{message}");
            message
        }
        other => panic!("{text}: {other:?}"),
    };
    // References, cycles, unused nodes.
    node_error(
        r#"Recipe(schema: 1, output: "a", graph: [("a", Add(a: "b", b: Const(1)))])"#,
        "a.a",
    );
    node_error(
        r#"Recipe(schema: 1, output: "a", graph: [("a", Add(a: "b", b: Const(1))), ("b", Scale(input: "a", factor: 2))])"#,
        "a",
    );
    let message = node_error(
        &format!(r#"Recipe(schema: 1, output: "a", graph: [("a", Const(1)), ("r", {RIDGE})])"#),
        "r",
    );
    assert!(message.contains("not used"));
    // Types.
    node_error(
        r#"Recipe(schema: 1, output: "w", graph: [("w", Warp(input: Field(Uplift), strength_m: 10, base_wavelength_m: 8192, octaves: 1, salt: 1))])"#,
        "w.input",
    );
    node_error(
        r#"Recipe(schema: 1, output: "d", graph: [("d", SlopeDamped(input: Const(1), damp: 1))])"#,
        "d.input",
    );
    node_error(
        &format!(
            r#"Recipe(schema: 1, output: "e", graph: [("r", {RIDGE}), ("s", Scale(input: "r", factor: 1)), ("e", ErosionFilter(input: "s", strength: 1, base_wavelength_m: 512, octaves: 2, salt: 1))])"#
        ),
        "e.input",
    );
    node_error(
        &format!(
            r#"Recipe(schema: 1, output: "w2", graph: [("r", {RIDGE}), ("w", Warp(input: "r", strength_m: 10, base_wavelength_m: 8192, octaves: 1, salt: 1)), ("w2", Warp(input: "w", strength_m: 10, base_wavelength_m: 8192, octaves: 1, salt: 1))])"#
        ),
        "w2",
    );
    // Parameter ranges and precision limits.
    for (node, message_part) in [
        (
            "RidgedFbm(base_wavelength_m: 4096, min_wavelength_m: 32, gain: 0.5, sharpness: 2.0, salt: 1)",
            "band edge",
        ),
        (
            "Fbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 1.0, salt: 1)",
            "gain",
        ),
        (
            "Fbm(base_wavelength_m: 2048, min_wavelength_m: 0.01, gain: 0.5, salt: 1)",
            "min_wavelength",
        ),
        (
            "Fbm(base_wavelength_m: 1100, min_wavelength_m: 1100, gain: 0.5, salt: 1)",
            "octaves",
        ),
        (
            "RidgedFbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, sharpness: 0.1, salt: 1)",
            "sharpness",
        ),
        (
            "Fbm(base_wavelength_m: 2560, min_wavelength_m: 32, gain: 0.5, salt: 1, anisotropy: Some((stretch: 2, kappa: 2.0, octaves: 1, clamp_km: 10)))",
            "power-of-two",
        ),
        (
            "Fbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, salt: 1, anisotropy: Some((stretch: 3, kappa: 2.0, octaves: 1, clamp_km: 10)))",
            "stretch",
        ),
        (
            "Fbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, salt: 1, anisotropy: Some((stretch: 2, kappa: 1.5, octaves: 1, clamp_km: 10)))",
            "kappa",
        ),
        (
            "Fbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, salt: 1, anisotropy: Some((stretch: 2, kappa: 2.0, octaves: 3, clamp_km: 150)))",
            "f·kappa·clamp",
        ),
        (
            "Warp(input: Fbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, salt: 1), strength_m: 2500, base_wavelength_m: 16384, octaves: 2, salt: 2)",
            "f·warp",
        ),
        (
            "Warp(input: Fbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, salt: 1), strength_m: 100, base_wavelength_m: 8192, octaves: 3, salt: 2)",
            "twice",
        ),
        (
            "ErosionFilter(input: Fbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, salt: 1), strength: 1, base_wavelength_m: 2048, octaves: 2, salt: 2)",
            "half",
        ),
        (
            "Curve(input: Field(Flow), points: [(0.0, 1.0), (0.0, 0.5)])",
            "increasing",
        ),
        ("Clamp(input: Field(Flow), min: 1, max: 0)", "min exceeds"),
        ("SmoothMin(a: Const(1), b: Const(2), k: 0)", "k must"),
        ("Const(1e9)", "finite constant"),
    ] {
        let text = format!(r#"Recipe(schema: 1, output: "x", graph: [("x", {node})])"#);
        let message = node_error(&text, "x");
        assert!(message.contains(message_part), "{node}: {message}");
    }
    // Whole-recipe errors.
    for text in [
        r#"Recipe(schema: 2, output: "x", graph: [("x", Const(1))])"#,
        r#"Recipe(schema: 1, output: "y", graph: [("x", Const(1))])"#,
        r#"Recipe(schema: 1, output: "x", graph: [("x", Const(1)), ("x", Const(2))])"#,
    ] {
        assert!(
            matches!(compile(text), Err(LandformError::Recipe(_))),
            "{text}"
        );
    }
}

#[test]
fn ir_is_topological_shares_named_stacks_and_snaps_to_ladders() {
    let program = compile(&format!(
        r#"Recipe(schema: 1, output: "out", graph: [
            ("r", {RIDGE}),
            ("hill", Fbm(base_wavelength_m: 2600, min_wavelength_m: 100, gain: 0.5, salt: 2)),
            ("out", Add(a: Multiply(a: "r", b: "r"), b: Mix(a: "hill", b: "r", t: Field(Uplift)))),
        ])"#
    ))
    .unwrap();
    let ops = program.ops();
    let stacks: Vec<&StackOp> = ops
        .iter()
        .filter_map(|op| match op {
            Op::Stack(s) => Some(&**s),
            _ => None,
        })
        .collect();
    // "r" is emitted once although used three times.
    assert_eq!(stacks.len(), 2);
    assert_eq!(stacks[1].base.wavelength_m(), 2560.0);
    assert_eq!(stacks[1].base.ladder, 1);
    for (i, op) in ops.iter().enumerate() {
        let operands: Vec<usize> = match op {
            Op::Add(a, b) | Op::Multiply(a, b) | Op::Min(a, b) | Op::Max(a, b) => vec![*a, *b],
            Op::SmoothMin(a, b, _) => vec![*a, *b],
            Op::Mix(a, b, t) => vec![*a, *b, *t],
            Op::Clamp(x, ..) | Op::Curve(x, _) | Op::Scale(x, _) => vec![*x],
            _ => vec![],
        };
        assert!(operands.iter().all(|&o| o < i));
    }
    assert_eq!(program.labels().last().map(String::as_str), Some("out"));
}

#[test]
fn structure_identity_ignores_constants_and_tracks_wiring() {
    let base = |gain: &str, kind: &str| {
        compile(&format!(
            r#"Recipe(schema: 1, output: "out", graph: [
                ("n", {kind}(base_wavelength_m: 2048, min_wavelength_m: 32, gain: {gain}, salt: 1)),
                ("out", Multiply(a: "n", b: Field(Uplift))),
            ])"#
        ))
        .unwrap()
    };
    let a = base("0.5", "Fbm");
    let b = base("0.45", "Fbm");
    let c = base("0.5", "Billow");
    assert_eq!(a.structure_identity(), b.structure_identity());
    assert_ne!(a.constants_identity(), b.constants_identity());
    assert_ne!(a.identity(), b.identity());
    assert_ne!(a.structure_identity(), c.structure_identity());
    assert_eq!(a.identity(), base("0.5", "Fbm").identity());
    let set = terra();
    assert_ne!(
        set.landforms()[0].program.structure_identity(),
        set.landforms()[1].program.structure_identity()
    );
}

#[test]
fn monotone_curve_never_overshoots() {
    let program = compile(
        r#"Recipe(schema: 1, output: "c", graph: [
            ("c", Curve(input: Field(Elevation), points: [(-1000, 0.0), (0, 0.1), (10, 0.9), (2000, 1.0)])),
        ])"#,
    )
    .unwrap();
    let Op::Curve(_, curve) = &program.ops()[1] else {
        panic!()
    };
    let mut last = -1.0;
    for i in 0..=30_000 {
        let x = -1500.0 + f64::from(i) * 0.1;
        let (y, dy) = curve.evaluate(x);
        assert!(y >= last - 1e-15 && (0.0..=1.0).contains(&y), "{x}: {y}");
        assert!(dy >= -1e-12 && dy <= curve.lipschitz * (1.0 + 1e-9));
        last = y;
    }
    assert_eq!(curve.evaluate(-5000.0), (0.0, 0.0));
    assert_eq!(curve.evaluate(5000.0), (1.0, 0.0));
}

// --------------------------------------------------------- evaluation

#[test]
fn gradients_match_finite_differences() {
    // Every node except damping and gullies has an exact gradient.
    let program = compile(
        r#"Recipe(schema: 1, output: "out", graph: [
            ("r", RidgedFbm(base_wavelength_m: 2048, min_wavelength_m: 32, gain: 0.5, sharpness: 2.0, salt: 1,
                            anisotropy: Some((stretch: 2, kappa: 2.0, octaves: 3, clamp_km: 64)))),
            ("w", Warp(input: "r", strength_m: 1200, base_wavelength_m: 16384, octaves: 3, salt: 2)),
            ("b", Billow(base_wavelength_m: 1500, min_wavelength_m: 40, gain: 0.55, salt: 3)),
            ("f", Fbm(base_wavelength_m: 700, min_wavelength_m: 20, gain: 0.5, salt: 4)),
            ("v", Curve(input: Field(Flow), points: [(0.0, 1.0), (0.6, 0.3), (1.0, 0.1)])),
            ("m", Mix(a: "w", b: "b", t: Field(Moisture))),
            ("s", SmoothMin(a: "m", b: Scale(input: "f", factor: 0.5), k: 0.3)),
            ("out", Add(a: Multiply(a: "s", b: Multiply(a: Field(Uplift), b: "v")),
                        b: Max(a: Clamp(input: "f", min: -0.2, max: 0.2), b: Min(a: Field(Hardness), b: Const(0.1))))),
        ])"#,
    )
    .unwrap();
    let fields = Synthetic::new();
    let mut rng = Rng(1);
    let p = params(800.0);
    let mut worst = 0.0f64;
    for i in 0..300 {
        let x = if i % 2 == 0 {
            rng.near_boundary(fields.axis, 60_000.0)
        } else {
            rng.sphere()
        };
        for texel in [None, Some(40.0)] {
            let g = program.evaluate(x, &p, texel, &fields).gradient;
            let fd = fd_gradient(&program, x, &p, texel, &fields);
            worst = worst.max((g - fd).length() / (1.0 + g.length()));
        }
    }
    assert!(worst < 1e-5, "worst relative gradient error {worst}");
}

#[test]
fn damped_and_eroded_gradients_are_close_to_finite_differences() {
    // Damping factors and gully directions are held constant in the
    // gradient (module docs of eval.rs); measure how far that is from the
    // exact derivative on the content mountains.
    let set = terra();
    let fields = Synthetic::new();
    let mut rng = Rng(2);
    let p = params(700.0);
    let mut errors = Vec::new();
    for _ in 0..300 {
        let x = rng.near_boundary(fields.axis, 60_000.0);
        let program = &set.landforms()[0].program;
        let g = tangential(program.evaluate(x, &p, None, &fields).gradient, x);
        let fd = tangential(fd_gradient(program, x, &p, None, &fields), x);
        let n_g = (x.normalize() - g).normalize();
        let n_fd = (x.normalize() - fd).normalize();
        errors.push(n_g.dot(n_fd).clamp(-1.0, 1.0).acos().to_degrees());
    }
    errors.sort_by(f64::total_cmp);
    let median = errors[errors.len() / 2];
    let p95 = errors[errors.len() * 95 / 100];
    println!("mountain normal error vs finite differences: median {median:.3}°, p95 {p95:.3}°");
    assert!(median < 3.0 && p95 < 12.0, "median {median}°, p95 {p95}°");
    // With damping and gullies switched off the same recipe is exact.
    let text = std::fs::read_to_string(content_dir().join("recipes/mountains.ron"))
        .unwrap()
        .replace("damp: 1.2", "damp: 0.0")
        .replace("strength: 0.8", "strength: 0.0");
    let options = CompileOptions {
        relief_band_edge_m: set.relief_band_edge_m(),
    };
    let program = Program::compile("test", &recipe(&text), &options).unwrap();
    for _ in 0..50 {
        let x = rng.near_boundary(fields.axis, 60_000.0);
        let g = program.evaluate(x, &p, None, &fields).gradient;
        let fd = fd_gradient(&program, x, &p, None, &fields);
        assert!((g - fd).length() < 1e-5 * (1.0 + g.length()), "{g} vs {fd}");
    }
}

#[test]
fn gully_kernel_is_continuous_bounded_and_differentiable() {
    let octave = LadderOctave {
        ladder: 0,
        level: 6,
    };
    let dir = DVec3::new(0.6, 0.0, 0.8);
    let mut rng = Rng(3);
    for _ in 0..500 {
        let q = rng.sphere();
        let (v, g) = gully(q, octave, 77, dir);
        assert!((-1.0..=1.0).contains(&v));
        let h = 1e-4;
        let fd = DVec3::new(
            gully(q + DVec3::X * h, octave, 77, dir).0 - gully(q - DVec3::X * h, octave, 77, dir).0,
            gully(q + DVec3::Y * h, octave, 77, dir).0 - gully(q - DVec3::Y * h, octave, 77, dir).0,
            gully(q + DVec3::Z * h, octave, 77, dir).0 - gully(q - DVec3::Z * h, octave, 77, dir).0,
        ) / (2.0 * h);
        assert!((g - fd).length() < 1e-6 * (1.0 + g.length()), "{g} vs {fd}");
    }
    // Continuity across cell faces (the 3×3×3 neighbourhood switches).
    let f = octave.frequency_per_m();
    for i in 0..200 {
        let face = (f64::from(i) - 100.0).floor() / f;
        let q = DVec3::new(face, 1234.5, 338_000.25);
        let offset = crate::terrain::surface::ladder::octave_offset(77);
        let q = q - DVec3::X * (offset.x / f);
        let a = gully(q - DVec3::X * 1e-7, octave, 77, dir).0;
        let b = gully(q + DVec3::X * 1e-7, octave, 77, dir).0;
        assert!((a - b).abs() < 1e-5, "{a} vs {b}");
    }
}

#[test]
fn stacks_are_zero_mean() {
    let fields = Synthetic::new();
    for (node, band) in [
        (RIDGE.to_string(), None),
        (
            "RidgedFbm(base_wavelength_m: 2048, min_wavelength_m: 128, gain: 0.5, sharpness: 2.0, salt: 1, anisotropy: Some((stretch: 2, kappa: 2.0, octaves: 3, clamp_km: 64)))".to_string(),
            Some(60_000.0),
        ),
        (
            "Billow(base_wavelength_m: 2048, min_wavelength_m: 64, gain: 0.5, salt: 5)".to_string(),
            None,
        ),
        (
            "RidgedFbm(base_wavelength_m: 1024, min_wavelength_m: 64, gain: 0.6, sharpness: 1.0, salt: 9)".to_string(),
            None,
        ),
    ] {
        let program = compile(&format!(
            r#"Recipe(schema: 1, output: "x", graph: [("x", {node})])"#
        ))
        .unwrap();
        let mut rng = Rng(4);
        let n = 4000;
        let mut sum = 0.0;
        for _ in 0..n {
            let x = match band {
                Some(b) => rng.near_boundary(fields.axis, b),
                None => rng.sphere(),
            };
            sum += program.evaluate(x, &params(1.0), None, &fields).value;
        }
        let mean = sum / f64::from(n);
        assert!(mean.abs() < 0.02, "{node}: mean {mean}");
    }
}

#[test]
fn band_limit_drops_octaves_above_the_limit_and_nests() {
    let fields = Synthetic::new();
    let p = params(1.0);
    // The definition: octave k weighted by octave_weight(f_k, texel).
    let program = compile(
        r#"Recipe(schema: 1, output: "x", graph: [("x", Fbm(base_wavelength_m: 64, min_wavelength_m: 1, gain: 0.5, salt: 3))])"#,
    )
    .unwrap();
    let Op::Stack(stack) = &program.ops()[0] else {
        panic!()
    };
    let relief = node_seed(p.seed, 3, LANE_RELIEF);
    let mut rng = Rng(5);
    for texel in [0.1, 0.7, 2.0, 5.0, 16.0, 40.0] {
        let x = rng.sphere();
        let mut expected = 0.0;
        for k in 0..stack.octaves {
            let octave = stack.octave(k);
            let w = octave_weight(octave.frequency_per_m(), texel);
            if octave.frequency_per_m() >= 0.5 / texel {
                assert_eq!(w, 0.0);
            }
            expected +=
                stack.amplitude(k) * w * octave_noise3(x, octave, octave_seed(relief, octave)).0;
        }
        let got = program.evaluate(x, &p, Some(texel), &fields).value;
        assert!(
            (got - expected).abs() < 1e-12,
            "{texel}: {got} vs {expected}"
        );
    }
    assert_eq!(
        program.evaluate(rng.sphere(), &p, Some(1e4), &fields).value,
        0.0
    );
    // Nesting on the full mountains: a texel and its parent agree exactly
    // where the parent resolves everything; refinement never loses relief;
    // the unresolved bound covers the difference to the complete function.
    let set = terra();
    for (l, p) in set.landforms().iter().zip(set.sample_params(3)) {
        let program = &l.program;
        assert_eq!(program.unresolved_bound_m(&p, 0.25), 0.0);
        let mut previous = 0.0;
        for texel in [1.0, 4.0, 16.0, 64.0, 256.0, 1024.0, 4096.0] {
            let bound = program.unresolved_bound_m(&p, texel);
            assert!(bound >= previous, "{}: {texel}", l.name);
            previous = bound;
            for _ in 0..40 {
                let x = rng.near_boundary(fields.axis, 60_000.0);
                let full = program.evaluate(x, &p, None, &fields).value;
                let coarse = program.evaluate(x, &p, Some(texel), &fields).value;
                assert!(
                    (full - coarse).abs() <= bound * (1.0 + 1e-9) + 1e-9,
                    "{} at {texel} m: {} > {bound}",
                    l.name,
                    (full - coarse).abs()
                );
                if program.unresolved_bound_m(&p, 2.0 * texel) == 0.0 {
                    let parent = program.evaluate(x, &p, Some(2.0 * texel), &fields).value;
                    assert_eq!(coarse, parent);
                }
            }
        }
    }
}

#[test]
fn bounds_cover_sampled_values() {
    let mut programs: Vec<(String, Program)> = terra()
        .landforms()
        .iter()
        .map(|l| (l.name.clone(), l.program.clone()))
        .collect();
    programs.push((
        "mixed".into(),
        compile(&format!(
            r#"Recipe(schema: 1, output: "out", graph: [
                ("r", {RIDGE}),
                ("b", Billow(base_wavelength_m: 1024, min_wavelength_m: 16, gain: 0.6, salt: 2)),
                ("out", Add(a: SmoothMin(a: "r", b: "b", k: 0.5),
                            b: Mix(a: Scale(input: "b", factor: -3), b: Clamp(input: "r", min: -0.1, max: 0.3), t: Field(Moisture)))),
            ])"#
        ))
        .unwrap(),
    ));
    let fields = Synthetic::new();
    let mut rng = Rng(6);
    for (name, program) in &programs {
        for amplitude in [10.0, 700.0] {
            let p = params(amplitude);
            let interval = program.interval_m(&p);
            let bound = program.bound_m(&p);
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for i in 0..1500 {
                let x = if i % 2 == 0 {
                    rng.near_boundary(fields.axis, 60_000.0)
                } else {
                    rng.sphere()
                };
                let v = program.evaluate(x, &p, None, &fields).value;
                lo = lo.min(v);
                hi = hi.max(v);
            }
            assert!(
                interval.lo <= lo && hi <= interval.hi,
                "{name}: [{lo}, {hi}] vs {interval:?}"
            );
            assert!(lo.abs().max(hi.abs()) <= bound);
            println!("{name} at {amplitude} m: bound {bound:.2} m, sampled [{lo:.2}, {hi:.2}] m");
            // Not wildly loose at authored amplitudes. (Gully depth is capped
            // at a 45° hillslope in metres, so the bound is loose for tiny
            // amplitudes whose real slopes never reach the cap.)
            if amplitude >= 300.0 {
                assert!(
                    bound < 10.0 * lo.abs().max(hi.abs()),
                    "{name}: {bound} vs [{lo}, {hi}]"
                );
            }
        }
    }
}

#[test]
fn anisotropic_ridges_run_along_the_boundary() {
    let fields = Synthetic::new();
    let mean_cos = |anisotropy: &str| {
        let program = compile(&format!(
            r#"Recipe(schema: 1, output: "x", graph: [("x", RidgedFbm(base_wavelength_m: 2048, min_wavelength_m: 512, gain: 0.5, sharpness: 2.0, salt: 1{anisotropy}))])"#
        ))
        .unwrap();
        let mut rng = Rng(7);
        let n = 3000;
        let mut sum = 0.0;
        for _ in 0..n {
            let x = rng.near_boundary(fields.axis, 50_000.0);
            let g = tangential(program.evaluate(x, &params(1.0), None, &fields).gradient, x);
            let d = tangential(fields.axis, x);
            sum += (g.dot(d) / (g.length() * d.length())).abs();
        }
        sum / f64::from(n)
    };
    let stretched =
        mean_cos(", anisotropy: Some((stretch: 2, kappa: 2.0, octaves: 3, clamp_km: 64))");
    let isotropic = mean_cos("");
    println!("mean |cos| stretched {stretched:.3}, isotropic {isotropic:.3}");
    assert!(stretched > 0.75, "{stretched}");
    assert!((isotropic - FRAC_2_PI).abs() < 0.04, "{isotropic}");
}

#[test]
fn schema_round_trips_and_inline_nodes_parse() {
    let file = recipe(
        r#"Recipe(schema: 1, output: "out", graph: [
            ("valley", Curve(input: Field(Flow), points: [(0.0, 1.0), (1.0, 0.1)])),
            ("out", Multiply(a: Const(2.0), b: Multiply(a: Field(Uplift), b: "valley"))),
        ])"#,
    );
    let Node::Multiply { a, b } = &file.graph[1].1 else {
        panic!()
    };
    assert_eq!(a, &Input::Node(Box::new(Node::Const(2.0))));
    assert!(matches!(b, Input::Node(_)));
    let text = ron::to_string(&file).unwrap();
    assert_eq!(recipe(&text), file);
}
