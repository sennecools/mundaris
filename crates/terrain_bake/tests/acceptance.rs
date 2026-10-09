//! Slice acceptance checks for `terrain-bake-v1` (docs/PLANET_DATA_PIPELINE.md §9):
//! CPU/GPU base-noise agreement, repeatability, mass conservation, periodic
//! seams, bundle round trip and recipe validation.
//!
//! GPU tests skip when no adapter is available unless `ASTRUM_REQUIRE_GPU=1`.

use astrum_terrain_bake::bake::{BakeOutput, bake};
use astrum_terrain_bake::bundle::{self, PublishInputs, seam_report};
use astrum_terrain_bake::derive::derive;
use astrum_terrain_bake::gpu::Gpu;
use astrum_terrain_bake::noise::base_sample;
use astrum_terrain_bake::recipe::{Process, Recipe, StageRecipe};
use std::fs;
use std::path::{Path, PathBuf};

type Mutation = Box<dyn Fn(&mut Recipe)>;

fn recipe_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/test_smoke.json")
}

fn smoke_recipe() -> (Recipe, Vec<u8>) {
    Recipe::load(&recipe_path()).expect("test recipe loads")
}

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(gpu) => Some(gpu),
        Err(error) if std::env::var("ASTRUM_REQUIRE_GPU").as_deref() != Ok("1") => {
            eprintln!("skipping GPU test: {error:#}");
            None
        }
        Err(error) => panic!("GPU required but unavailable: {error:#}"),
    }
}

/// Small two-stage recipe (fluvial 64² → hydraulic 128²) for fast debug runs.
fn small_recipe() -> Recipe {
    let (mut recipe, _) = smoke_recipe();
    recipe.stages = vec![
        StageRecipe {
            resolution: 64,
            iterations: 60,
            process: Process::Fluvial,
            detail_scale: 0.0,
            gullies: false,
        },
        StageRecipe {
            resolution: 128,
            iterations: 64,
            process: Process::Hydraulic,
            detail_scale: 0.15,
            gullies: false,
        },
    ];
    recipe.hydraulic.settle_iterations = 16;
    recipe.derived.channel_resolution = 64;
    recipe.validate().expect("small recipe is valid");
    recipe
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "astrum-terrain-bake-{}-{name}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn gpu_base_noise_matches_cpu_f64_reference() {
    let Some(mut gpu) = gpu() else { return };
    let (plain, _) = smoke_recipe();
    // Same recipe with rotated, slope-damped octaves on every layer.
    let mut rotated = plain.clone();
    let base = rotated.base.as_mut().unwrap();
    for layer in base
        .layers
        .iter_mut()
        .chain(base.warp.as_mut().map(|w| &mut w.layer))
        .chain(std::iter::once(&mut base.hardness.layer))
    {
        layer.rotate_octaves = true;
        layer.slope_damping = 0.8;
        layer.domain = Some([[1, 2], [-1, 1]]);
    }
    rotated.validate().unwrap();
    for (name, recipe) in [("plain", plain), ("rotated+damped+domain", rotated)] {
        let n = 128u32;
        let (height, hardness) = gpu
            .base(recipe.base.as_ref().unwrap(), recipe.seed, n)
            .unwrap();
        let mut max_height_error = 0.0f64;
        let mut max_hardness_error = 0.0f64;
        for y in 0..n {
            for x in 0..n {
                let u = (f64::from(x) + 0.5) / f64::from(n);
                let v = (f64::from(y) + 0.5) / f64::from(n);
                let (h, k) = base_sample(recipe.base.as_ref().unwrap(), recipe.seed, u, v);
                let i = (y * n + x) as usize;
                max_height_error = max_height_error.max((f64::from(height[i]) - h).abs());
                max_hardness_error = max_hardness_error.max((f64::from(hardness[i]) - k).abs());
            }
        }
        eprintln!(
            "{name}: base noise max |GPU − CPU|: height {max_height_error:.3e} m, hardness {max_hardness_error:.3e}"
        );
        // f32 evaluation of a unit-normalized sum scaled by amplitude_m: allow 1e-5 relative.
        let tolerance_m = 1.0e-5 * recipe.base.as_ref().unwrap().amplitude_m;
        assert!(
            max_height_error <= tolerance_m,
            "{name}: height error {max_height_error} m exceeds {tolerance_m} m"
        );
        assert!(
            max_hardness_error <= 1.0e-4,
            "{name}: hardness error {max_hardness_error}"
        );
    }
}

