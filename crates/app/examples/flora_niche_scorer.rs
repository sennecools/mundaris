#![allow(clippy::needless_range_loop, clippy::type_complexity)] // scorer: index-parallel tables
//! Flora niche scorer: where each species grows on Rust, from the CPU
//! reference of the GPU placement field (astrum_flora::scatter) sampled on
//! the Tier A world map (CPU oracle: climate, elevation, slope, sediment).
//!
//! `cargo run --release -p astrum_app --example flora_niche_scorer -- [body] [out_dir] [species_dir]`
//!
//! Writes `<out>/niche-report.md` (density per climate bin and slope bin per
//! species, treeline and water checks, clearing statistics, far-tint vs
//! placed density per climate bin) and `<out>/niche-map.png` (equirectangular
//! species map). Exits non-zero when a hard check fails.
//!
//! Limits: Tier A resolution (km scale), so slopes are macro slopes and
//! rivers/lakes are not water here; the GPU pass also drops water pages.

use std::fmt::Write as _;

use astrum_app::shared_system::SharedTestSystem;
use astrum_flora::niche::{Layer, Site};
use astrum_flora::scatter::Placement;
use astrum_flora::{SpeciesFile, load_species_for_body};
use astrum_world::terrain::SurfaceGenerator;
use astrum_world::terrain::producer::ProducerRecipe;
use glam::DVec3;

const CELLS_PER_UNIT: f64 = 32768.0; // 2^(SCATTER_CELL_BITS - 1)

/// Face and base-cell coordinates of a direction (dominant-axis gnomonic
/// chart, statistically equivalent to the GPU node charts).
fn cells(d: DVec3) -> (u32, f64, f64) {
    let a = d.abs();
    let (face, u, v) = if a.x >= a.y && a.x >= a.z {
        (if d.x >= 0.0 { 0 } else { 1 }, d.y / a.x, d.z / a.x)
    } else if a.y >= a.z {
        (if d.y >= 0.0 { 2 } else { 3 }, d.x / a.y, d.z / a.y)
    } else {
        (if d.z >= 0.0 { 4 } else { 5 }, d.x / a.z, d.y / a.z)
    };
    (face, (u + 1.0) * CELLS_PER_UNIT, (v + 1.0) * CELLS_PER_UNIT)
}

struct Sample {
    site: Site,
    /// Tier A rock hardness 0..1.
    hardness: f64,
    face: u32,
    ci: f64,
    cj: f64,
}

