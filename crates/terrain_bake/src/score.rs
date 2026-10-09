//! Scoreboard: compare `inspect` outputs of candidates against a reference set.
//!
//! - **Scalars:** each candidate value becomes a z-score against the references
//!   (mean ± spread, with a floor of 15% of |mean| so a single reference still
//!   gives a usable scale).
//! - **Histograms:** Wasserstein-1 distance against the averaged reference
//!   histogram (as a fraction of the histogram range), with the sign of the
//!   mean shift for direction.
//!
//! Each metric has plain-language phrases for "too high" and "too low", so the
//! scoreboard reads like a review ("texture does not flow downslope",
//! "grid-aligned artefacts"). Also writes `compare.png`: the same panels of
//! every input side by side, references first.
//!
//! References must be legitimate: real public-domain data (e.g. LRO NAC DTMs)
//! or our own accepted terrain. Never third-party game assets
//! (`docs/PLANET_DATA_PIPELINE.md` §8).

use crate::inspect::Image;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

struct ScalarSpec {
    key: &'static str,
    meaning: &'static str,
    high: &'static str,
    low: &'static str,
}

const SCALARS: &[ScalarSpec] = &[
    ScalarSpec {
        key: "slope_p50",
        meaning: "median slope",
        high: "too steep overall",
        low: "too flat overall",
    },
    ScalarSpec {
        key: "slope_p99",
        meaning: "steepest 1% slope",
        high: "spikes / cliffs too steep",
        low: "lacks steep faces and crisp rims",
    },
    ScalarSpec {
        key: "hypsometric_integral",
        meaning: "height distribution",
        high: "too much high ground (plateau-like)",
        low: "too much low ground (basin-like)",
    },
    ScalarSpec {
        key: "curvature_kurtosis",
        meaning: "sharpness of ridges and rims",
        high: "too spiky / noisy fine detail",
        low: "ridges and rims too soft",
    },
    ScalarSpec {
        key: "grid_bias",
        meaning: "slope directions at 0/45/90 deg",
        high: "grid-aligned artefacts",
        low: "(fine)",
    },
    ScalarSpec {
        key: "anisotropy",
        meaning: "preferred slope direction",
        high: "too directional / stripy",
        low: "lacks structural grain",
    },
    ScalarSpec {
        key: "flow_alignment",
        meaning: "fine texture runs downslope",
        high: "(fine)",
        low: "texture does not flow downslope",
    },
    ScalarSpec {
        key: "area_exponent",
        meaning: "drainage area distribution",
        high: "too few large drainage basins",
        low: "too many large basins / channels",
    },
    ScalarSpec {
        key: "concavity",
        meaning: "slope falls with drainage area",
        high: "valleys too concave",
        low: "valley profiles unrealistic (not graded)",
    },
    ScalarSpec {
        key: "channel_fraction",
        meaning: "area in channels",
        high: "too channelised",
        low: "too few channels",
    },
    ScalarSpec {
        key: "depressions_per_km2",
        meaning: "closed depressions (craters/pits)",
        high: "too many craters/pits",
        low: "too few craters/pits",
    },
    ScalarSpec {
        key: "depression_depth_ratio_p50",
        meaning: "typical depth/diameter",
        high: "craters/pits too deep",
        low: "craters/pits too shallow",
    },
    ScalarSpec {
        key: "pits_per_km2",
        meaning: "single-cell local minima",
        high: "noisy pits",
        low: "(fine)",
    },
    ScalarSpec {
        key: "ridge_continuity_m2",
        meaning: "connected ridge lines",
        high: "ridges too long / regular",
        low: "ridges broken / fragmented",
    },
    ScalarSpec {
        key: "valley_continuity_m2",
        meaning: "connected valley lines",
        high: "valleys too long / regular",
        low: "valleys broken / fragmented",
    },
    ScalarSpec {
        key: "relief_heterogeneity",
        meaning: "variation of local relief",
        high: "too patchy",
        low: "too uniform everywhere",
    },
    ScalarSpec {
        key: "slope_heterogeneity",
        meaning: "variation of local slope",
        high: "too patchy",
        low: "slopes too uniform",
    },
];

