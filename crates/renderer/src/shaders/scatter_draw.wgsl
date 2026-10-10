// PROTOTYPE (M5 Life): procedural forests (conifers, broadleaf trees), shrubs
// and boulders drawn on the selected world-map terrain nodes (pipeline §12),
// appended to the terrain draw shader (same bindings and helpers).
//
// Placement. Base cells are a fixed 2^SCATTER_CELL_BITS grid per cube face
// (~10 m on Rust), so plants are anchored to the world, not to nodes. One
// candidate per base cell, jittered.
//
// LOD without seams or pops. A base cell's level λ is how many consecutive
// ancestors (2×2 quadrant hierarchy, hashed choices) pick it; P(λ ≥ k) = 4^-k.
// Its rank r lies in [4^-(λ+1), 4^-λ), so P(r < x) ≈ x and every candidate
// with r < 4^-k has λ ≥ k. A drawn node SCATTER_SIDE² slots wide in coarse
// cells enumerates exactly the candidates with λ ≥ k (descending by the same
// hashed choices), and a candidate is shown while r < keep(d), with
// keep(d) = (SCATTER_FULL_M / d)² continuous in view distance. CDLOD draws a
// node of size S only beyond ~1.5 S, where keep ≤ 4^-k, so every node holds
// all the plants its distance asks for: density is continuous across node
// levels and refinement adds nothing that pops. Plants grow in as keep(d)
// passes their rank. Beyond the drawn plants the terrain fragment shader
// lays the canopy colour (forest_cover) with a strength that rises as keep
// falls, so forests read as forests from orbit.
//
// Forest field. Climate suitability sets how much of the land is forest; four
// octaves of value noise (~4 km to ~60 m) against a suitability threshold make
// large contiguous forests in good climates and patches in marginal ones. A
// fringe outside each forest carries shrubs, small trees and lone trees;
// trees shrink towards the forest edge and the cold tree line. Conifers take
// over from broadleaf trees with cold.

const SCATTER_SIDE: u32 = 32u;
const SCATTER_SLOTS: u32 = 1024u;
const SCATTER_VERTS: u32 = 96u;
const SCATTER_CELL_BITS: u32 = 16u;
// Full density within this view distance (m); keep(d) = (FULL / d)².
const SCATTER_FULL_M: f32 = 350.0;
// No plants beyond this view distance (m), faded over the last 30 %.
const SCATTER_MAX_DISTANCE_M: f32 = 16000.0;
const SCATTER_DEG: f32 = 0.017453292;
const SCATTER_MAX_LEVEL: u32 = 12u;

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

// Plant probabilities of one base cell (they sum to at most 1).
struct ForestSite {
    tree: f32,
    shrub: f32,
    boulder: f32,
    // 1 in a forest's core, 0 outside: trees shrink towards the edge.
    core: f32,
    // Share of conifers among trees.
    conifer: f32,
    // Tree-line stunting, 1 = full size.
    stature: f32,
}

