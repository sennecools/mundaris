//! Pop scorer for the forest LOD chain: reads the plant dumps written with
//! `ASTRUM_FLORA_TRACE=<dir>` during a scripted descent and counts, between
//! consecutive dumps, plants that appear, vanish, move or switch tier
//! without a fade (outside the dither bands), inside the inner view cone.
//!
//! cargo run -p astrum_flora --release --example flora_pop_scorer -- <dir> [out.md]
//!
//! Camera motion between dumps is removed with a rigid fit (Kabsch) over the
//! plants present in both dumps; a plant "moves" when its residual exceeds
//! 5 cm + 0.1 % of its distance. Layout constants mirror
//! crates/renderer/src/flora_draw.rs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use glam::{DMat3, DVec3};

const ARGS_BYTES: usize = 4096;
const FAR_CAPACITY: usize = 32_768;
const HEADER: usize = 32;
const SORTED_START: usize = 147_456;
const ARGS_WORD: usize = 16;
const BUCKETS: usize = 84;
const TIERS: usize = 3;
const PLANT: usize = 64;
/// Half-angle tangents of the inner cone (60° vertical FOV, 16:10, 80 %).
const TAN_V: f64 = 0.8 * 0.577;
const TAN_H: f64 = 0.8 * 0.577 * 1.26;
const MAX_D: f64 = 14_000.0;

#[derive(Clone, Debug)]
struct Rec {
    pos: DVec3,
    tiers: BTreeSet<usize>,
    transition: bool,
}

fn load(path: &std::path::Path) -> BTreeMap<u64, Rec> {
    let data = std::fs::read(path).unwrap();
    let word = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    let f = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as f64;
    let u = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    let plant_off = |abs_index: usize| {
        if abs_index < SORTED_START {
            ARGS_BYTES + (abs_index - FAR_CAPACITY) * PLANT
        } else {
            ARGS_BYTES + HEADER * PLANT + (abs_index - SORTED_START) * PLANT
        }
    };
    let mut out: BTreeMap<u64, Rec> = BTreeMap::new();
    for b in 0..BUCKETS {
        let count = word(ARGS_WORD + b * 5 + 1) as usize;
        if count == 0 {
            continue;
        }
        let h = plant_off(FAR_CAPACITY + b / 4) + 48 + (b % 4) * 4;
        let base = u(h) as usize;
        for k in 0..count {
            let o = plant_off(base + k);
            if o + PLANT > data.len() {
                break;
            }
            let pos = DVec3::new(f(o), f(o + 4), f(o + 8));
            let fade = f(o + 28);
            let split = f(o + 44);
            let id = ((u(o + 52) as u64) << 32) | u(o + 60) as u64;
            let r = out.entry(id).or_insert(Rec { pos, tiers: BTreeSet::new(), transition: false });
            r.tiers.insert(b % TIERS);
            r.transition |= fade < 0.999 || split < 0.999;
        }
    }
    out
}

fn inner(p: DVec3) -> bool {
    let z = -p.z;
    z > 1.0 && p.length() < MAX_D && p.x.abs() < TAN_H * z && p.y.abs() < TAN_V * z
}

/// Rigid fit B ≈ R·A + t (Kabsch via power-iterated polar decomposition).
fn fit(pairs: &[(DVec3, DVec3)]) -> (DMat3, DVec3) {
    let n = pairs.len() as f64;
    let ca = pairs.iter().map(|p| p.0).sum::<DVec3>() / n;
    let cb = pairs.iter().map(|p| p.1).sum::<DVec3>() / n;
    let mut h = DMat3::ZERO;
    for (a, b) in pairs {
        let (a, b) = (*a - ca, *b - cb);
        h += DMat3::from_cols(b * a.x, b * a.y, b * a.z);
    }
    // Polar decomposition: R = H (HᵀH)^-1/2 by Newton iteration.
    let mut r = h * (1.0 / h.to_cols_array().iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12));
    for _ in 0..30 {
        let inv_t = r.inverse().transpose();
        r = (r + inv_t) * 0.5;
    }
    (r, cb - r * ca)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let out = args.get(2).cloned();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    files.sort();
    let mut md = String::from("| dumps | plants | matched | appeared | vanished | moved | tier pops | worst move m |\n|---|---|---|---|---|---|---|---|\n");
    let (mut t_app, mut t_van, mut t_mov, mut t_tier, mut t_match) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut new_any, mut new_faded) = (0usize, 0usize);
    let mut prev: Option<(String, BTreeMap<u64, Rec>)> = None;
    for path in &files {
        let cur = load(path);
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        if let Some((pname, a)) = &prev {
            let pairs: Vec<(DVec3, DVec3)> = a.iter().filter_map(|(id, ra)| cur.get(id).map(|rb| (ra.pos, rb.pos))).collect();
            if pairs.len() < 10 {
                prev = Some((name, cur));
                continue;
            }
            let (r, t) = fit(&pairs);
            let (mut app, mut van, mut mov, mut tier) = (0, 0, 0, 0);
            let mut worst = 0.0f64;
            for (id, ra) in a {
                let pred = r * ra.pos + t;
                match cur.get(id) {
                    Some(rb) => {
                        if !inner(rb.pos) {
                            continue;
                        }
                        let res = (rb.pos - pred).length();
                        if res > 0.05 + 1e-3 * rb.pos.length() {
                            mov += 1;
                            worst = worst.max(res);
                        }
                        if ra.tiers != rb.tiers && !ra.transition && !rb.transition {
                            tier += 1;
                        }
                    }
                    None => {
                        if inner(pred) && !ra.transition {
                            van += 1;
                        }
                    }
                }
            }
            let (ri, ti) = (r.inverse(), -(r.inverse() * t));
            for (id, rb) in &cur {
                if !a.contains_key(id) {
                    new_any += 1;
                    if inner(rb.pos) && inner(ri * rb.pos + ti) {
                        if rb.transition {
                            new_faded += 1;
                        } else {
                            app += 1;
                        }
                    }
                }
            }
            let _ = writeln!(md, "| {pname}→{name} | {} | {} | {app} | {van} | {mov} | {tier} | {worst:.2} |", cur.len(), pairs.len());
            t_app += app;
            t_van += van;
            t_mov += mov;
            t_tier += tier;
            t_match += pairs.len();
        }
        prev = Some((name, cur));
    }
    let _ = writeln!(
        md,
        "\nTotals over {} dumps: appeared {t_app}, vanished {t_van}, moved {t_mov}, tier pops {t_tier} (matched plant-pairs {t_match}). Pops = changes outside fade bands inside the inner view cone. New plants overall {new_any}, of which {new_faded} entered inside the inner cone while fading in.",
        files.len(),
    );
    print!("{md}");
    if let Some(o) = out {
        std::fs::write(o, &md).unwrap();
    }
}