#[test]
fn bake_is_bitwise_repeatable_and_periodic() {
    let Some(mut gpu) = gpu() else { return };
    let recipe = small_recipe();
    let first = bake(&recipe, &mut gpu).unwrap();
    let second = bake(&recipe, &mut gpu).unwrap();
    let bits = |o: &BakeOutput| -> Vec<u64> {
        o.height_m
            .iter()
            .chain(&o.erosion_delta_m)
            .map(|v| v.to_bits())
            .collect()
    };
    assert_eq!(
        bits(&first),
        bits(&second),
        "bake is not bitwise repeatable"
    );

    let n = first.resolution as usize;
    let seam = seam_report(&first.height_m, n);
    eprintln!("seam: {seam:?}");
    assert!(seam.interior_max_step_m > 0.0);
    assert!(
        seam.wrap_max_step_m <= seam.interior_max_step_m,
        "wrap step {} exceeds interior step {}",
        seam.wrap_max_step_m,
        seam.interior_max_step_m
    );
    // Mean |Δh| across the wrap edges must look like an ordinary interior edge; a
    // non-periodic field has wrap steps on the order of its relief.
    let h = &first.height_m;
    let (mut wrap_sum, mut interior_sum) = (0.0, 0.0);
    for i in 0..n {
        wrap_sum += (h[i * n + n - 1] - h[i * n]).abs() + (h[(n - 1) * n + i] - h[i]).abs();
        interior_sum += (h[i * n + n / 2 - 1] - h[i * n + n / 2]).abs()
            + (h[(n / 2 - 1) * n + i] - h[(n / 2) * n + i]).abs();
    }
    eprintln!(
        "mean step: wrap {:.3} m, interior {:.3} m",
        wrap_sum / (2 * n) as f64,
        interior_sum / (2 * n) as f64
    );
    assert!(
        wrap_sum <= 2.0 * interior_sum,
        "mean wrap step {wrap_sum} is not comparable to the interior {interior_sum}"
    );
}

#[test]
fn closed_hydraulic_stage_conserves_mass() {
    let Some(mut gpu) = gpu() else { return };
    let mut recipe = small_recipe();
    recipe.stages = vec![StageRecipe {
        resolution: 64,
        iterations: 256,
        process: Process::Hydraulic,
        detail_scale: 0.0,
        gullies: false,
    }];
    recipe.hydraulic.outlet_fraction = 0.0;
    recipe.hydraulic.fill_slope = 0.0;
    recipe.derived.channel_resolution = 64;
    recipe.derived.relief_radius_m = 200.0;
    recipe.validate().unwrap();
    let output = bake(&recipe, &mut gpu).unwrap();
    let stage = &output.stages[0];
    eprintln!("closed stage: {stage:?}");
    assert!(stage.outlet_level_m.is_none());
    assert!(
        stage.output_relief_m != stage.input_relief_m,
        "erosion did not change the terrain"
    );
    assert!(
        stage.mass_relative_change < 1.0e-5,
        "closed stage mass change {} exceeds 1e-5",
        stage.mass_relative_change
    );
}