// Forest field at base-cell coordinates (ci, cj) of `face`. `footprint` is
// the sample spacing in base cells: octaves finer than about four samples
// fade to their mean (band limit for the far tint); 0 for single plants.
fn sc_forest(face: u32, ci: f32, cj: f32, footprint: f32, t: f32, m: f32, slope: f32, h: f32) -> ForestSite {
    var site: ForestSite;
    let warm = sc_range(-7.0, 30.0, 6.0, t);
    let wet = smoothstep(0.16, 0.42, m);
    let steep = smoothstep(32.0 * SCATTER_DEG, 48.0 * SCATTER_DEG, slope);
    let suit = warm * wet * (1.0 - steep) * (1.0 - smoothstep(3200.0, 3800.0, h));
    let f1 = smoothstep(100.0, 200.0, footprint);
    let f2 = smoothstep(25.0, 50.0, footprint);
    let f3 = smoothstep(6.0, 12.0, footprint);
    let f4 = smoothstep(1.5, 3.0, footprint);
    let o1 = mix(sc_value(face, ci / 400.0, cj / 400.0, 40u), 0.5, f1);
    let o2 = mix(sc_value(face, ci / 100.0, cj / 100.0, 41u), 0.5, f2);
    let o3 = mix(sc_value(face, ci / 25.0, cj / 25.0, 42u), 0.5, f3);
    let o4 = mix(sc_value(face, ci / 6.0, cj / 6.0, 43u), 0.5, f4);
    let n = 0.45 * o1 + 0.3 * o2 + 0.15 * o3 + 0.1 * o4;
    // Octaves faded to their mean no longer cross the threshold locally, so
    // widen the transitions by the faded amplitude: far away the result is
    // the expected coverage instead of a hard contour of a coarse field.
    let faded = 0.45 * f1 + 0.3 * f2 + 0.15 * f3 + 0.1 * f4;
    let soft = 0.05 + 0.3 * faded;
    // Good climates are mostly forest with clearings; marginal ones patchy.
    let threshold = 1.0 - 0.72 * suit;
    let core = smoothstep(threshold - soft, threshold + soft, n) * smoothstep(0.0, 0.08, suit);
    let fringe = smoothstep(threshold - 0.3 - soft, threshold - 0.03 + soft, n) * (1.0 - core);
    site.core = core;
    // Dense in the core, thinning through the fringe, lone trees elsewhere.
    site.tree = suit * (0.9 * core + 0.18 * fringe * fringe + 0.06 * fringe) + 0.025 * suit;
    let shrubby = sc_range(-3.0, 32.0, 6.0, t) * smoothstep(0.06, 0.3, m) * (1.0 - steep);
    site.shrub = shrubby * (0.35 * fringe + 0.06 * core + 0.04);
    let rock = smoothstep(25.0 * SCATTER_DEG, 45.0 * SCATTER_DEG, slope);
    site.boulder = 0.02 + 0.25 * rock * (1.0 - smoothstep(60.0 * SCATTER_DEG, 70.0 * SCATTER_DEG, slope));
    let total = site.tree + site.shrub + site.boulder;
    if total > 1.0 {
        site.tree /= total;
        site.shrub /= total;
        site.boulder /= total;
    }
    site.conifer = clamp(smoothstep(15.0, 5.0, t) + 0.3 * smoothstep(1800.0, 2800.0, h), 0.0, 1.0);
    site.stature = mix(0.4, 1.0, smoothstep(-7.0, 1.0, t)) * mix(0.6, 1.0, core);
    return site;
}

// keep(d): share of candidates shown at view distance `d` (m).
fn sc_keep(d: f32) -> f32 {
    let ratio = SCATTER_FULL_M / max(d, 1.0);
    return min(1.0, ratio * ratio)
        * (1.0 - smoothstep(0.7 * SCATTER_MAX_DISTANCE_M, SCATTER_MAX_DISTANCE_M, d));
}

// Shapes (32 triangles, 96 vertices): kind 0 conifer, 1 broadleaf, 2 shrub,
// 3 boulder. Triangles 0..23 form three "lobes" of 8 triangles; 24..31 the
// trunk (4 quads; degenerate for shrubs and boulders).
// - Conifer lobes are stacked open cones (8 sides), widest at the bottom.
// - Other lobes are 4-sided bipyramids (4 up, 4 down) around a lobe centre,
//   offset and sized per plant from `seed`, with per-vertex jitter; several
//   overlapping lobes read as a rounded, irregular crown, shrub or rock pile.
// Positions are metres before scaling, z up; trunks start below ground.
struct Lobe {
    centre: vec3<f32>,
    radius: f32,
    up: f32,
    down: f32,
}

fn sc_rand(seed: u32, a: u32, b: u32) -> f32 {
    return sc_unit(sc_pcg3d(vec3<u32>(seed, a, b)).x);
}

