//! CPU-only end-to-end tests of the exemplar synthesis pipeline filter and the
//! `public-domain-derived` bundle provenance, on a synthetic GeoTIFF.

use astrum_terrain_bake::bake_and_publish;
use astrum_terrain_bake::bundle;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "astrum-exemplar-test-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Minimal little-endian, strip-based, uncompressed float32 GeoTIFF.
fn write_tiff(path: &Path, w: usize, h: usize, data: &[f32]) {
    const ROWS: usize = 64;
    let strips = h.div_ceil(ROWS);
    let entries = 10usize;
    let ifd_len = 2 + entries * 12 + 4;
    let offsets_at = 8 + ifd_len;
    let counts_at = offsets_at + 4 * strips;
    let data_at = counts_at + 4 * strips;
    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(entries as u16).to_le_bytes());
    let mut entry = |tag: u16, kind: u16, count: u32, value: u32| {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    };
    entry(256, 4, 1, w as u32);
    entry(257, 4, 1, h as u32);
    entry(258, 3, 1, 32);
    entry(259, 3, 1, 1);
    entry(262, 3, 1, 1);
    entry(273, 4, strips as u32, offsets_at as u32);
    entry(277, 3, 1, 1);
    entry(278, 4, 1, ROWS as u32);
    entry(279, 4, strips as u32, counts_at as u32);
    entry(339, 3, 1, 3);
    out.extend_from_slice(&0u32.to_le_bytes());
    let strip_bytes = |s: usize| (ROWS.min(h - s * ROWS)) * w * 4;
    let mut at = data_at;
    for s in 0..strips {
        out.extend_from_slice(&(at as u32).to_le_bytes());
        at += strip_bytes(s);
    }
    for s in 0..strips {
        out.extend_from_slice(&(strip_bytes(s) as u32).to_le_bytes());
    }
    for v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, out).unwrap();
}

/// Rolling hills plus fine noise, with a no-data hole in one corner.
fn terrain(w: usize, h: usize) -> Vec<f32> {
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            if x < 40.0 && y < 40.0 {
                return -3.4e38;
            }
            let hash = (i as u32).wrapping_mul(2_654_435_761) >> 20;
            (30.0 * (x * 0.05).sin() * (y * 0.04).cos()
                + 8.0 * (x * 0.21 + y * 0.17).sin()
                + f64::from(hash) * 0.002) as f32
        })
        .collect()
}

fn derived() -> Value {
    json!({
        "channel_resolution": 32,
        "flow_exponent": 1.1,
        "relief_radius_m": 20,
        "wetness": {"flow": 0.8, "deposition": 0.4, "shelter": 0.3, "bias": 0},
        "exposure": {"relief": 0.8, "convexity": 0.4, "bias": 0.5},
        "spawn": {"flow": 0.5, "incision": 0.5, "steepness": 0.4, "steep_tangent": 1, "bias": -0.2}
    })
}

fn constant_graph() -> Value {
    json!({
        "nodes": [
            {"id": "zero", "op": {"kind": "constant", "value": 0.0}},
            {"id": "hardness", "op": {"kind": "constant", "value": 0.5}}
        ],
        "height": "zero",
        "hardness": "hardness"
    })
}

fn exemplar_recipe(id: &str, tiff: &Path, sha256: &str) -> Value {
    let level =
        |patch: u32| json!({"patch_px": patch, "overlap_px": 8, "candidates": 24, "feather_px": 1});
    json!({
        "schema": 2,
        "id": id,
        "version": 1,
        "seed": 5,
        "footprint_m": 128,
        "graph": constant_graph(),
        "pipeline": [{
            "name": "exemplar",
            "filter": {
                "kind": "exemplar_synthesis",
                "resolution": 64,
                "sources": [{
                    "product": "SYNTH",
                    "path": tiff.to_string_lossy(),
                    "sha256": sha256,
                    // The first window avoids the no-data corner; the second overlaps it.
                    "windows": [[48, 48, 128], [160, 96, 128]]
                }],
                "levels": [level(16), level(16)],
                "alpha": 0.5,
                "lambda": 0.05,
                "seed": 9
            }
        }],
        "derived": derived(),
        "output_relief_m": null
    })
}

