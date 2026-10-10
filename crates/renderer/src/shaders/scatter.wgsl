// Scatter placement (pipeline §12; milestone M5 Life). PROTOTYPE.
//
// WGSL mirror of crates/world/src/terrain/scatter.rs (the CPU oracle). f32 and
// u32 only. Standalone: not included by any pipeline yet. Keep constants and
// function bodies in step with the Rust file. Pure functions; the wiring
// (compute entry point, site sampling from the Tier A / atlas pages, instance
// buffer) is added when the renderer integration starts.

const SLOTS_PER_CELL: u32 = 2u;
const SPECIES_COUNT: u32 = 3u;
const PI: f32 = 3.14159265;
const DEG: f32 = 0.01745329;

fn pcg3d(v_in: vec3<u32>) -> vec3<u32> {
    var v = v_in * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

// 24-bit unit float in [0, 1).
fn unit(bits: u32) -> f32 {
    return f32(bits >> 8u) / 16777216.0;
}

fn rotl(x: u32, k: u32) -> u32 {
    return (x << k) | (x >> (32u - k));
}

struct Range {
    min_v: f32,
    max_v: f32,
    falloff: f32,
}

fn range_eval(r: Range, x: f32) -> f32 {
    let d = max(max(r.min_v - x, x - r.max_v), 0.0);
    return clamp(1.0 - d / max(r.falloff, 1e-6), 0.0, 1.0);
}

struct Species {
    footprint_m: f32,
    temperature_c: Range,
    moisture: Range,
    height_m: Range,
    max_slope: f32,
    slope_falloff: f32,
    rock_affinity: f32,
    soil_min: f32,
    water_bonus: f32,
    water_within_m: f32,
    base_density: f32,
    prior_weight: f32,
    scale_min: f32,
    scale_max: f32,
}

// PROTOTYPE: index 0 conifer, 1 broadleaf shrub, 2 boulder.
fn species_def(k: u32) -> Species {
    if k == 0u {
        return Species(3.0, Range(-8.0, 16.0, 4.0), Range(0.30, 1.0, 0.10),
            Range(0.0, 3000.0, 200.0), 38.0 * DEG, 8.0 * DEG, -1.0, 0.15,
            0.15, 40.0, 0.85, 0.5, 0.8, 1.3);
    }
    if k == 1u {
        return Species(1.2, Range(-2.0, 32.0, 6.0), Range(0.20, 1.0, 0.10),
            Range(0.0, 2400.0, 300.0), 45.0 * DEG, 10.0 * DEG, -0.5, 0.05,
            0.3, 30.0, 0.7, 0.35, 0.6, 1.4);
    }
    return Species(1.5, Range(-80.0, 80.0, 10.0), Range(0.0, 1.0, 0.10),
        Range(0.0, 9000.0, 500.0), 60.0 * DEG, 10.0 * DEG, 1.0, 0.0,
        0.0, 0.0, 0.35, 0.15, 0.4, 2.5);
}

struct SiteInputs {
    temperature_c: f32,
    moisture: f32,
    slope: f32,
    height_m: f32,
    hardness: f32,
    sediment: f32,
    water_distance_m: f32,
    rock_fraction: f32,
}

fn density(sp: Species, s: SiteInputs) -> f32 {
    if s.height_m <= 0.0 || s.water_distance_m < 2.0 {
        return 0.0;
    }
    let slope_ok = range_eval(Range(0.0, sp.max_slope, sp.slope_falloff), s.slope);
    let env = range_eval(sp.temperature_c, s.temperature_c)
        * range_eval(sp.moisture, s.moisture)
        * range_eval(sp.height_m, s.height_m)
        * slope_ok;
    let a = sp.rock_affinity;
    let rock = clamp(s.rock_fraction, 0.0, 1.0);
    var rock_factor = 1.0 + a * rock;
    if a >= 0.0 {
        rock_factor = (1.0 - a) + a * rock * (0.4 + 0.6 * clamp(s.hardness, 0.0, 1.0));
    }
    var soil = 1.0;
    if sp.soil_min > 0.0 {
        soil = clamp((s.sediment - sp.soil_min) / 0.15 + 1.0, 0.0, 1.0);
    }
    var water = 1.0;
    if s.water_distance_m <= sp.water_within_m {
        water = 1.0 + sp.water_bonus;
    }
    return clamp(sp.base_density * env * rock_factor * soil * water, 0.0, 1.0);
}

struct Candidate {
    face: u32,
    cell_i: u32,
    cell_j: u32,
    level: u32,
    slot: u32,
    u: f32,
    v: f32,
    species: u32,
    rank: f32,
    accept_roll: f32,
    yaw: f32,
    scale: f32,
}

// Fraction of candidates kept at LOD `lod` (0 = finest keeps all). A pure
// function of the candidate rank, so coarse LOD sets are subsets of finer ones.
fn lod_keep(lod: u32) -> f32 {
    return exp2(-f32(min(lod, 30u)));
}

// Candidate for one (cell, slot). `cell_size` is the width in chart units.
// Cells are indexed with u32 so every cell is owned by exactly one invocation.
// Note: f32 positions lose precision for very fine cells far from the face
// centre; the GPU path must offset from the cell origin (chart = origin +
// local) when cells get below ~1 m. Prototype keeps the direct form.
fn place_candidate(seed: u32, face: u32, cell_i: u32, cell_j: u32, level: u32,
                   slot: u32, cell_size: f32) -> Candidate {
    let base = vec3<u32>(
        cell_i ^ seed,
        cell_j ^ rotl(seed, 13u) ^ (face << 24u),
        (level * 0x9e3779b9u) ^ slot ^ (face << 8u),
    );
    let h0 = pcg3d(base);
    let h1 = pcg3d(vec3<u32>(h0.x, h0.y ^ 0x5bd1e995u, h0.z));
    var total = 0.0;
    for (var k = 0u; k < SPECIES_COUNT; k++) {
        total += species_def(k).prior_weight;
    }
    let pick = unit(h0.z) * total;
    var acc = 0.0;
    var species = SPECIES_COUNT - 1u;
    for (var k = 0u; k < SPECIES_COUNT; k++) {
        acc += species_def(k).prior_weight;
        if pick < acc {
            species = k;
            break;
        }
    }
    let sp = species_def(species);
    var c: Candidate;
    c.face = face;
    c.cell_i = cell_i;
    c.cell_j = cell_j;
    c.level = level;
    c.slot = slot;
    c.u = -1.0 + (f32(cell_i) + unit(h0.x)) * cell_size;
    c.v = -1.0 + (f32(cell_j) + unit(h0.y)) * cell_size;
    c.species = species;
    c.rank = unit(h1.x);
    c.accept_roll = unit(h1.y);
    c.yaw = unit(h1.z) * 2.0 * PI;
    c.scale = sp.scale_min + (sp.scale_max - sp.scale_min) * unit(rotl(h0.z, 11u) ^ h1.x);
    return c;
}

// Packed instance key low/high words (removed set, §12.6): the 64-bit CPU key
// is split because WGSL has no u64.
fn candidate_key(c: Candidate) -> vec2<u32> {
    let lo = (c.cell_i << 28u) | (c.cell_j << 4u) | c.slot;
    let hi = (c.level << 24u) | (c.face << 20u) | (c.cell_i >> 4u);
    return vec2<u32>(lo, hi);
}

// Forest / clearing factor from smooth value noise (per-face lattice).
fn patch_factor(seed: u32, face: u32, species: u32, u: f32, v: f32,
                metres_per_unit: f32, scale_m: f32, coverage: f32) -> f32 {
    let x = u * metres_per_unit / scale_m;
    let y = v * metres_per_unit / scale_m;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let c00 = unit(pcg3d(vec3<u32>(u32(ix) ^ seed, u32(iy) ^ (face << 24u), 0x7a7c0000u ^ species)).x);
    let c10 = unit(pcg3d(vec3<u32>(u32(ix + 1) ^ seed, u32(iy) ^ (face << 24u), 0x7a7c0000u ^ species)).x);
    let c01 = unit(pcg3d(vec3<u32>(u32(ix) ^ seed, u32(iy + 1) ^ (face << 24u), 0x7a7c0000u ^ species)).x);
    let c11 = unit(pcg3d(vec3<u32>(u32(ix + 1) ^ seed, u32(iy + 1) ^ (face << 24u), 0x7a7c0000u ^ species)).x);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = c00 + (c10 - c00) * sx;
    let b = c01 + (c11 - c01) * sx;
    let n = a + (b - a) * sy;
    let t = clamp((n - (1.0 - coverage)) / 0.1 + 0.5, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Accept when the roll is below density * patch_f.
fn accepted(c: Candidate, site: SiteInputs, patch_f: f32) -> bool {
    let d = density(species_def(c.species), site) * clamp(patch_f, 0.0, 1.0);
    return c.accept_roll < d;
}

// Spacing rule between two instances; same species pack at 0.8 of the sum.
fn spacing_ok(a: Candidate, b: Candidate, metres_per_unit: f32) -> bool {
    let fa = species_def(a.species).footprint_m * a.scale;
    let fb = species_def(b.species).footprint_m * b.scale;
    var need = fa + fb;
    if a.species == b.species {
        need *= 0.8;
    }
    let d = length(vec2<f32>(a.u - b.u, a.v - b.v)) * metres_per_unit;
    return d >= need;
}

// Competition priority: bigger footprint wins, then the lower key.
fn wins(a: Candidate, b: Candidate) -> bool {
    let fa = species_def(a.species).footprint_m * a.scale;
    let fb = species_def(b.species).footprint_m * b.scale;
    if fa != fb {
        return fa > fb;
    }
    let ka = candidate_key(a);
    let kb = candidate_key(b);
    return ka.y < kb.y || (ka.y == kb.y && ka.x < kb.x);
}

// LOD thinning (run after competition): keep when rank < keep fraction.
fn kept_at_lod(c: Candidate, lod: u32) -> bool {
    return c.rank < lod_keep(lod);
}