fn sc_lobe(kind: u32, lobe: u32, seed: u32) -> Lobe {
    var l: Lobe;
    let angle = (f32(lobe) * 2.1 + sc_rand(seed, lobe, 1u) * 1.2) ;
    let r1 = sc_rand(seed, lobe, 2u);
    if kind == 0u {
        // Conifer tiers: ring height, ring radius, apex above the ring.
        let tier = f32(lobe);
        l.centre = vec3<f32>(0.0, 0.0, 2.0 + 3.4 * tier);
        l.radius = (3.3 - 0.85 * tier) * (0.9 + 0.2 * r1);
        l.up = 5.2 - 0.6 * tier;
        l.down = 0.0;
    } else if kind == 1u {
        // Broadleaf: one main crown and two side lobes.
        if lobe == 0u {
            l.centre = vec3<f32>(0.0, 0.0, 8.0);
            l.radius = 4.0;
            l.up = 3.4;
            l.down = 2.6;
        } else {
            let off = 2.0 + 0.8 * r1;
            l.centre = vec3<f32>(cos(angle) * off, sin(angle) * off, 7.0 + 2.0 * sc_rand(seed, lobe, 3u));
            l.radius = 2.8 + 0.6 * r1;
            l.up = 2.4;
            l.down = 1.8;
        }
    } else if kind == 2u {
        // Shrub: a clump of three low lobes.
        let off = select(0.9 + 0.5 * r1, 0.0, lobe == 0u);
        l.centre = vec3<f32>(cos(angle) * off, sin(angle) * off, 0.9 + 0.3 * r1);
        l.radius = select(1.0 + 0.3 * r1, 1.5, lobe == 0u);
        l.up = select(0.8 + 0.3 * r1, 1.1, lobe == 0u);
        l.down = 1.0;
    } else {
        // Boulder: one main block and two smaller stones beside it.
        let off = select(1.3 + 0.6 * r1, 0.0, lobe == 0u);
        l.centre = vec3<f32>(cos(angle) * off, sin(angle) * off, select(0.15, 0.35, lobe == 0u));
        l.radius = select(0.45 + 0.35 * r1, 1.25, lobe == 0u);
        l.up = select(0.35 + 0.3 * r1, 0.85, lobe == 0u);
        l.down = l.up * 0.6;
    }
    return l;
}

// Local corner `corner` of triangle `tri` of a plant of `kind`.
fn sc_corner(kind: u32, tri: u32, corner: u32, seed: u32) -> vec3<f32> {
    if tri < 24u {
        let lobe = tri / 8u;
        let i = tri % 8u;
        let l = sc_lobe(kind, lobe, seed);
        if kind == 0u {
            // Open cone: apex, then two ring vertices (8 sides).
            if corner == 0u {
                return l.centre + vec3<f32>(0.0, 0.0, l.up);
            }
            let j = (i + corner - 1u) % 8u;
            let a = f32(j) * (6.2831853 / 8.0) + f32(lobe) * 0.39;
            let r = l.radius * (0.88 + 0.24 * sc_rand(seed, lobe * 8u + j, 5u));
            return l.centre + vec3<f32>(cos(a) * r, sin(a) * r, 0.0);
        }
        // Bipyramid: triangles 0..3 to the top apex, 4..7 to the bottom one.
        let upper = i < 4u;
        let k = i % 4u;
        if corner == 0u {
            return l.centre + vec3<f32>(0.0, 0.0, select(-l.down, l.up, upper));
        }
        let j = (k + corner - 1u) % 4u;
        let a = (f32(j) + 0.5) * (6.2831853 / 4.0) + sc_rand(seed, lobe, 4u) * 1.57;
        let r = l.radius * (0.8 + 0.4 * sc_rand(seed, lobe * 4u + j, 6u));
        let dz = (sc_rand(seed, lobe * 4u + j, 7u) - 0.5) * 0.4 * l.radius;
        return l.centre + vec3<f32>(cos(a) * r, sin(a) * r, dz);
    }
    // Trunk: 4 quads, two triangles each.
    var trunk_r = 0.32;
    var trunk_top = 3.0;
    if kind == 1u {
        trunk_r = 0.38;
        trunk_top = 6.0;
    } else if kind >= 2u {
        trunk_r = 0.0;
        trunk_top = 0.0;
    }
    let t = tri - 24u;
    let q = t / 2u;
    let a0 = f32(q) * (6.2831853 / 4.0);
    let a1 = f32(q + 1u) * (6.2831853 / 4.0);
    let p00 = vec3<f32>(cos(a0) * trunk_r, sin(a0) * trunk_r, -0.6);
    let p10 = vec3<f32>(cos(a1) * trunk_r, sin(a1) * trunk_r, -0.6);
    let p01 = vec3<f32>(cos(a0) * trunk_r * 0.7, sin(a0) * trunk_r * 0.7, trunk_top);
    let p11 = vec3<f32>(cos(a1) * trunk_r * 0.7, sin(a1) * trunk_r * 0.7, trunk_top);
    if t % 2u == 0u {
        return select(select(p01, p10, corner == 1u), p00, corner == 0u);
    }
    return select(select(p01, p11, corner == 1u), p10, corner == 0u);
}