#[derive(Default, Clone)]
struct Bin {
    n: f64,
    tree: f64,
    shrub: f64,
    near_cover: f64,
    far_cover: f64,
    m5_tree: f64,
    species: Vec<f64>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let body_name = args.get(1).map(String::as_str).unwrap_or("Rust");
    let out = std::path::PathBuf::from(args.get(2).map(String::as_str).unwrap_or("target/flora-niche"));
    std::fs::create_dir_all(&out).unwrap();
    let species_dir = args.get(3).map(String::as_str).unwrap_or("content/flora/species");
    let species: Vec<SpeciesFile> = load_species_for_body(
        std::path::Path::new(species_dir),
        std::path::Path::new("content/flora/planets"),
        &body_name.to_lowercase(),
    )
    .unwrap();
    let placement = Placement { species: &species };
    let ns = species.len();

    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(97).unwrap()).unwrap();
    let (_, body) = loaded.system.bodies().find(|(_, b)| b.name() == body_name).expect("body");
    let radius = body.properties().reference_radius_m();
    let recipe = SurfaceGenerator::new(body.surface_definition().expect("surface"), radius)
        .unwrap()
        .producer_recipe()
        .unwrap();
    let ProducerRecipe::World(world) = &recipe else { panic!("{body_name} has no world map") };
    let t0 = std::time::Instant::now();
    let maps = world.field.maps().unwrap();
    println!("Tier A CPU bake {:.1} s", t0.elapsed().as_secs_f64());
    let shape = maps.shape.as_ref().expect("shape maps (sediment)");

    // Fibonacci sphere samples.
    let count = 400_000usize;
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let mut samples = Vec::with_capacity(count);
    for i in 0..count {
        let z = 1.0 - 2.0 * (i as f64 + 0.5) / count as f64;
        let r = (1.0 - z * z).sqrt();
        let a = golden * i as f64;
        let d = DVec3::new(r * a.cos(), r * a.sin(), z);
        let (h, grad) = maps.sample(0, d);
        let (t, m) = maps.climate(0, d);
        let slope = (grad.length() / radius).atan();
        let (face, ci, cj) = cells(d);
        samples.push(Sample {
            site: Site { temperature_c: t, moisture: m, height_m: h, slope, sediment: shape.sediment_mips[0].bilinear(d) },
            hardness: shape.hardness_mips[0].bilinear(d),
            face,
            ci,
            cj,
        });
    }

    let mut fails: Vec<String> = Vec::new();
    let tb = |t: f64| (((t + 30.0) / 5.0).floor().clamp(0.0, 15.0)) as usize; // -30..50 °C
    let mb = |m: f64| ((m * 10.0).floor().clamp(0.0, 9.0)) as usize;
    let sb = |s: f64| ((s.to_degrees() / 5.0).floor().clamp(0.0, 11.0)) as usize;
    let mut climate = vec![vec![Bin { species: vec![0.0; ns], ..Default::default() }; 10]; 16];
    let mut slope_bins = vec![Bin { species: vec![0.0; ns], ..Default::default() }; 12];
    let (mut land, mut water_tree, mut above_line) = (0usize, 0.0f64, vec![0.0f64; ns]);
    let mut treeline = vec![f64::MIN; ns];
    let mut sediment_hist = [0usize; 10];
    let (mut suitable, mut clearing) = (0usize, 0usize);
    let (mut m5_suitable, mut m5_tree_sum, mut tree_sum) = (0usize, 0.0f64, 0.0f64);
    let (mut m5_suit_sum, mut suit_sum) = (0.0f64, 0.0f64);
    let mut map_px = vec![[0u8; 3]; 1024 * 512];

    for s in &samples {
        let site = &s.site;
        let f = placement.forest(s.face, s.ci, s.cj, 0.0, site);
        let far = placement.forest(s.face, s.ci, s.cj, 400.0, site);
        if site.height_m <= 0.0 {
            water_tree += f.tree + f.shrub;
            continue;
        }
        land += 1;
        sediment_hist[((site.sediment * 10.0).floor() as usize).min(9)] += 1;
        // Expected species shares within each layer (exact pick probabilities).
        let mut share = vec![0.0; ns];
        for (layer, p) in [(Layer::Canopy, f.tree), (Layer::Shrub, f.shrub)] {
            let w: Vec<f64> = (0..ns)
                .map(|k| if species[k].niche.layer == layer { placement.suit(k, site).powi(2) * species[k].niche.prior } else { 0.0 })
                .collect();
            let tot: f64 = w.iter().sum();
            if tot > 0.0 {
                for k in 0..ns {
                    share[k] += p * w[k] / tot;
                }
            }
        }
        for k in 0..ns {
            let ni = &species[k].niche;
            if site.height_m > ni.height_m.max + ni.height_m.falloff {
                above_line[k] += share[k];
            }
            if share[k] > 0.01 {
                treeline[k] = treeline[k].max(site.height_m);
            }
        }
        // Placed cover averaged over 16 independent noise draws at this
        // climate (cells ~40 km apart), so the check compares expectations,
        // not one spatially correlated noise realisation.
        let near_cover = (0..16)
            .map(|k| placement.forest(s.face, s.ci + 4001.0 * k as f64, s.cj + 2999.0 * k as f64, 0.0, site).cover)
            .sum::<f64>()
            / 16.0;
        let far_cover = far.cover;
        let (m5s, m5t) = m5_tree(s.face, s.ci, s.cj, site);
        if std::env::var_os("FLORA_DEBUG").is_some() && tb(site.temperature_c) == 9 && mb(site.moisture) == 5 && m5t > 0.8 && f.tree < 0.5 {
            eprintln!("{site:?} suits {:?} canopy {:.3} tree {:.3} core {:.3} m5 {m5s:.3}/{m5t:.3}", (0..ns).map(|k| placement.suit(k, site)).collect::<Vec<_>>(), f.canopy, f.tree, f.core);
        }
        for bin in [&mut climate[tb(site.temperature_c)][mb(site.moisture)], &mut slope_bins[sb(site.slope)]] {
            bin.n += 1.0;
            bin.tree += f.tree;
            bin.shrub += f.shrub;
            bin.near_cover += near_cover;
            bin.far_cover += far_cover;
            bin.m5_tree += m5t;
            for k in 0..ns {
                bin.species[k] += share[k];
            }
        }
        if m5s > 0.5 {
            m5_suitable += 1;
        }
        m5_tree_sum += m5t;
        m5_suit_sum += m5s;
        suit_sum += f.canopy;
        tree_sum += f.tree;
        if f.canopy > 0.5 {
            suitable += 1;
            if f.core < 0.5 {
                clearing += 1;
            }
        }
    }

    // Capture poses: the first sample matching each climate band.
    let bands: [(&str, Box<dyn Fn(&Site, &astrum_flora::scatter::ForestSite, &[f64]) -> bool>); 6] = [
        ("broadleaf-forest", Box::new(|s, f, sh| s.temperature_c > 15.0 && s.moisture > 0.5 && s.slope < 0.1 && f.tree > 0.6 && sh[0] > 0.7 * f.tree)),
        ("conifer-forest", Box::new(|s, f, sh| s.temperature_c < 0.0 && s.moisture > 0.5 && s.slope < 0.15 && f.tree > 0.5 && sh[2] > 0.8 * f.tree)),
        ("mixed-forest", Box::new(|s, f, sh| s.moisture > 0.5 && f.tree > 0.5 && sh[0] > 0.35 * f.tree && sh[2] > 0.35 * f.tree)),
        ("treeline", Box::new(|s, f, _| s.height_m > 3100.0 && f.tree > 0.05 && f.tree < 0.35)),
        ("dry-fringe", Box::new(|s, f, _| s.moisture > 0.2 && s.moisture < 0.38 && s.temperature_c > 10.0 && f.shrub > 0.05 && f.tree < 0.25)),
        ("coast", Box::new(|s, f, _| s.height_m > 3.0 && s.height_m < 30.0 && f.tree > 0.4)),
    ];
    let mut poses = String::from("[
");
    for (name, test) in &bands {
        let found = samples.iter().enumerate().find(|(_, s)| {
            if s.site.height_m <= 0.0 {
                return false;
            }
            let f = placement.forest(s.face, s.ci, s.cj, 0.0, &s.site);
            let sh: Vec<f64> = (0..ns).map(|k| f.tree * placement.suit(k, &s.site).powi(2)).collect();
            let tot: f64 = sh.iter().sum::<f64>().max(1e-12);
            let sh: Vec<f64> = sh.iter().map(|v| v / tot * f.tree).collect();
            test(&s.site, &f, &sh)
        });
        if let Some((i, s)) = found {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / count as f64;
            let rr = (1.0 - z * z).sqrt();
            let a = golden * i as f64;
            let d = DVec3::new(rr * a.cos(), rr * a.sin(), z);
            let pos = d * (radius + s.site.height_m + 30.0);
            let q = horizon_quat(d, 0.12).normalize();
            let _ = writeln!(poses, "  {{\"name\": \"{name}\", \"position_body_m\": [{:.3}, {:.3}, {:.3}], \"orientation_xyzw\": [{:.15}, {:.15}, {:.15}, {:.15}], \"site\": \"T {:.1} M {:.2} h {:.0} slope {:.1} sed {:.2}\"}},", pos.x, pos.y, pos.z, q.x, q.y, q.z, q.w, s.site.temperature_c, s.site.moisture, s.site.height_m, s.site.slope.to_degrees(), s.site.sediment);
        } else {
            println!("no sample for band {name}");
        }
    }
    // Rock poses: steep ground on hard and on soft bedrock.
    for (name, test) in [
        ("granite-rocks", (|s: &Sample| s.hardness > 0.7 && s.site.slope > 12f64.to_radians()) as fn(&Sample) -> bool),
        ("sandstone-rocks", |s: &Sample| s.hardness < 0.4 && s.site.slope > 1.5f64.to_radians()),
    ] {
        if let Some((i, s)) = samples.iter().enumerate().find(|(_, s)| s.site.height_m > 50.0 && test(s)) {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / count as f64;
            let rr = (1.0 - z * z).sqrt();
            let a = golden * i as f64;
            let d = DVec3::new(rr * a.cos(), rr * a.sin(), z);
            let pos = d * (radius + s.site.height_m + 30.0);
            let q = horizon_quat(d, 0.2).normalize();
            let _ = writeln!(poses, "  {{\"name\": \"{name}\", \"position_body_m\": [{:.3}, {:.3}, {:.3}], \"orientation_xyzw\": [{:.15}, {:.15}, {:.15}, {:.15}], \"site\": \"hardness {:.2} slope {:.1} h {:.0} M {:.2}\"}},", pos.x, pos.y, pos.z, q.x, q.y, q.z, q.w, s.hardness, s.site.slope.to_degrees(), s.site.height_m, s.site.moisture);
        } else {
            println!("no sample for {name}");
        }
    }
    let hard: Vec<f64> = samples.iter().filter(|s| s.site.height_m > 0.0).map(|s| s.hardness).collect();
    let soft = hard.iter().filter(|h| **h < 0.5).count() as f64 / hard.len().max(1) as f64;
    println!("land with hardness < 0.5 (sandstone): {:.1} %", 100.0 * soft);
    poses.push_str("]
");
    std::fs::write(out.join("poses.json"), &poses).unwrap();

    // Equirectangular species map (separate pass at pixel centres).
    for py in 0..512 {
        for px in 0..1024 {
            let lat = (0.5 - (py as f64 + 0.5) / 512.0) * std::f64::consts::PI;
            let lon = ((px as f64 + 0.5) / 1024.0 - 0.5) * std::f64::consts::TAU;
            let d = DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin());
            let (h, grad) = maps.sample(0, d);
            let (t, m) = maps.climate(0, d);
            let site = Site { temperature_c: t, moisture: m, height_m: h, slope: (grad.length() / radius).atan(), sediment: shape.sediment_mips[0].bilinear(d) };
            let (face, ci, cj) = cells(d);
            let c = if h <= 0.0 {
                [0.05, 0.12, 0.25]
            } else {
                let f = placement.forest(face, ci, cj, 0.0, &site);
                let ground = [0.45 - 0.2 * m, 0.38 - 0.05 * m, 0.25 - 0.1 * m];
                let ground = if t < -10.0 { [0.85, 0.87, 0.9] } else { ground };
                let mut best = (0.0, None);
                for k in 0..ns {
                    let sh = placement.suit(k, &site).powi(2) * species[k].niche.prior;
                    if sh > best.0 {
                        best = (sh, Some(k));
                    }
                }
                let col = match best.1 {
                    Some(k) => {
                        let o = species[k].look.organ;
                        [o[0] as f64 * 3.0, o[1] as f64 * 3.0, o[2] as f64 * 3.0]
                    }
                    None => ground,
                };
                let a = (f.tree + 0.5 * f.shrub).clamp(0.0, 1.0);
                [0, 1, 2].map(|i| ground[i] * (1.0 - a) + col[i] * a)
            };
            map_px[py * 1024 + px] = c.map(|v| (v.clamp(0.0, 1.0).sqrt() * 255.0) as u8);
        }
    }
    write_png(&out.join("niche-map.png"), 1024, 512, &map_px);

    // Report.
    let mut r = String::new();
    let _ = writeln!(r, "# Flora niche report: {body_name}\n");
    let _ = writeln!(r, "{} samples, {land} on land. Species: {}.\n", samples.len(), species.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "));
    let _ = writeln!(r, "## Hard checks\n");
    let _ = writeln!(r, "- Plant density under sea level: {water_tree:.4} (must be 0)");
    if water_tree > 0.0 {
        fails.push(format!("plants under water {water_tree:.4}"));
    }
    for k in 0..ns {
        let ni = &species[k].niche;
        let _ = writeln!(
            r,
            "- {}: density above its treeline ({:.0} m + {:.0} m fade): {:.4} (must be 0); highest site with share > 1 %: {:.0} m",
            species[k].name, ni.height_m.max, ni.height_m.falloff, above_line[k], treeline[k]
        );
        if above_line[k] > 0.0 {
            fails.push(format!("{} above treeline", species[k].name));
        }
    }
    let _ = writeln!(r, "\n## Clearings\n");
    let _ = writeln!(
        r,
        "- Canopy-suitable land (best canopy suitability > 0.5): {:.1} % of land; clearings (core < 0.5) within it: {:.1} %",
        100.0 * suitable as f64 / land.max(1) as f64,
        100.0 * clearing as f64 / suitable.max(1) as f64
    );
    let _ = writeln!(
        r,
        "- Look check against the M5 rules before niches: canopy-suitable land {:.1} % (was {:.1} %), mean tree density on land {:.4} (was {:.4})",
        100.0 * suitable as f64 / land.max(1) as f64,
        100.0 * m5_suitable as f64 / land.max(1) as f64,
        tree_sum / land.max(1) as f64,
        m5_tree_sum / land.max(1) as f64
    );
    let _ = writeln!(r, "- Mean canopy suitability on land {:.3} (M5 rules {:.3})", suit_sum / land.max(1) as f64, m5_suit_sum / land.max(1) as f64);
    let (patches, mean_ha, cover) = clearing_patches(&placement, &samples);
    let _ = writeln!(r, "- Local 2.56 km windows in suitable forest: {patches:.1} clearings per km², mean clearing {mean_ha:.2} ha, forest core cover {:.1} %", cover * 100.0);
    let _ = writeln!(r, "\n## Sediment (soil) on land\n");
    let _ = writeln!(r, "| bin | {} |", (0..10).map(|i| format!("{:.1}", i as f64 / 10.0)).collect::<Vec<_>>().join(" | "));
    let _ = writeln!(r, "|---|{}|", "---|".repeat(10));
    let _ = writeln!(r, "| % land | {} |", sediment_hist.iter().map(|c| format!("{:.1}", 100.0 * *c as f64 / land.max(1) as f64)).collect::<Vec<_>>().join(" | "));

    let _ = writeln!(r, "\n## Tree density by climate (mean tree probability per base cell; species share)\n");
    let _ = writeln!(r, "Rows: temperature °C bin start; columns: moisture. Cell: tree density / dominant species (share). Blank: < 20 samples.\n");
    let _ = writeln!(r, "| T \\ M | {} |", (0..10).map(|i| format!("{:.1}", i as f64 / 10.0)).collect::<Vec<_>>().join(" | "));
    let _ = writeln!(r, "|---|{}|", "---|".repeat(10));
    let mut worst_tint = 0.0f64;
    let mut worst_bin = String::new();
    for (ti, row) in climate.iter().enumerate() {
        if row.iter().all(|b| b.n < 20.0) {
            continue;
        }
        let cells: Vec<String> = row
            .iter()
            .map(|b| {
                if b.n < 20.0 {
                    return String::new();
                }
                let (k, v) = b.species.iter().enumerate().fold((0, 0.0), |a, (k, v)| if *v > a.1 { (k, *v) } else { a });
                let tot: f64 = b.species.iter().sum();
                if tot <= 1e-9 {
                    format!("{:.2} (M5 {:.2})", b.tree / b.n, b.m5_tree / b.n)
                } else {
                    format!("{:.2} {} ({:.0}%) (M5 {:.2})", b.tree / b.n, &species[k].name[..4.min(species[k].name.len())], 100.0 * v / tot, b.m5_tree / b.n)
                }
            })
            .collect();
        let _ = writeln!(r, "| {} | {} |", -30 + 5 * ti as i32, cells.join(" | "));
        for b in row.iter().filter(|b| b.n >= 200.0 && b.near_cover / b.n > 0.05) {
            let rel = ((b.far_cover - b.near_cover) / b.near_cover).abs();
            if rel > worst_tint {
                worst_bin = format!("T {} M {:.1}: placed {:.3}, far {:.3}, {} samples", -30 + 5 * ti as i32, 0.0, b.near_cover / b.n, b.far_cover / b.n, b.n);
            }
            worst_tint = worst_tint.max(rel);
        }
    }
    let _ = writeln!(r, "\n## Density by slope (macro slope, Tier A)\n");
    let _ = writeln!(r, "| slope ° | samples | tree | shrub | {} |", species.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" | "));
    let _ = writeln!(r, "|---|---|---|---|{}|", "---|".repeat(ns));
    for (i, b) in slope_bins.iter().enumerate().filter(|(_, b)| b.n > 0.0) {
        let _ = writeln!(
            r,
            "| {}–{} | {} | {:.3} | {:.3} | {} |",
            5 * i,
            5 * i + 5,
            b.n,
            b.tree / b.n,
            b.shrub / b.n,
            b.species.iter().map(|v| format!("{:.3}", v / b.n)).collect::<Vec<_>>().join(" | ")
        );
    }
    let _ = writeln!(r, "\n## Far tint vs placed density\n");
    let _ = writeln!(r, "Worst relative difference of mean canopy cover (far, band-limited footprint 400 cells vs placed, footprint 0) over climate bins with ≥ 200 samples and cover > 5 %: {:.1} % (limit 15 %)", 100.0 * worst_tint);
    let _ = writeln!(r, "Worst bin: {worst_bin}");
    if worst_tint > 0.15 {
        fails.push(format!("far tint differs from placed density by {:.1} %", 100.0 * worst_tint));
    }
    let _ = writeln!(r, "\n## Result\n\n{}", if fails.is_empty() { "All hard checks pass.".to_string() } else { format!("FAIL: {}", fails.join("; ")) });
    std::fs::write(out.join("niche-report.md"), &r).unwrap();
    print!("{r}");
    if !fails.is_empty() {
        std::process::exit(1);
    }
}