#[test]
fn published_bundle_validates_and_rejects_tampering() {
    let Some(mut gpu) = gpu() else { return };
    let recipe = small_recipe();
    let recipe_bytes = serde_json::to_vec_pretty(&recipe).unwrap();
    let output = bake(&recipe, &mut gpu).unwrap();
    let derived = derive(
        &output.height_m,
        &output.erosion_delta_m,
        output.resolution as usize,
        output.cell_size_m,
        &recipe.derived,
    );
    let library = scratch_dir("roundtrip");
    let info = bundle::RecipeInfo::layered(&recipe).unwrap();
    let inputs = PublishInputs {
        recipe: &info,
        recipe_bytes: &recipe_bytes,
        output: &output,
        derived: &derived,
        adapter: &gpu.identity,
        total_seconds: 0.0,
        derive_seconds: 0.0,
        peak_gpu_allocated_bytes: gpu.peak_allocated_bytes,
    };
    let dir = bundle::publish(&library, &inputs).unwrap();

    // Published, no staging left behind, and immutable.
    let entries: Vec<String> = fs::read_dir(&library)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec![recipe.id.clone()]);
    assert!(bundle::publish(&library, &inputs).is_err());

    let metadata = bundle::validate(&dir).unwrap();
    assert_eq!(metadata.provenance, "original-bake");
    assert!((metadata.height.offset_m + 0.5 * metadata.height.range_m).abs() < 1e-9);
    // Decoded centred heights reproduce the bake about its relief midpoint.
    let (lo, hi) = output
        .height_m
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &h| {
            (a.min(h), b.max(h))
        });
    assert!((metadata.height.range_m - (hi - lo)).abs() < 1e-9);
    assert!(metadata.height.max_quantization_error_m <= 0.5 * (hi - lo) / 65535.0 + 1e-12);

    // A flipped height sample fails the hash check.
    let height_path = dir.join("height.r16");
    let original = fs::read(&height_path).unwrap();
    let mut tampered = original.clone();
    tampered[100] ^= 1;
    fs::write(&height_path, &tampered).unwrap();
    assert!(bundle::validate(&dir).is_err());
    fs::write(&height_path, &original).unwrap();
    bundle::validate(&dir).unwrap();

    // Edited metadata (non-periodic wrap, external provenance) fails.
    let metadata_path = dir.join("bundle.json");
    let text = fs::read_to_string(&metadata_path).unwrap();
    for (from, to) in [
        ("\"wrap\": \"periodic\"", "\"wrap\": \"clamp\""),
        (
            "\"provenance\": \"original-bake\"",
            "\"provenance\": \"temporary-reference-input\"",
        ),
    ] {
        assert!(text.contains(from), "metadata lacks {from}");
        fs::write(&metadata_path, text.replace(from, to)).unwrap();
        assert!(bundle::validate(&dir).is_err(), "{to} was accepted");
    }
    fs::write(&metadata_path, &text).unwrap();
    bundle::validate(&dir).unwrap();
    fs::remove_dir_all(&library).unwrap();
}

#[test]
fn recipe_validation_rejects_invalid_recipes() {
    let (recipe, _) = smoke_recipe();
    recipe.validate().unwrap();
    let cases: Vec<(&str, Mutation)> = vec![
        ("schema", Box::new(|r| r.schema = 2)),
        ("id", Box::new(|r| r.id = "Bad-Id".into())),
        ("footprint", Box::new(|r| r.footprint_m = 1.0)),
        (
            "no layers",
            Box::new(|r| r.base.as_mut().unwrap().layers.clear()),
        ),
        (
            "odd lacunarity",
            Box::new(|r| r.base.as_mut().unwrap().layers[0].lacunarity = 0),
        ),
        (
            "non-power-of-two stage",
            Box::new(|r| r.stages[0].resolution = 100),
        ),
        (
            "stage not doubling",
            Box::new(|r| r.stages[1].resolution = 1024),
        ),
        ("NaN rain", Box::new(|r| r.hydraulic.rain = f64::NAN)),
        (
            "fluvial without outlet",
            Box::new(|r| r.hydraulic.outlet_fraction = 0.0),
        ),
        (
            "channel resolution",
            Box::new(|r| r.derived.channel_resolution = 4096),
        ),
        (
            "relief radius",
            Box::new(|r| r.derived.relief_radius_m = 1.0e6),
        ),
    ];
    for (name, mutate) in cases {
        let mut bad = recipe.clone();
        mutate(&mut bad);
        assert!(bad.validate().is_err(), "{name} was accepted");
    }
    // Unknown fields are rejected at parse time.
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(recipe_path()).unwrap()).unwrap();
    value["unexpected"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Recipe>(value).is_err());
}