// Point the face normal points away from (lobe centre or trunk axis).
fn sc_face_origin(kind: u32, tri: u32, seed: u32, centre: vec3<f32>) -> vec3<f32> {
    if tri < 24u {
        return sc_lobe(kind, tri / 8u, seed).centre;
    }
    return vec3<f32>(0.0, 0.0, centre.z);
}
// Chart coordinates of node position `st` in base cells.
fn sc_cells(inst: Instance, st: vec2<f32>) -> vec2<f32> {
    let width = inst.face_u.w;
    let q0 = inst.n0.w;
    let cells_per_unit = f32(1u << (SCATTER_CELL_BITS - 1u));
    let u = dot(inst.n0.xyz, inst.face_u.xyz) * q0 + (st.x - 0.5) * width;
    let v = dot(inst.n0.xyz, inst.face_v.xyz) * q0 + (st.y - 0.5) * width;
    return vec2<f32>((u + 1.0) * cells_per_unit, (v + 1.0) * cells_per_unit);
}

// Canopy colour and strength (0..1) for the terrain fragment at node
// position `st` and view distance `d`: the forest field band-limited to the
// drawn grid, near the camera only the darker forest floor (the drawn trees
// carry the canopy), far away the full canopy.
// `footprint`: base cells per pixel (screen-space derivative of `c`, taken
// by the caller in uniform control flow). A per-node footprint would differ
// between neighbouring nodes of different levels and turn the octave fades
// into node-shaped blocks.
fn forest_cover(inst: Instance, st: vec2<f32>, normal_body: vec3<f32>, climate: vec2<f32>, ground: f32, d: f32, footprint_px: f32) -> vec4<f32> {
    let c = sc_cells(inst, st);
    // Fade each octave once it spans fewer than about eight pixels.
    let footprint = 2.0 * footprint_px;
    let up_body = normalize(inst.n0.xyz + chart_diff(inst, st));
    let slope = acos(clamp(dot(normal_body, up_body), -1.0, 1.0));
    let site = sc_forest(sc_face(inst.n0.xyz), c.x, c.y, footprint, climate.x, climate.y, slope, ground);
    var colour = mix(vec3<f32>(0.04, 0.07, 0.025), vec3<f32>(0.02, 0.04, 0.025), site.conifer);
    // Canopy texture: crowns and gaps (~30 m) and clumps (~90 m), each fading
    // to its mean once the drawn grid is too coarse to carry it.
    let face = sc_face(inst.n0.xyz);
    let crowns = mix(sc_value(face, c.x / 3.0, c.y / 3.0, 44u), 0.5, smoothstep(0.75, 1.5, footprint));
    let clumps = mix(sc_value(face, c.x / 9.0, c.y / 9.0, 45u), 0.5, smoothstep(2.25, 4.5, footprint));
    colour *= 0.55 + 0.6 * crowns + 0.35 * (clumps - 0.5);
    let cover = clamp(site.tree * site.stature + 0.4 * site.shrub, 0.0, 1.0);
    let far = 1.0 - sc_keep(d);
    return vec4<f32>(colour, cover * mix(0.7, 0.9, far));
}