/// Clearing statistics in 256×256-cell (2.56 km) windows at up to 40
/// suitable forest sites, holding the site's climate fixed (the noise makes
/// the clearings).
fn clearing_patches(placement: &Placement, samples: &[Sample]) -> (f64, f64, f64) {
    const N: usize = 256;
    let cell_m = 10.0; // ~base cell on Rust
    let mut windows = 0;
    let (mut patches, mut area, mut core_cells) = (0usize, 0usize, 0usize);
    for s in samples.iter().step_by(97) {
        if windows >= 40 || s.site.height_m <= 0.0 {
            continue;
        }
        if placement.forest(s.face, s.ci, s.cj, 0.0, &s.site).canopy <= 0.6 {
            continue;
        }
        windows += 1;
        let mut open = vec![false; N * N];
        for j in 0..N {
            for i in 0..N {
                let f = placement.forest(s.face, s.ci + i as f64, s.cj + j as f64, 0.0, &s.site);
                open[j * N + i] = f.core < 0.5;
                if !open[j * N + i] {
                    core_cells += 1;
                }
            }
        }
        // Connected clearings (4-neighbour flood fill), ignoring those
        // touching the window edge.
        let mut seen = vec![false; N * N];
        for start in 0..N * N {
            if !open[start] || seen[start] {
                continue;
            }
            let mut stack = vec![start];
            seen[start] = true;
            let (mut size, mut edge) = (0usize, false);
            while let Some(p) = stack.pop() {
                size += 1;
                let (x, y) = (p % N, p / N);
                if x == 0 || y == 0 || x == N - 1 || y == N - 1 {
                    edge = true;
                }
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= N as i64 || ny >= N as i64 {
                        continue;
                    }
                    let q = ny as usize * N + nx as usize;
                    if open[q] && !seen[q] {
                        seen[q] = true;
                        stack.push(q);
                    }
                }
            }
            if !edge && size >= 4 {
                patches += 1;
                area += size;
            }
        }
    }
    let km2 = windows as f64 * (N as f64 * cell_m / 1000.0).powi(2);
    let per_km2 = patches as f64 / km2.max(1e-9);
    let mean_ha = area as f64 * cell_m * cell_m / 10_000.0 / patches.max(1) as f64;
    let cover = core_cells as f64 / (windows.max(1) * N * N) as f64;
    (per_km2, mean_ha, cover)
}

