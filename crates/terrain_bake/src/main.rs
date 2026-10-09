//! `terrain_bake bake <recipe.json> <library_dir>` | `terrain_bake validate <bundle_dir>` |
//! `terrain_bake preview <recipe.json> <out_dir> [resolution]` |
//! `terrain_bake inspect <bundle_dir | heightmap> --out <dir> [options]` |
//! `terrain_bake score --out <dir> --ref <inspect_dir>... --cand <inspect_dir>...`

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const INSPECT_USAGE: &str = "terrain_bake inspect <bundle_dir | heightmap> --out <dir> [--label NAME] \n[--format r16le|r16be|f32le] [--size N | --width W --height H] [--footprint M | --cell M] \n[--relief M] [--non-periodic] [--crop X,Y] [--window X,Y,W,H | --auto-window N [--window-index K]] [--detrend] [--smooth-m S]";

/// Value following `--flag`, if present.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn inspect(args: &[String]) -> Result<()> {
    use astrum_terrain_bake::inspect;
    let path = PathBuf::from(args.first().context(INSPECT_USAGE)?);
    let out = PathBuf::from(flag(args, "--out").context(INSPECT_USAGE)?);
    let num = |name: &str| -> Result<Option<f64>> {
        flag(args, name)
            .map(|v| v.parse::<f64>().with_context(|| format!("{name} {v}")))
            .transpose()
    };
    let size = num("--size")?;
    let width = num("--width")?.or(size).unwrap_or(0.0) as usize;
    let height = num("--height")?.or(size).unwrap_or(0.0) as usize;
    let cell_m = match (num("--cell")?, num("--footprint")?) {
        (Some(c), _) => c,
        (None, Some(f)) if width > 0 => f / width as f64,
        _ => 0.0,
    };
    let source = inspect::Source {
        path: path.clone(),
        format: flag(args, "--format").unwrap_or("r16le").to_string(),
        width,
        height,
        cell_m,
        relief_m: num("--relief")?.unwrap_or(1000.0),
        periodic: !args.iter().any(|a| a == "--non-periodic"),
        window: flag(args, "--window")
            .map(|v| -> Result<(usize, usize, usize, usize)> {
                let p: Vec<usize> = v
                    .split(',')
                    .map(str::parse)
                    .collect::<std::result::Result<_, _>>()?;
                anyhow::ensure!(p.len() == 4, "--window X,Y,W,H");
                Ok((p[0], p[1], p[2], p[3]))
            })
            .transpose()?,
        auto_window: num("--auto-window")?
            .map(|n| -> Result<(usize, Option<usize>)> {
                Ok((n as usize, num("--window-index")?.map(|k| k as usize)))
            })
            .transpose()?,
    };
    let mut grid = inspect::Grid::load(&source).with_context(|| INSPECT_USAGE)?;
    if args.iter().any(|a| a == "--detrend") {
        grid.detrend();
    }
    if let Some(sigma) = num("--smooth-m")? {
        grid.smooth_in_place(sigma);
    }
    let label = flag(args, "--label")
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let crop = flag(args, "--crop")
        .map(|v| -> Result<(usize, usize)> {
            let (x, y) = v.split_once(',').context("--crop X,Y")?;
            Ok((x.parse()?, y.parse()?))
        })
        .transpose()?;
    let metrics = inspect::inspect(&grid, &label, &out, crop)?;
    println!("{}", serde_json::to_string_pretty(&metrics["scalars"])?);
    Ok(())
}

fn score(args: &[String]) -> Result<()> {
    let (mut refs, mut cands, mut out) = (Vec::new(), Vec::new(), None);
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1).cloned().context("missing value")?;
        match args[i].as_str() {
            "--ref" => refs.push(PathBuf::from(value)),
            "--cand" => cands.push(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            other => bail!("unknown score argument {other}"),
        }
        i += 2;
    }
    let out = out.context("score needs --out")?;
    print!(
        "{}",
        astrum_terrain_bake::score::score(&refs, &cands, &out)?
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, rest @ ..] if command == "inspect" => inspect(rest)?,
        [command, rest @ ..] if command == "score" => score(rest)?,
        [command, recipe, library] if command == "bake" => {
            let dir =
                astrum_terrain_bake::bake_and_publish(Path::new(recipe), Path::new(library))?;
            let metadata = astrum_terrain_bake::bundle::validate(&dir)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "published": dir,
                    "height": metadata.height,
                    "seam": metadata.seam,
                    "bake": metadata.bake,
                    "channels": metadata.channels.iter().map(|c| (&c.file, &c.sha256)).collect::<Vec<_>>(),
                }))?
            );
        }
        [command, recipe, out, rest @ ..] if command == "preview" && rest.len() <= 1 => {
            let resolution = rest.first().map_or(Ok(1024), |r| r.parse())?;
            println!(
                "{}",
                astrum_terrain_bake::preview_base(Path::new(recipe), Path::new(out), resolution)?
            );
        }
        [command, file, order, side, footprint, relief] if command == "metrics" => {
            // Statistics of a square r16 heightmap rescaled to `relief` metres.
            let n: usize = side.parse()?;
            let bytes = std::fs::read(file)?;
            anyhow::ensure!(bytes.len() == n * n * 2, "size does not match {n}²");
            let raw: Vec<f64> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&p| {
                    f64::from(if order == "be" {
                        u16::from_be_bytes(p)
                    } else {
                        u16::from_le_bytes(p)
                    })
                })
                .collect();
            let (lo, hi) = raw
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                    (a.min(v), b.max(v))
                });
            let relief: f64 = relief.parse()?;
            let height: Vec<f64> = raw
                .iter()
                .map(|v| (v - lo) / (hi - lo).max(1.0) * relief)
                .collect();
            let footprint: f64 = footprint.parse()?;
            println!(
                "{}",
                astrum_terrain_bake::pipeline::metrics(&height, n, footprint / n as f64)
            );
        }
        [command, bundle] if command == "validate" => {
            let metadata = astrum_terrain_bake::bundle::validate(Path::new(bundle))?;
            println!(
                "valid {} ({}², {} channels)",
                metadata.bundle_id,
                metadata.resolution,
                metadata.channels.len()
            );
        }
        _ => bail!(
            "usage: terrain_bake bake <recipe.json> <library_dir> | terrain_bake validate <bundle_dir> | terrain_bake preview <recipe.json> <out_dir> [resolution]"
        ),
    }
    Ok(())
}