const HISTOGRAMS: &[(&str, &str, &str, &str)] = &[
    (
        "slope_deg",
        "slope angle distribution",
        "steeper",
        "gentler",
    ),
    ("hypsometry", "height distribution", "higher", "lower"),
    (
        "curvature16",
        "16 m curvature distribution",
        "more valley-dominated",
        "more ridge-dominated",
    ),
];

/// Relief per scale: each band's RMS is compared in log10 against the
/// references' mean log10, with tolerance max(reference spread, this many dex)
/// (0.1 dex ≈ ±26 %). Spectral shape was the clearest real-vs-ours signal in the
/// 2026-10-09 diagnosis and no other metric sees it.
const BAND_TOLERANCE_DEX: f64 = 0.1;

/// Continuous loss for searches and ranking (lower is better; about 1 means
/// "within tolerance everywhere"). Terms: the spectrum's RMS log10 deviation over
/// [`SPECTRUM_LOSS_DEX`], plus these scalars, each `(key, log10?, tolerance)`,
/// deviation from the reference mean over the tolerance, uncapped. Ported from
/// the diagnosis objective (`ai/experiments/heightmap-diagnosis/e1-scoreboard`).
const SPECTRUM_LOSS_DEX: f64 = 0.15;
const LOSS_TERMS: &[(&str, bool, f64)] = &[
    ("slope_p50", true, 0.15),
    ("slope_p99", true, 0.20),
    ("depression_depth_ratio_p50", true, 0.20),
    ("curvature_kurtosis", true, 0.35),
    ("flow_alignment", false, 0.10),
    ("depressions_per_km2", true, 0.40),
    ("ridge_continuity_m2", true, 0.50),
    ("valley_continuity_m2", true, 0.50),
];

struct Entry {
    dir: PathBuf,
    metrics: Value,
}