fn write_png(path: &std::path::Path, w: u32, h: u32, px: &[[u8; 3]]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = px.iter().flatten().copied().collect();
    enc.write_header().unwrap().write_image_data(&data).unwrap();
}

/// The M5 forest rules before species niches (scatter_draw.wgsl at 2fefc3a),
/// for the "recognisable look" comparison: (canopy suitability, tree density).
fn m5_tree(face: u32, ci: f64, cj: f64, s: &Site) -> (f64, f64) {
    use astrum_flora::niche::smoothstep;
    use astrum_flora::scatter::value;
    let range = |min: f64, max: f64, fall: f64, x: f64| (1.0 - (min - x).max(x - max).max(0.0) / fall).clamp(0.0, 1.0);
    let deg = std::f64::consts::PI / 180.0;
    let suit = range(-7.0, 30.0, 6.0, s.temperature_c)
        * smoothstep(0.16, 0.42, s.moisture)
        * (1.0 - smoothstep(32.0 * deg, 48.0 * deg, s.slope))
        * (1.0 - smoothstep(3200.0, 3800.0, s.height_m));
    let n = 0.45 * value(face, ci / 400.0, cj / 400.0, 40)
        + 0.3 * value(face, ci / 100.0, cj / 100.0, 41)
        + 0.15 * value(face, ci / 25.0, cj / 25.0, 42)
        + 0.1 * value(face, ci / 6.0, cj / 6.0, 43);
    let soft = 0.05;
    let threshold = 1.0 - 0.72 * suit;
    let core = smoothstep(threshold - soft, threshold + soft, n) * smoothstep(0.0, 0.08, suit);
    let fringe = smoothstep(threshold - 0.3 - soft, threshold - 0.03 + soft, n) * (1.0 - core);
    (suit, suit * (0.9 * core + 0.18 * fringe * fringe + 0.06 * fringe) + 0.025 * suit)
}

/// Camera orientation at `d` looking along the local north-ish tangent, tilted
/// down by `tilt` (camera y = up, looks along -z, as the astrum-dev poses).
fn horizon_quat(d: DVec3, tilt: f64) -> glam::DQuat {
    let up = d.normalize();
    let north = (DVec3::Z - up * up.z).normalize_or(DVec3::X);
    let fwd = (north * tilt.cos() - up * tilt.sin()).normalize();
    let z = -fwd;
    let y = (up - z * up.dot(z)).normalize();
    let x = y.cross(z);
    glam::DQuat::from_mat3(&glam::DMat3::from_cols(x, y, z))
}
