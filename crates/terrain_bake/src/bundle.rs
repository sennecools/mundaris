//! `mundaris.terrain-bundle.v1` writer and validator. See
//! `docs/PLANET_DATA_PIPELINE.md` §3 for the format contract.

use crate::bake::{BakeOutput, StageReport};
use crate::derive::{DerivedFields, downsample};
use crate::gpu::AdapterIdentity;
use crate::preview;
use crate::recipe::Recipe;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "mundaris.terrain-bundle.v1";
pub const GENERATOR: &str = "mundaris_terrain_bake";
pub const GENERATOR_VERSION: &str = "erosion-pipes-1";
const METADATA_FILE: &str = "bundle.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub components: u32,
    /// `u16` or `u8`.
    pub sample_type: String,
    pub byte_order: String,
    pub meaning: String,
    pub units: String,
    /// Decoded value range of the encoding.
    pub decoded_range: [f64; 2],
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HeightEncoding {
    /// height_m = offset_m + value / 65535 * range_m
    pub offset_m: f64,
    pub range_m: f64,
    pub decoded_min_m: f64,
    pub decoded_max_m: f64,
    pub max_quantization_error_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SeamReport {
    /// Largest |Δh| between the first and last row/column (wrap neighbours).
    pub wrap_max_step_m: f64,
    /// Largest |Δh| between interior neighbours.
    pub interior_max_step_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakeEnvironment {
    pub adapter: serde_json::Value,
    pub total_seconds: f64,
    pub derive_seconds: f64,
    pub peak_gpu_allocated_bytes: u64,
    pub stages: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub format: String,
    pub schema_version: u32,
    pub bundle_id: String,
    pub generator: String,
    pub generator_version: String,
    pub provenance: String,
    pub external_asset_dependencies: Vec<String>,
    pub seed: u64,
    pub recipe_sha256: String,
    pub recipe: serde_json::Value,
    pub footprint_m: f64,
    pub resolution: u32,
    pub spacing_m: f64,
    pub sample_location: String,
    pub row_order: String,
    pub wrap: String,
    pub height: HeightEncoding,
    pub seam: SeamReport,
    pub channels: Vec<Channel>,
    pub bake: BakeEnvironment,
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn extrema(values: &[f64]) -> (f64, f64) {
    values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
}

pub fn seam_report(height_m: &[f64], n: usize) -> SeamReport {
    let mut wrap = 0.0f64;
    let mut interior = 0.0f64;
    for y in 0..n {
        for x in 0..n {
            let h = height_m[y * n + x];
            let right = height_m[y * n + (x + 1) % n];
            let up = height_m[((y + 1) % n) * n + x];
            let (dx, dy) = ((h - right).abs(), (h - up).abs());
            if x + 1 == n {
                wrap = wrap.max(dx);
            } else {
                interior = interior.max(dx);
            }
            if y + 1 == n {
                wrap = wrap.max(dy);
            } else {
                interior = interior.max(dy);
            }
        }
    }
    SeamReport {
        wrap_max_step_m: wrap,
        interior_max_step_m: interior,
    }
}

fn unit_u8(values: &[f64]) -> Vec<u8> {
    values
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    fs::write(dir.join(name), bytes).with_context(|| format!("writing {name}"))
}

pub struct PublishInputs<'a> {
    pub recipe: &'a Recipe,
    pub recipe_bytes: &'a [u8],
    pub output: &'a BakeOutput,
    pub derived: &'a DerivedFields,
    pub adapter: &'a AdapterIdentity,
    pub total_seconds: f64,
    pub derive_seconds: f64,
    pub peak_gpu_allocated_bytes: u64,
}

/// Write a bundle into `library_dir/<id>` through a staging directory, validate
/// it, and publish by rename. An existing bundle is never overwritten.
pub fn publish(library_dir: &Path, inputs: &PublishInputs<'_>) -> Result<PathBuf> {
    let recipe = inputs.recipe;
    let output = inputs.output;
    let final_dir = library_dir.join(&recipe.id);
    ensure!(
        !final_dir.exists(),
        "{} already exists; published bundles are immutable (bump id or remove it explicitly)",
        final_dir.display()
    );
    let staging = library_dir.join(format!(".{}.staging-{}", recipe.id, std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).context("clearing stale staging directory")?;
    }
    fs::create_dir_all(staging.join("previews")).context("creating staging directory")?;

    let n = output.resolution as usize;
    let (min_m, max_m) = extrema(&output.height_m);
    let range_m = (max_m - min_m).max(1.0e-9);
    let mut height_bytes = Vec::with_capacity(n * n * 2);
    let mut max_error = 0.0f64;
    let mut decoded_min = f64::INFINITY;
    let mut decoded_max = f64::NEG_INFINITY;
    for &h in &output.height_m {
        let value = ((h - min_m) / range_m * 65535.0)
            .round()
            .clamp(0.0, 65535.0) as u16;
        let decoded = min_m + f64::from(value) / 65535.0 * range_m;
        max_error = max_error.max((decoded - h).abs());
        decoded_min = decoded_min.min(decoded);
        decoded_max = decoded_max.max(decoded);
        height_bytes.extend_from_slice(&value.to_le_bytes());
    }
    write_file(&staging, "height.r16", &height_bytes)?;

    let channel_n = inputs.recipe.derived.channel_resolution as usize;
    let factor = n / channel_n;
    let wetness = downsample(&inputs.derived.wetness, n, factor);
    let exposure = downsample(&inputs.derived.exposure, n, factor);
    let spawn = downsample(&inputs.derived.spawn, n, factor);
    let clim: Vec<u8> = unit_u8(&wetness)
        .into_iter()
        .zip(unit_u8(&exposure))
        .flat_map(|(w, e)| [w, e])
        .collect();
    let spawn_bytes = unit_u8(&spawn);
    write_file(&staging, "clim.rg8", &clim)?;
    write_file(&staging, "spawn.r8", &spawn_bytes)?;

    let side = output.resolution;
    let channel_side = inputs.recipe.derived.channel_resolution;
    let channels = vec![
        Channel {
            file: "height.r16".into(),
            width: side,
            height: side,
            components: 1,
            sample_type: "u16".into(),
            byte_order: "little-endian".into(),
            meaning: "physical surface height; height_m = offset_m + value / 65535 * range_m".into(),
            units: "m".into(),
            decoded_range: [decoded_min, decoded_max],
            sha256: sha256_hex(&height_bytes),
            bytes: height_bytes.len() as u64,
        },
        Channel {
            file: "clim.rg8".into(),
            width: channel_side,
            height: channel_side,
            components: 2,
            sample_type: "u8".into(),
            byte_order: "none".into(),
            meaning: "R = wetness (flow accumulation, deposition, shelter); G = exposure (local relief, convexity); local modifiers of global humidity/temperature".into(),
            units: "unit interval (value / 255)".into(),
            decoded_range: [0.0, 1.0],
            sha256: sha256_hex(&clim),
            bytes: clim.len() as u64,
        },
        Channel {
            file: "spawn.r8".into(),
            width: channel_side,
            height: channel_side,
            components: 1,
            sample_type: "u8".into(),
            byte_order: "none".into(),
            meaning: "object spawn support (flow, incision, steepness)".into(),
            units: "unit interval (value / 255)".into(),
            decoded_range: [0.0, 1.0],
            sha256: sha256_hex(&spawn_bytes),
            bytes: spawn_bytes.len() as u64,
        },
    ];

    let metadata = Metadata {
        format: FORMAT.into(),
        schema_version: 1,
        bundle_id: recipe.id.clone(),
        generator: GENERATOR.into(),
        generator_version: GENERATOR_VERSION.into(),
        provenance: "original-bake".into(),
        external_asset_dependencies: Vec::new(),
        seed: recipe.seed,
        recipe_sha256: Recipe::sha256(inputs.recipe_bytes),
        recipe: serde_json::to_value(recipe)?,
        footprint_m: recipe.footprint_m,
        resolution: side,
        spacing_m: output.cell_size_m,
        sample_location: "pixel_centres".into(),
        row_order: "row-major; first row is v = 0; x increases with u".into(),
        wrap: "periodic".into(),
        height: HeightEncoding {
            offset_m: min_m,
            range_m,
            decoded_min_m: decoded_min,
            decoded_max_m: decoded_max,
            max_quantization_error_m: max_error,
        },
        seam: seam_report(&output.height_m, n),
        channels,
        bake: BakeEnvironment {
            adapter: serde_json::to_value(inputs.adapter)?,
            total_seconds: inputs.total_seconds,
            derive_seconds: inputs.derive_seconds,
            peak_gpu_allocated_bytes: inputs.peak_gpu_allocated_bytes,
            stages: serde_json::to_value::<&[StageReport]>(&output.stages)?,
        },
    };
    write_file(
        &staging,
        METADATA_FILE,
        serde_json::to_string_pretty(&metadata)?.as_bytes(),
    )?;
    preview::write_all(&staging.join("previews"), output, inputs.derived)?;

    validate(&staging).context("staged bundle failed validation")?;
    fs::rename(&staging, &final_dir).context("publishing bundle")?;
    Ok(final_dir)
}

/// Re-hash and re-decode a bundle and check its metadata invariants.
pub fn validate(dir: &Path) -> Result<Metadata> {
    let metadata: Metadata =
        serde_json::from_slice(&fs::read(dir.join(METADATA_FILE)).context("reading bundle.json")?)
            .context("parsing bundle.json")?;
    ensure!(
        metadata.format == FORMAT,
        "unexpected format {}",
        metadata.format
    );
    ensure!(
        metadata.wrap == "periodic",
        "library bundles must be periodic"
    );
    ensure!(
        metadata.provenance == "original-bake" && metadata.external_asset_dependencies.is_empty(),
        "bundle must be an original bake without external assets"
    );
    let recipe: Recipe = serde_json::from_value(metadata.recipe.clone())
        .context("embedded recipe does not parse")?;
    recipe.validate()?;
    ensure!(
        recipe.output_resolution() == metadata.resolution,
        "resolution mismatch"
    );

    let mut height = None;
    for channel in &metadata.channels {
        let bytes = fs::read(dir.join(&channel.file))
            .with_context(|| format!("reading {}", channel.file))?;
        let sample_bytes = match channel.sample_type.as_str() {
            "u16" => 2,
            "u8" => 1,
            other => bail!("unsupported sample type {other}"),
        };
        let expected = u64::from(channel.width)
            * u64::from(channel.height)
            * u64::from(channel.components)
            * sample_bytes;
        ensure!(
            bytes.len() as u64 == expected && channel.bytes == expected,
            "{} has {} bytes, expected {expected}",
            channel.file,
            bytes.len()
        );
        ensure!(
            sha256_hex(&bytes) == channel.sha256,
            "{} hash mismatch",
            channel.file
        );
        if channel.file == "height.r16" {
            ensure!(
                channel.byte_order == "little-endian",
                "height must be little-endian"
            );
            height = Some(bytes);
        }
    }
    let bytes = height.context("bundle has no height.r16 channel")?;
    let encoding = &metadata.height;
    let decoded: Vec<f64> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| {
            encoding.offset_m + f64::from(u16::from_le_bytes(pair)) / 65535.0 * encoding.range_m
        })
        .collect();
    let (lo, hi) = extrema(&decoded);
    ensure!(
        (lo - encoding.decoded_min_m).abs() < 1e-9 && (hi - encoding.decoded_max_m).abs() < 1e-9,
        "decoded height extrema differ from metadata"
    );
    let seam = seam_report(&decoded, metadata.resolution as usize);
    ensure!(
        seam.wrap_max_step_m <= seam.interior_max_step_m + 2.0 * encoding.max_quantization_error_m,
        "periodic seam step {} m exceeds the interior maximum {} m",
        seam.wrap_max_step_m,
        seam.interior_max_step_m
    );
    Ok(metadata)
}