fn bands(e: &Entry) -> Vec<(f64, f64)> {
    e.metrics["band_rms_m"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|b| Some((b[0].as_f64()?, b[1].as_f64()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Per band: (scale m, candidate log10, reference mean log10, reference sd),
/// over the bands every input has. Empty if cell sizes differ by more than 10 %
/// (bands are cell·2^k, so they would not be the same scales).
fn spectrum(c: &Entry, refs: &[Entry]) -> Vec<(f64, f64, f64, f64)> {
    let cell = |e: &Entry| e.metrics["cell_m"].as_f64().unwrap_or(f64::NAN);
    // NaN (missing cell size) also fails the check.
    let same_scale = |r: &Entry| (cell(r) / cell(c) - 1.0).abs() <= 0.1;
    if !refs.iter().all(same_scale) {
        return Vec::new();
    }
    let cb = bands(c);
    let rb: Vec<Vec<(f64, f64)>> = refs.iter().map(bands).collect();
    let n = rb.iter().map(Vec::len).fold(cb.len(), usize::min);
    (0..n)
        .map(|k| {
            let logs: Vec<f64> = rb.iter().map(|b| b[k].1.max(1e-12).log10()).collect();
            let mean = logs.iter().sum::<f64>() / logs.len() as f64;
            let sd =
                (logs.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / logs.len() as f64).sqrt();
            (cb[k].0, cb[k].1.max(1e-12).log10(), mean, sd)
        })
        .collect()
}

/// The continuous loss and its terms (see [`LOSS_TERMS`]).
fn loss(c: &Entry, refs: &[Entry]) -> (f64, Vec<(String, f64)>) {
    let mut terms = Vec::new();
    let spec = spectrum(c, refs);
    if !spec.is_empty() {
        let ms = spec.iter().map(|s| (s.1 - s.2).powi(2)).sum::<f64>() / spec.len() as f64;
        terms.push(("spectrum".to_string(), ms.sqrt() / SPECTRUM_LOSS_DEX));
    }
    for &(key, log, tol) in LOSS_TERMS {
        let Some(v) = c.metrics["scalars"][key].as_f64() else {
            continue;
        };
        let rv: Vec<f64> = refs
            .iter()
            .filter_map(|r| r.metrics["scalars"][key].as_f64())
            .collect();
        if rv.is_empty() {
            continue;
        }
        let f = |x: f64| if log { x.max(1e-9).log10() } else { x };
        let mean = rv.iter().map(|&x| f(x)).sum::<f64>() / rv.len() as f64;
        terms.push((key.to_string(), (f(v) - mean).abs() / tol));
    }
    let total = terms.iter().map(|t| t.1).sum::<f64>() / terms.len().max(1) as f64;
    (total, terms)
}

/// Problems that make a comparison meaningless or suspect.
fn protocol_warnings(refs: &[Entry], cands: &[Entry]) -> Vec<String> {
    let mut out = Vec::new();
    let smooth = |e: &Entry| e.metrics["protocol"]["smooth_m"].as_f64();
    for e in refs.iter().chain(cands) {
        if e.metrics["protocol"].is_null() {
            out.push(format!(
                "{} was inspected by an older tool without a recorded protocol; re-inspect it \
                 (periodic tiles inspected with --detrend then carried a fake wrap-seam crease)",
                label(e)
            ));
        }
    }
    let known: Vec<(String, f64)> = refs
        .iter()
        .chain(cands)
        .filter_map(|e| smooth(e).map(|s| (label(e), s)))
        .collect();
    if let Some((_, first)) = known.first()
        && known.iter().any(|(_, s)| (s - first).abs() > 1e-9)
    {
        out.push(format!(
            "inputs were smoothed differently: {}",
            known
                .iter()
                .map(|(l, s)| format!("{l} {s} m"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

fn load(dir: &Path) -> Result<Entry> {
    let metrics: Value = serde_json::from_slice(
        &fs::read(dir.join("metrics.json"))
            .with_context(|| format!("reading {}/metrics.json", dir.display()))?,
    )?;
    ensure!(
        metrics["schema"] == "mundaris.terrain-inspect.v1",
        "{} is not an inspect output",
        dir.display()
    );
    Ok(Entry {
        dir: dir.to_path_buf(),
        metrics,
    })
}

fn label(e: &Entry) -> String {
    e.metrics["label"].as_str().unwrap_or("?").to_string()
}

fn hist(e: &Entry, key: &str) -> Option<Vec<f64>> {
    e.metrics["histograms"][key]["counts"]
        .as_array()
        .map(|v| v.iter().filter_map(Value::as_f64).collect())
}

/// Wasserstein-1 between two normalized histograms, in units of the range,
/// plus the signed mean shift (candidate − reference), also in range units.
fn wasserstein(a: &[f64], b: &[f64]) -> (f64, f64) {
    let n = a.len().max(1) as f64;
    let (mut ca, mut cb, mut w) = (0.0, 0.0, 0.0);
    let (mut ma, mut mb) = (0.0, 0.0);
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        ca += x;
        cb += y;
        w += (ca - cb).abs();
        let centre = (i as f64 + 0.5) / n;
        ma += x * centre;
        mb += y * centre;
    }
    (w / n, ma - mb)
}

/// Compare candidates with references; writes scoreboard.md/.json and compare.png.
pub fn score(references: &[PathBuf], candidates: &[PathBuf], out: &Path) -> Result<String> {
    ensure!(!references.is_empty(), "need at least one --ref");
    ensure!(!candidates.is_empty(), "need at least one --cand");
    fs::create_dir_all(out)?;
    let refs: Vec<Entry> = references.iter().map(|p| load(p)).collect::<Result<_>>()?;
    let cands: Vec<Entry> = candidates.iter().map(|p| load(p)).collect::<Result<_>>()?;

    let mut md = String::new();
    writeln!(md, "# Terrain scoreboard\n")?;
    writeln!(
        md,
        "References: {}\n",
        refs.iter().map(label).collect::<Vec<_>>().join(", ")
    )?;
    writeln!(
        md,
        "Verdicts: ok |z| < 1 · close |z| < 2 · off otherwise. z uses the reference spread with a 15% floor. Histogram distance is Wasserstein-1 as a fraction of the range (ok < 0.02, close < 0.05). Relief per scale compares band RMS in log10 against the reference mean, tolerance max(reference spread, {BAND_TOLERANCE_DEX} dex). Loss is continuous and uncapped (lower is better, about 1 = within tolerance); rank by loss, the score % is only a summary.\n"
    )?;
    let warnings = protocol_warnings(&refs, &cands);
    for w in &warnings {
        writeln!(md, "> **Warning:** {w}\n")?;
    }
    let mut json_out = Vec::new();
    for c in &cands {
        let mut rows = Vec::new();
        let mut points = 0.0;
        let mut counted = 0.0;
        writeln!(md, "## {}\n", label(c))?;
        writeln!(md, "| metric | candidate | reference | z | verdict |")?;
        writeln!(md, "| --- | --- | --- | --- | --- |")?;
        for spec in SCALARS {
            let Some(value) = c.metrics["scalars"][spec.key].as_f64() else {
                continue;
            };
            let values: Vec<f64> = refs
                .iter()
                .filter_map(|r| r.metrics["scalars"][spec.key].as_f64())
                .collect();
            if values.is_empty() {
                continue;
            }
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let sd = (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64)
                .sqrt();
            let scale = sd.max(0.15 * mean.abs()).max(1e-9);
            let z = (value - mean) / scale;
            let phrase = if z > 0.0 { spec.high } else { spec.low };
            let (verdict, pts) = if z.abs() < 1.0 || phrase == "(fine)" {
                ("ok".to_string(), 1.0)
            } else if z.abs() < 2.0 {
                (
                    format!("close ({})", if z > 0.0 { spec.high } else { spec.low }),
                    0.5,
                )
            } else {
                (
                    format!("**off: {}**", if z > 0.0 { spec.high } else { spec.low }),
                    0.0,
                )
            };
            points += pts;
            counted += 1.0;
            writeln!(
                md,
                "| {} ({}) | {:.4} | {:.4} ± {:.4} | {:+.1} | {} |",
                spec.key, spec.meaning, value, mean, sd, z, verdict
            )?;
            rows.push(json!({"metric": spec.key, "value": value, "ref_mean": mean, "ref_sd": sd, "z": z, "verdict": verdict}));
        }
        for &(key, meaning, higher, lower) in HISTOGRAMS {
            let Some(ch) = hist(c, key) else { continue };
            let rh: Vec<Vec<f64>> = refs.iter().filter_map(|r| hist(r, key)).collect();
            if rh.is_empty() {
                continue;
            }
            let mut avg = vec![0.0; ch.len()];
            for h in &rh {
                for (a, v) in avg.iter_mut().zip(h) {
                    *a += v / rh.len() as f64;
                }
            }
            let (w, shift) = wasserstein(&ch, &avg);
            // A small mean shift relative to the distance means the shape differs
            // (spread or tails), not the overall level.
            let direction = if shift.abs() < w / 3.0 {
                "different spread (narrower or wider)"
            } else if shift > 0.0 {
                higher
            } else {
                lower
            };
            let (verdict, pts) = if w < 0.02 {
                ("ok".to_string(), 1.0)
            } else if w < 0.05 {
                (format!("close (slightly {direction})"), 0.5)
            } else {
                (format!("**off: {direction}**"), 0.0)
            };
            points += pts;
            counted += 1.0;
            writeln!(
                md,
                "| {key} ({meaning}) | W1 {w:.3} | | shift {shift:+.3} | {verdict} |"
            )?;
            rows.push(json!({"metric": key, "wasserstein": w, "shift": shift, "verdict": verdict}));
        }
        let spec = spectrum(c, &refs);
        if spec.is_empty() {
            writeln!(
                md,
                "| relief per scale | | | | not compared (cell sizes differ by more than 10 %) |"
            )?;
        }
        for &(scale, value, mean, sd) in &spec {
            let d = value - mean;
            let z = d / sd.max(BAND_TOLERANCE_DEX);
            let ratio = 10f64.powf(d);
            let phrase = if d > 0.0 { "too rough" } else { "too smooth" };
            let (verdict, pts) = if z.abs() < 1.0 {
                ("ok".to_string(), 1.0)
            } else if z.abs() < 2.0 {
                (format!("close ({phrase} at this scale)"), 0.5)
            } else {
                (
                    format!("**off: {phrase} at this scale ({ratio:.2}×)**"),
                    0.0,
                )
            };
            points += pts;
            counted += 1.0;
            writeln!(
                md,
                "| relief at ~{scale:.0} m (band RMS) | {:.3} m | {:.3} m × {:.2}^±1 | {z:+.1} | {verdict} |",
                10f64.powf(value),
                10f64.powf(mean),
                10f64.powf(sd)
            )?;
            rows.push(json!({"metric": "band_rms", "scale_m": scale, "value": 10f64.powf(value), "ref_geomean": 10f64.powf(mean), "ratio": ratio, "z": z, "verdict": verdict}));
        }
        let (loss_total, terms) = loss(c, &refs);
        let total = if counted > 0.0 { points / counted } else { 0.0 };
        writeln!(
            md,
            "\n**Loss {loss_total:.2}** ({}) · **Score {:.0}%** ({points}/{counted} points)\n",
            terms
                .iter()
                .map(|(k, v)| format!("{k} {v:.1}"))
                .collect::<Vec<_>>()
                .join(", "),
            total * 100.0
        )?;
        json_out.push(json!({
            "label": label(c),
            "loss": loss_total,
            "loss_terms": terms.iter().map(|(k, v)| json!({"term": k, "value": v})).collect::<Vec<_>>(),
            "score": total,
            "rows": rows,
        }));
    }
    // Ranking by loss up front, so the best candidate is visible at a glance.
    let mut ranked: Vec<(String, f64, f64)> = json_out
        .iter()
        .map(|c| {
            (
                c["label"].as_str().unwrap_or("?").to_string(),
                c["loss"].as_f64().unwrap_or(f64::INFINITY),
                c["score"].as_f64().unwrap_or(0.0),
            )
        })
        .collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut summary = String::from(
        "## Ranking (by loss)\n\n| rank | candidate | loss | score |\n| --- | --- | --- | --- |\n",
    );
    for (k, (name, l, s)) in ranked.iter().enumerate() {
        writeln!(
            summary,
            "| {} | {name} | {l:.2} | {:.0}% |",
            k + 1,
            s * 100.0
        )?;
    }
    if let Some(at) = md.find("\n## ") {
        md.insert_str(at + 1, &format!("{summary}\n"));
    }
    fs::write(out.join("scoreboard.md"), &md)?;
    fs::write(
        out.join("scoreboard.json"),
        serde_json::to_string_pretty(&json!({"warnings": warnings, "candidates": json_out}))?,
    )?;
    compare_image(&refs, &cands, &out.join("compare.png"))?;
    Ok(md)
}

/// Rows: each input (references first); columns: the same diagnostic panels.
fn compare_image(refs: &[Entry], cands: &[Entry], path: &Path) -> Result<()> {
    let columns = [
        ("hillshade.png", "HILLSHADE NW"),
        ("slope.png", "SLOPE"),
        ("curvature.png", "CURVATURE"),
        ("drainage.png", "DRAINAGE"),
        ("depressions.png", "DEPRESSIONS"),
        ("crop_hillshade_nw.png", "1:1 CROP"),
    ];
    let tile = 300;
    let label_w = 0;
    let header = 26;
    let row_label = 22;
    let entries: Vec<(&Entry, bool)> = refs
        .iter()
        .map(|e| (e, true))
        .chain(cands.iter().map(|e| (e, false)))
        .collect();
    let mut img = Image::new(
        label_w + columns.len() * (tile + 4) + 4,
        header + entries.len() * (tile + row_label + 4),
        [24, 24, 28],
    );
    for (k, (_, name)) in columns.iter().enumerate() {
        img.text(4 + k * (tile + 4) + 2, 6, name, 2, [255, 255, 255]);
    }
    for (row, (entry, is_ref)) in entries.iter().enumerate() {
        let y = header + row * (tile + row_label + 4);
        let tag = if *is_ref { "REF" } else { "CANDIDATE" };
        img.text(
            6,
            y + 4,
            &format!("{tag}: {}", label(entry)),
            2,
            if *is_ref {
                [140, 220, 140]
            } else {
                [240, 200, 120]
            },
        );
        for (k, (file, _)) in columns.iter().enumerate() {
            let p = entry.dir.join("panels").join(file);
            if let Ok(src) = Image::load(&p) {
                img.blit(&downscale(&src, tile), 4 + k * (tile + 4), y + row_label);
            }
        }
    }
    img.save(path)
}

fn downscale(src: &Image, size: usize) -> Image {
    let factor = (src.w.max(src.h) as f64 / size as f64).max(1.0);
    let (w, h) = (
        (src.w as f64 / factor) as usize,
        (src.h as f64 / factor) as usize,
    );
    let mut out = Image::new(w, h, [0, 0, 0]);
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = ((x as f64 * factor) as usize, (y as f64 * factor) as usize);
            let (x1, y1) = (
                (((x + 1) as f64 * factor) as usize).min(src.w),
                (((y + 1) as f64 * factor) as usize).min(src.h),
            );
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for sy in y0..y1.max(y0 + 1) {
                for sx in x0..x1.max(x0 + 1) {
                    let c = src.get(sx.min(src.w - 1), sy.min(src.h - 1));
                    for k in 0..3 {
                        acc[k] += u32::from(c[k]);
                    }
                    n += 1;
                }
            }
            out.set(x, y, acc.map(|v| (v / n.max(1)) as u8));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasserstein_is_zero_for_equal_and_signed_for_shifts() {
        let a = [0.0, 1.0, 0.0, 0.0];
        let b = [0.0, 0.0, 1.0, 0.0];
        assert_eq!(wasserstein(&a, &a).0, 0.0);
        let (w, shift) = wasserstein(&b, &a);
        assert!((w - 0.25).abs() < 1e-12 && shift > 0.0);
    }

    fn entry(cell: f64, band_rms: &[f64], smooth: Option<f64>) -> Entry {
        let bands: Vec<Value> = band_rms
            .iter()
            .enumerate()
            .map(|(k, &v)| json!([cell * 2f64.powi(k as i32 + 1), v]))
            .collect();
        let mut metrics = json!({"label": "t", "cell_m": cell, "band_rms_m": bands, "scalars": {}});
        if let Some(s) = smooth {
            metrics["protocol"] = json!({"plane_removed": false, "smooth_m": s});
        }
        Entry {
            dir: PathBuf::new(),
            metrics,
        }
    }

    #[test]
    fn spectrum_loss_sees_a_tenfold_rough_surface() {
        let real = [0.05, 0.13, 0.26, 0.48, 1.1];
        let refs = [entry(2.0, &real, Some(3.0))];
        let same = entry(1.953, &real, Some(3.0));
        let rough = entry(1.953, &real.map(|v| v * 10.0), Some(3.0));
        assert!(loss(&same, &refs).0 < 1e-9);
        // One decade everywhere = 1 dex RMS over a 0.15 dex scale.
        assert!((loss(&rough, &refs).0 - 1.0 / SPECTRUM_LOSS_DEX).abs() < 1e-9);
        // Different cell sizes are different scales: not compared.
        assert!(spectrum(&entry(4.0, &real, Some(3.0)), &refs).is_empty());
    }

    #[test]
    fn protocol_warnings_flag_old_and_mismatched_inspections() {
        let refs = [entry(2.0, &[1.0], Some(3.0))];
        assert!(protocol_warnings(&refs, &[entry(2.0, &[1.0], Some(3.0))]).is_empty());
        assert_eq!(
            protocol_warnings(&refs, &[entry(2.0, &[1.0], None)]).len(),
            1
        );
        assert_eq!(
            protocol_warnings(&refs, &[entry(2.0, &[1.0], Some(0.0))]).len(),
            1
        );
    }
}