// PROTOTYPE (M4/M5): mean-preserving ground detail near the camera: four
// octaves of world-anchored value noise (about 0.5, 2, 8 and 32 m), each
// fading out once its wavelength spans fewer than about four pixels, so the
// far colour is unchanged. Returns a brightness factor around 1 and a 0..1
// patch value for a slight green/dry hue shift.
fn ground_detail(inst: Instance, st: vec2<f32>, footprint_px: f32) -> vec2<f32> {
    let c = sc_cells(inst, st);
    let face = sc_face(inst.n0.xyz);
    var sum = 0.0;
    var hue = 0.0;
    let scales = array<f32, 4>(16.0, 4.0, 1.0, 0.25);
    let amps = array<f32, 4>(0.16, 0.14, 0.12, 0.10);
    for (var i = 0u; i < 4u; i = i + 1u) {
        let wl = 1.0 / scales[i];
        let keep = 1.0 - smoothstep(0.25 * wl, 0.5 * wl, footprint_px);
        let v = sc_value(face, c.x * scales[i], c.y * scales[i], 50u + i);
        sum += amps[i] * (v - 0.5) * 2.0 * keep;
        if i >= 2u {
            hue += (v - 0.5) * keep;
        }
    }
    return vec2<f32>(1.0 + sum, 0.5 + hue);
}

// One placed plant for the draw, in view space: stem foot and scale, yawed
// horizontal axis and crown spread, up, then kind and three hash seeds
// (shape, brightness, hue).
struct Plant {
    base: vec4<f32>,
    e1: vec4<f32>,
    up: vec4<f32>,
    info: vec4<u32>,
}

struct Placed {
    ok: bool,
    plant: Plant,
}