fn sha(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[test]
fn exemplar_bake_publishes_public_domain_derived_bundle() {
    let dir = scratch_dir("bake");
    let tiff = dir.join("synthetic.tif");
    write_tiff(&tiff, 320, 256, &terrain(320, 256));
    let hash = sha(&tiff);
    let recipe_path = dir.join("recipe.json");
    write_json(
        &recipe_path,
        &exemplar_recipe("exemplar_test", &tiff, &hash),
    );
    let library = dir.join("library");
    fs::create_dir_all(&library).unwrap();

    let bundle_dir = bake_and_publish(&recipe_path, &library).unwrap();
    let metadata = bundle::validate(&bundle_dir).unwrap();
    assert_eq!(metadata.provenance, "public-domain-derived");
    assert_eq!(metadata.resolution, 64);
    assert_eq!(
        metadata.external_asset_dependencies,
        vec![format!("NASA LROC NAC DTM SYNTH sha256:{hash} windows:2")]
    );
    assert!(metadata.credit.as_deref().unwrap().contains("NASA"));
    assert!(metadata.height.range_m > 1.0);
    // Periodic by construction: the wrap step is within the interior maximum.
    assert!(metadata.seam.wrap_max_step_m <= metadata.seam.interior_max_step_m + 1e-6);

    // The same recipe bakes to the same bytes.
    let again = dir.join("library2");
    fs::create_dir_all(&again).unwrap();
    let second = bake_and_publish(&recipe_path, &again).unwrap();
    for file in ["height.r16", "clim.rg8", "spawn.r8"] {
        assert_eq!(
            fs::read(bundle_dir.join(file)).unwrap(),
            fs::read(second.join(file)).unwrap(),
            "{file} differs between bakes"
        );
    }

    // Provenance edits that do not match the recipe are rejected.
    let path = bundle_dir.join("bundle.json");
    let text = fs::read_to_string(&path).unwrap();
    let original: Value = serde_json::from_str(&text).unwrap();
    type Edit = Box<dyn Fn(&mut Value)>;
    let edits: Vec<(&str, Edit)> = vec![
        (
            "no credit",
            Box::new(|v| {
                v.as_object_mut().unwrap().remove("credit");
            }),
        ),
        ("blank credit", Box::new(|v| v["credit"] = json!("  "))),
        (
            "no dependencies",
            Box::new(|v| v["external_asset_dependencies"] = json!([])),
        ),
        (
            "wrong dependencies",
            Box::new(|v| v["external_asset_dependencies"] = json!(["something else"])),
        ),
        (
            "claimed original",
            Box::new(|v| v["provenance"] = json!("original-bake")),
        ),
        (
            "unknown provenance",
            Box::new(|v| v["provenance"] = json!("temporary-reference-input")),
        ),
    ];
    for (name, edit) in edits {
        let mut value = original.clone();
        edit(&mut value);
        write_json(&path, &value);
        assert!(
            bundle::validate(&bundle_dir).is_err(),
            "{name} was accepted"
        );
    }
    fs::write(&path, &text).unwrap();
    bundle::validate(&bundle_dir).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn exemplar_bake_rejects_tampered_or_missing_sources() {
    let dir = scratch_dir("sources");
    let tiff = dir.join("synthetic.tif");
    write_tiff(&tiff, 320, 256, &terrain(320, 256));
    let library = dir.join("library");
    fs::create_dir_all(&library).unwrap();
    let recipe_path = dir.join("recipe.json");

    // Wrong hash.
    write_json(
        &recipe_path,
        &exemplar_recipe("exemplar_bad_hash", &tiff, &"0".repeat(64)),
    );
    let err = format!(
        "{:#}",
        bake_and_publish(&recipe_path, &library).unwrap_err()
    );
    assert!(err.contains("sha256 mismatch"), "{err}");

    // Missing file.
    write_json(
        &recipe_path,
        &exemplar_recipe("exemplar_missing", &dir.join("absent.tif"), &sha(&tiff)),
    );
    let err = format!(
        "{:#}",
        bake_and_publish(&recipe_path, &library).unwrap_err()
    );
    assert!(err.contains("cannot read"), "{err}");

    // A window over no-data.
    let mut recipe = exemplar_recipe("exemplar_nodata", &tiff, &sha(&tiff));
    recipe["pipeline"][0]["filter"]["sources"][0]["windows"] = json!([[0, 0, 128]]);
    write_json(&recipe_path, &recipe);
    let err = format!(
        "{:#}",
        bake_and_publish(&recipe_path, &library).unwrap_err()
    );
    assert!(err.contains("no-data"), "{err}");

    // A window outside the raster.
    let mut recipe = exemplar_recipe("exemplar_outside", &tiff, &sha(&tiff));
    recipe["pipeline"][0]["filter"]["sources"][0]["windows"] = json!([[300, 200, 128]]);
    write_json(&recipe_path, &recipe);
    assert!(bake_and_publish(&recipe_path, &library).is_err());

    // Nothing was published.
    assert_eq!(fs::read_dir(&library).unwrap().count(), 0);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pipeline_original_bake_keeps_original_provenance_without_credit() {
    let dir = scratch_dir("original");
    let recipe = json!({
        "schema": 2,
        "id": "original_pipeline_test",
        "version": 1,
        "seed": 1,
        "footprint_m": 128,
        "graph": {
            "nodes": [
                {"id": "terrain", "op": {"kind": "fbm", "frequency": 2, "octaves": 3, "gain": 0.5, "seed": 1}},
                {"id": "height", "op": {"kind": "scale", "input": "terrain", "factor": 20}},
                {"id": "hardness", "op": {"kind": "constant", "value": 0.5}}
            ],
            "height": "height",
            "hardness": "hardness"
        },
        "pipeline": [{"filter": {"kind": "base", "resolution": 64}}],
        "derived": derived(),
        "output_relief_m": null
    });
    let recipe_path = dir.join("recipe.json");
    write_json(&recipe_path, &recipe);
    let library = dir.join("library");
    fs::create_dir_all(&library).unwrap();
    let bundle_dir = bake_and_publish(&recipe_path, &library).unwrap();
    let metadata = bundle::validate(&bundle_dir).unwrap();
    assert_eq!(metadata.provenance, "original-bake");
    assert!(metadata.credit.is_none());
    let text = fs::read_to_string(bundle_dir.join("bundle.json")).unwrap();
    assert!(
        !text.contains("\"credit\""),
        "old-format bundle gained a credit key"
    );
    fs::remove_dir_all(&dir).unwrap();
}
