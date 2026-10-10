// PROTOTYPE (flora lane): forest placement field from species niches, pure
// functions only (no bindings), so the GPU-vs-CPU test can run them alone
// (crates/renderer/tests/flora_niche_gpu.rs). CPU mirror:
// astrum_flora::scatter (keep both in step). Needs the generated niche table
// (scatter_species.wgsl: FL_SPECIES, FL_*_MASK, fl_niche) before it.
//
// Each species has a suitability s_k from its niche (temperature, moisture,
// height with its treeline, slope, sediment as soil depth). The canopy
// suitability is the best canopy species; four octaves of world-anchored
// value noise against a threshold that falls with suitability make forest
// cores, fringes and clearings. Within a layer the species is drawn with
// weights s_k² · prior_k.

const FL_DEG: f32 = 0.017453292;

fn sc_pcg3d(v_in: vec3<u32>) -> vec3<u32> {
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

fn sc_unit(bits: u32) -> f32 {
    return f32(bits >> 8u) / 16777216.0;
}

fn sc_range(min_v: f32, max_v: f32, falloff: f32, x: f32) -> f32 {
    let d = max(max(min_v - x, x - max_v), 0.0);
    return clamp(1.0 - d / max(falloff, 1.0e-6), 0.0, 1.0);
}

// Smooth value noise in 0..1 over base-cell coordinates (x, y) of a face.
fn sc_value(face: u32, x: f32, y: f32, salt: u32) -> f32 {
    let i = floor(x);
    let j = floor(y);
    let f = vec2<f32>(x - i, y - j);
    let w = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let c = vec2<u32>(u32(i), u32(j));
    let k = face * 64u + salt;
    let a = sc_unit(sc_pcg3d(vec3<u32>(c.x, c.y, k)).x);
    let b = sc_unit(sc_pcg3d(vec3<u32>(c.x + 1u, c.y, k)).x);
    let d = sc_unit(sc_pcg3d(vec3<u32>(c.x, c.y + 1u, k)).x);
    let e = sc_unit(sc_pcg3d(vec3<u32>(c.x + 1u, c.y + 1u, k)).x);
    return mix(mix(a, b, w.x), mix(d, e, w.x), w.y);
}

// Face id of a chart normal: dominant axis and sign (consistent per face).
fn sc_face(n: vec3<f32>) -> u32 {
    let a = abs(n);
    if a.x >= a.y && a.x >= a.z {
        return select(1u, 0u, n.x >= 0.0);
    }
    if a.y >= a.z {
        return select(3u, 2u, n.y >= 0.0);
    }
    return select(5u, 4u, n.z >= 0.0);
}


// Facts at a placement site: temperature °C, moisture 0..1, height above sea
// level (m), slope (rad), Tier A sediment 0..1.
struct FlSite {
    t: f32,
    m: f32,
    h: f32,
    slope: f32,
    sediment: f32,
}

// Suitability 0..1 of species `k` at `s` (Niche::suitability).
fn fl_suit(k: u32, s: FlSite) -> f32 {
    let n = fl_niche(k);
    let env = sc_range(n.t.x, n.t.y, n.t.z, s.t) * sc_range(n.m.x, n.m.y, n.m.z, s.m)
        * sc_range(n.h.x, n.h.y, n.h.z, s.h);
    let steep = smoothstep(n.slope.x - n.slope.y, n.slope.x + n.slope.y, s.slope);
    // Bare rock keeps 60 % vigour (SOIL_FLOOR in niche.rs).
    let soil = clamp(0.6 + 0.4 * (s.sediment + 1.0e-3) / (n.soil + 1.0e-3), 0.0, 1.0);
    // Nothing grows at or below sea level (shore fade over 15 m).
    let shore = smoothstep(0.0, 15.0, s.h);
    return env * (1.0 - steep) * soil * shore;
}

fn fl_weight(k: u32, s: FlSite) -> f32 {
    let su = fl_suit(k, s);
    return su * su * fl_niche(k).prior;
}

// Expected crown areas (m², top and side) of one plant of `mask`: species
// weighted like the pick (Placement::layer_crown).
fn fl_layer_crown(mask: u32, s: FlSite) -> vec2<f32> {
    var a = vec2<f32>(0.0);
    var wsum = 0.0;
    for (var k = 0u; k < FL_SPECIES; k = k + 1u) {
        if ((mask >> k) & 1u) != 0u {
            let w = fl_weight(k, s);
            a += w * fl_niche(k).crown;
            wsum += w;
        }
    }
    return select(vec2<f32>(0.0), a / max(wsum, 1.0e-6), wsum > 1.0e-6);
}

// Best suitability among the species of `mask`.
fn fl_layer_suit(mask: u32, s: FlSite) -> f32 {
    var best = 0.0;
    for (var k = 0u; k < FL_SPECIES; k = k + 1u) {
        if ((mask >> k) & 1u) != 0u {
            best = max(best, fl_suit(k, s));
        }
    }
    return best;
}

// Species of `mask` drawn by `roll` (0..1) with weights s² · prior:
// x = index (-1 if none fits), y = its suitability.
fn fl_pick(mask: u32, s: FlSite, roll: f32) -> vec2<f32> {
    var total = 0.0;
    for (var k = 0u; k < FL_SPECIES; k = k + 1u) {
        if ((mask >> k) & 1u) != 0u {
            total += fl_weight(k, s);
        }
    }
    if total <= 0.0 {
        return vec2<f32>(-1.0, 0.0);
    }
    let target_w = roll * total;
    var acc = 0.0;
    var last = -1.0;
    for (var k = 0u; k < FL_SPECIES; k = k + 1u) {
        if ((mask >> k) & 1u) != 0u {
            let w = fl_weight(k, s);
            acc += w;
            if target_w < acc {
                return vec2<f32>(f32(k), fl_suit(k, s));
            }
            if w > 0.0 {
                last = f32(k);
            }
        }
    }
    return vec2<f32>(last, fl_suit(u32(max(last, 0.0)), s));
}

// Plant probabilities of one base cell (they sum to at most 1).
struct ForestSite {
    tree: f32,
    shrub: f32,
    boulder: f32,
    // 1 in a forest's core, 0 outside.
    core: f32,
    // Best canopy / shrub suitability.
    canopy: f32,
    shrubland: f32,
    // Canopy size factor (niche edge and forest core).
    stature: f32,
    // Expected share of the view the crowns hide for the `view` given to
    // sc_forest (Boolean model): the far tint strength.
    cover: f32,
    // Canopy colour for the far tint (linear).
    color: vec3<f32>,
}

// Forest field at base-cell coordinates (ci, cj) of `face`. `footprint` is
// the sample spacing in base cells: octaves finer than about four samples
// fade to their mean (band limit for the far tint); 0 for single plants.
// `view` = (gain, cosine of the view zenith angle): `cover` is the expected
// share of the view hidden when gain × crown area per cell is the mean crown
// count over a point (Placement::forest_view).
fn sc_forest(face: u32, ci: f32, cj: f32, footprint: f32, s: FlSite, view: vec2<f32>) -> ForestSite {
    var site: ForestSite;
    // Single plants (gain 0) skip the crown weights.
    var crown_c = vec2<f32>(0.0);
    var crown_s = vec2<f32>(0.0);
    if view.x > 0.0 {
        crown_c = fl_layer_crown(FL_CANOPY_MASK, s);
        crown_s = fl_layer_crown(FL_SHRUB_MASK, s);
    }
    let cos_v = clamp(view.y, 0.05, 1.0);
    let suit = fl_layer_suit(FL_CANOPY_MASK, s);
    let f1 = smoothstep(100.0, 200.0, footprint);
    let f2 = smoothstep(25.0, 50.0, footprint);
    let f3 = smoothstep(6.0, 12.0, footprint);
    let f4 = smoothstep(1.5, 3.0, footprint);
    let o1 = mix(sc_value(face, ci / 400.0, cj / 400.0, 40u), 0.5, f1);
    let o2 = mix(sc_value(face, ci / 100.0, cj / 100.0, 41u), 0.5, f2);
    let o3 = mix(sc_value(face, ci / 25.0, cj / 25.0, 42u), 0.5, f3);
    let o4 = mix(sc_value(face, ci / 6.0, cj / 6.0, 43u), 0.5, f4);
    let n = 0.45 * o1 + 0.3 * o2 + 0.15 * o3 + 0.1 * o4;
    // Octaves faded to their mean leave the noise's spread out of `n`;
    // average over it instead (16 equal-mass nodes at the measured quantiles
    // of the noise sum, NOISE_QUANTILES in scatter.rs), so the far tint is the
    // expected cover of the plants it stands for. Per-octave sd 0.215.
    let sigma = 0.215 * length(vec4<f32>(0.45 * f1, 0.3 * f2, 0.15 * f3, 0.1 * f4));
    let shrubby = fl_layer_suit(FL_SHRUB_MASK, s);
    let rock = smoothstep(25.0 * FL_DEG, 45.0 * FL_DEG, s.slope);
    let boulder = 0.02 + 0.25 * rock * (1.0 - smoothstep(60.0 * FL_DEG, 70.0 * FL_DEG, s.slope));
    // Good climates are mostly forest with clearings; marginal ones patchy.
    let threshold = 1.0 - 0.72 * suit;
    let soft = 0.05;
    site.canopy = suit;
    site.shrubland = shrubby;
    let z0 = vec4<f32>(-1.8306, -1.3436, -1.0457, -0.8138);
    let z1 = vec4<f32>(-0.6127, -0.4298, -0.2542, -0.0872);
    let z2 = vec4<f32>(0.0801, 0.2506, 0.4245, 0.6083);
    let z3 = vec4<f32>(0.8109, 1.0471, 1.3482, 1.8451);
    let nodes = select(1u, 16u, sigma > 0.0);
    let w = 1.0 / f32(nodes);
    for (var q = 0u; q < nodes; q = q + 1u) {
        let zv = select(select(select(z0, z1, q >= 4u), z2, q >= 8u), z3, q >= 12u);
        let nn = n + sigma * select(0.0, zv[q % 4u], sigma > 0.0);
        let core = smoothstep(threshold - soft, threshold + soft, nn) * smoothstep(0.0, 0.08, suit);
        let fringe = smoothstep(threshold - 0.3 - soft, threshold - 0.03 + soft, nn) * (1.0 - core);
        // Dense in the core, thinning through the fringe, lone trees elsewhere.
        var tree = suit * (0.9 * core + 0.18 * fringe * fringe + 0.06 * fringe) + 0.025 * suit;
        var shrub = shrubby * (0.35 * fringe + 0.06 * core + 0.04);
        var bould = boulder;
        let total = tree + shrub + bould;
        if total > 1.0 {
            tree /= total;
            shrub /= total;
            bould /= total;
        }
        let stature = mix(0.45, 1.0, suit) * mix(0.6, 1.0, core);
        site.tree += w * tree;
        site.shrub += w * shrub;
        site.boulder += w * bould;
        site.core += w * core;
        site.stature += w * stature;
        // Crown area per cell seen at the view angle (ellipsoid-like crowns).
        let top = tree * stature * stature * crown_c.x + shrub * crown_s.x;
        let side = tree * stature * stature * crown_c.y + shrub * crown_s.y;
        let seen = sqrt(top * top * cos_v * cos_v + side * side * (1.0 - cos_v * cos_v)) / cos_v;
        site.cover += w * (1.0 - exp(-view.x * seen));
    }
    var c = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var k = 0u; k < FL_SPECIES; k = k + 1u) {
        if ((FL_CANOPY_MASK >> k) & 1u) != 0u {
            let w = fl_weight(k, s);
            c += w * fl_niche(k).color;
            wsum += w;
        }
    }
    // Too little canopy to weigh: the default tint (f32 cannot divide tiny sums).
    site.color = select(vec3<f32>(0.04, 0.07, 0.025), c / max(wsum, 1.0e-6), wsum > 1.0e-6);
    return site;
}

// Rock archetype for bedrock `hardness` (Tier A, 0..1) drawn by `roll` with
// weights from the planet's hardness envelopes; -1 when none fits
// (astrum_flora::scatter::pick_rock).
fn fl_pick_rock(hardness: f32, roll: f32) -> i32 {
    var total = 0.0;
    for (var r = 0u; r < FL_ROCKS; r = r + 1u) {
        let e = fl_rock(r);
        total += sc_range(e.x, e.y, e.z, hardness);
    }
    if total <= 0.0 {
        return -1;
    }
    var acc = 0.0;
    var last = -1;
    for (var r = 0u; r < FL_ROCKS; r = r + 1u) {
        let e = fl_rock(r);
        let w = sc_range(e.x, e.y, e.z, hardness);
        acc += w;
        if roll * total < acc {
            return i32(r);
        }
        if w > 0.0 {
            last = i32(r);
        }
    }
    return last;
}