// Place candidate `index` (drawn node index × SCATTER_SLOTS + slot): the
// hierarchy, rank, keep, water, forest field and species choice described
// at the top of this file.
fn sc_place(index: u32) -> Placed {
    var out: Placed;
    out.ok = false;
    let inst = instances[index / SCATTER_SLOTS];
    let slot = index % SCATTER_SLOTS;
    let mode = u32(inst.b2v_x.w + 0.5);
    // World maps only (flat ocean flag), lit views only.
    if inst.surface.x < 0.5 || !(mode == 0u || mode == 9u || mode == 11u) {
        return out;
    }
    let radius = inst.anchor.w;
    let width = inst.face_u.w;
    let q0 = inst.n0.w;
    // Whole node beyond the plant distance: nothing to draw.
    if length(inst.anchor.xyz) - 0.75 * width / q0 * radius > SCATTER_MAX_DISTANCE_M {
        return out;
    }
    let u_min = dot(inst.n0.xyz, inst.face_u.xyz) * q0 - 0.5 * width;
    let v_min = dot(inst.n0.xyz, inst.face_v.xyz) * q0 - 0.5 * width;
    let cells_per_unit = f32(1u << (SCATTER_CELL_BITS - 1u));
    let across = max(round(width * cells_per_unit), 1.0);
    let face = sc_face(inst.n0.xyz);
    let i0 = u32(round((u_min + 1.0) * cells_per_unit));
    let j0 = u32(round((v_min + 1.0) * cells_per_unit));
    var side = u32(across);
    var k = 0u;
    while side > SCATTER_SIDE {
        side = side >> 1u;
        k = k + 1u;
    }
    let a = slot % SCATTER_SIDE;
    let b = slot / SCATTER_SIDE;
    if a >= side || b >= side {
        return out;
    }
    var ci = (i0 >> k) + a;
    var cj = (j0 >> k) + b;
    for (var level = k; level > 0u; level = level - 1u) {
        let q = sc_pcg3d(vec3<u32>(ci, cj, face * 64u + level)).x >> 30u;
        ci = ci * 2u + (q & 1u);
        cj = cj * 2u + (q >> 1u);
    }
    var lambda = k;
    loop {
        let next = lambda + 1u;
        if next > SCATTER_MAX_LEVEL {
            break;
        }
        let q = sc_pcg3d(vec3<u32>(ci >> next, cj >> next, face * 64u + next)).x >> 30u;
        let own = ((ci >> lambda) & 1u) | (((cj >> lambda) & 1u) << 1u);
        if q != own {
            break;
        }
        lambda = next;
    }
    let h = sc_pcg3d(vec3<u32>(ci, cj, face * 64u + 63u));
    let h2 = sc_pcg3d(h ^ vec3<u32>(0x5ca77e5u));
    let rank = exp2(-2.0 * f32(lambda)) * (0.25 + 0.75 * sc_unit(h2.z ^ h.x));
    let u = (f32(ci) + 0.15 + 0.7 * sc_unit(h.x)) / cells_per_unit - 1.0;
    let v = (f32(cj) + 0.15 + 0.7 * sc_unit(h.y)) / cells_per_unit - 1.0;
    let st = vec2<f32>((u - u_min) / width, (v - v_min) / width);
    if any(st < vec2<f32>(0.0)) || any(st > vec2<f32>(1.0)) {
        return out;
    }
    let ground = blended_height(inst, st, 0.0);
    let base_view = to_view(inst, body_position(inst, st, ground));
    let keep = sc_keep(length(base_view));
    let grow = clamp((keep - rank) / (0.35 * keep + 1.0e-12), 0.0, 1.0);
    if grow <= 0.0 {
        return out;
    }
    let own_uv = normal_uv(own_st(inst, st));
    let page_n = textureSampleLevel(normal_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    if page_n.w > -0.2 {
        return out;
    }
    let climate = textureSampleLevel(climate_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    let diff = chart_diff(inst, st);
    let up_body = normalize(inst.n0.xyz + diff);
    let normal_body = normalize(page_n.xyz);
    let slope = acos(clamp(dot(normal_body, up_body), -1.0, 1.0));
    let site = sc_forest(face, f32(ci), f32(cj), 0.0, climate.x, climate.y, slope, ground);
    let roll = sc_unit(h.z);
    var kind = 3u;
    var scale = 1.0;
    if roll < site.tree {
        kind = select(1u, 0u, sc_unit(h2.x) < site.conifer);
        scale = mix(0.75, 1.3, sc_unit(h2.y)) * site.stature;
    } else if roll < site.tree + site.shrub {
        kind = 2u;
        scale = mix(0.6, 1.4, sc_unit(h2.y));
    } else if roll < site.tree + site.shrub + site.boulder {
        kind = 3u;
        scale = mix(0.5, 2.2, sc_unit(h2.y) * sc_unit(h2.x));
    } else {
        return out;
    }
    scale *= smoothstep(0.0, 1.0, grow);
    // Each kept tree or shrub stands for the 1 / keep candidates around it:
    // its crown spreads sideways (not up) so far forests close into a
    // canopy at true canopy height with a bumpy silhouette.
    var spread = 1.0;
    if kind <= 2u {
        spread = clamp(1.25 * inverseSqrt(max(keep, 1.0e-6)), 1.0, 30.0);
    }
    let yaw = sc_unit(h2.z) * 6.2831853;
    let e1r = normalize(inst.face_u.xyz - up_body * dot(inst.face_u.xyz, up_body));
    let e2r = cross(up_body, e1r);
    let e1 = e1r * cos(yaw) + e2r * sin(yaw);
    let to_view_dir = mat3x3<f32>(inst.b2v_x.xyz, inst.b2v_y.xyz, inst.b2v_z.xyz);
    out.plant.base = vec4<f32>(to_view(inst, body_position(inst, st, ground - 0.15 * scale)), scale);
    out.plant.e1 = vec4<f32>(to_view_dir * e1, spread);
    out.plant.up = vec4<f32>(to_view_dir * up_body, 0.0);
    out.plant.info = vec4<u32>(kind, h.x ^ h.y, h2.x ^ h2.y, h.y ^ h2.z);
    out.ok = true;
    return out;
}