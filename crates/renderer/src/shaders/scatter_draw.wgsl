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
// levels and refinement adds nothing that pops. Plants dissolve in (dither,
// constant size) as keep(d) passes their rank. Beyond the drawn plants the
// terrain fragment shader lays the canopy colour (forest_cover) with the
// coverage the thinned plants no longer provide.
//
// Forest field (scatter_niche.wgsl): the species niches (content/flora/species,
// compiled in as scatter_species.wgsl) set how much of the land is forest;
// value noise against a suitability threshold makes large contiguous forests
// in good climates and patches in marginal ones. A fringe outside each forest
// carries shrubs, small trees and lone trees; trees shrink towards the edge of
// their niche (treeline, dry or cold margins). Which species grows is drawn
// from the niches that fit the site.

const SCATTER_SIDE: u32 = 32u;
const SCATTER_SLOTS: u32 = 1024u;
const SCATTER_VERTS: u32 = 96u;
const SCATTER_CELL_BITS: u32 = 16u;
// Full density within this view distance (m); keep(d) = (FULL / d)².
const SCATTER_FULL_M: f32 = 800.0;
// No plants beyond this view distance (m), faded over the last 30 %.
const SCATTER_MAX_DISTANCE_M: f32 = 16000.0;
const SCATTER_DEG: f32 = 0.017453292;
const SCATTER_MAX_LEVEL: u32 = 12u;

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
// position `st` and view position `view_pos`: the forest field band-limited
// to the drawn grid. Strength = share of the view hidden by the crowns the
// thinning no longer draws, with crowns seen at the view angle: if all
// plants hide C(λ) and the drawn ones C(keep·λ), the tint covers
// 1 - (1 - C(λ)) / (1 - C(keep·λ)) of the rest, so together they hide C(λ)
// at every distance (fl_hidden_by_undrawn; examples/flora_cover_check.rs).
// `footprint`: base cells per pixel (screen-space derivative of `c`, taken
// by the caller in uniform control flow). A per-node footprint would differ
// between neighbouring nodes of different levels and turn the octave fades
// into node-shaped blocks.
fn forest_cover(inst: Instance, st: vec2<f32>, normal_body: vec3<f32>, climate: vec2<f32>, sediment: f32, ground: f32, view_pos: vec3<f32>, footprint_px: f32) -> vec4<f32> {
    let c = sc_cells(inst, st);
    // Fade each octave once it spans fewer than about eight pixels.
    let footprint = 2.0 * footprint_px;
    let up_body = normalize(inst.n0.xyz + chart_diff(inst, st));
    let slope = acos(clamp(dot(normal_body, up_body), -1.0, 1.0));
    // View angle against the local vertical and the base cell's area
    // (gnomonic cube: (R / cells per unit)² / |q0|³).
    let n0_view = inst.b2v_x.xyz * inst.n0.x + inst.b2v_y.xyz * inst.n0.y + inst.b2v_z.xyz * inst.n0.z;
    let up_view = normalize(view_pos - (inst.anchor.xyz - n0_view * inst.anchor.w));
    let d = length(view_pos);
    let cos_v = dot(up_view, -view_pos / max(d, 1.0e-3));
    let cell_m = inst.anchor.w / f32(1u << (SCATTER_CELL_BITS - 1u));
    let cell_m2 = cell_m * cell_m / (inst.n0.w * inst.n0.w * inst.n0.w);
    let view = vec3<f32>(1.0 / max(cell_m2, 1.0e-3), sc_keep(d), cos_v);
    let site = sc_forest(sc_face(inst.n0.xyz), c.x, c.y, footprint, FlSite(climate.x, climate.y, ground, slope, sediment), view);
    var colour = site.color;
    let face = sc_face(inst.n0.xyz);
    // Crown cells: the tint shows the crowns it stands for as sun-lit domes
    // at the plants' own positions (sc_plant's cell hash and roll) over a
    // shaded floor, so a far forest keeps its crown-scale light and shade.
    // Mean-preserving; fades to the mean once a cell spans under ~2 pixels.
    let detail = 1.0 - smoothstep(0.3, 0.7, footprint_px);
    if detail > 0.0 {
        let fsite = FlSite(climate.x, climate.y, ground, slope, sediment);
        let crown_m2 = fl_layer_crown(FL_CANOPY_MASK, fsite).x;
        let rc = clamp(sqrt(crown_m2 / 3.14159265) * site.stature / max(cell_m * inverseSqrt(inst.n0.w * inst.n0.w * inst.n0.w), 1.0e-3), 0.05, 1.2);
        // Sun in the cell frame (face u, face v, up).
        let sun_view = normalize(lighting.sun.xyz - view_pos);
        let sun_body = vec3<f32>(dot(inst.b2v_x.xyz, sun_view), dot(inst.b2v_y.xyz, sun_view), dot(inst.b2v_z.xyz, sun_view));
        let tu = normalize(inst.face_u.xyz - up_body * dot(inst.face_u.xyz, up_body));
        let tv = normalize(cross(up_body, tu));
        let sun_t = vec3<f32>(dot(sun_body, tu), dot(sun_body, tv), dot(sun_body, up_body));
        let base = floor(c);
        var best = -1.0;
        var lit = 0.0;
        for (var dj = -1; dj <= 1; dj = dj + 1) {
            for (var di = -1; di <= 1; di = di + 1) {
                let cell = base + vec2<f32>(f32(di), f32(dj));
                let h = sc_pcg3d(vec3<u32>(u32(i32(cell.x)), u32(i32(cell.y)), face * 64u + 63u));
                if sc_unit(h.z) >= site.tree + site.shrub {
                    continue;
                }
                let centre = cell + vec2<f32>(0.15 + 0.7 * sc_unit(h.x), 0.15 + 0.7 * sc_unit(h.y));
                let off = (c - centre) / rc;
                let r2 = dot(off, off);
                // Topmost dome wins where crowns overlap.
                let z = sqrt(max(1.0 - r2, 0.0));
                if r2 < 1.0 && z > best {
                    best = z;
                    lit = max(dot(normalize(vec3<f32>(off, z)), sun_t), 0.0);
                }
            }
        }
        let inside = select(0.0, 1.0, best >= 0.0);
        let pattern = mix(0.3, 0.45 + 0.9 * lit, inside);
        // Expected pattern: crown share of the ground times the mean lit
        // term of a dome seen from above (2/3 of the sun's height).
        let share = clamp((site.tree + site.shrub) * 3.14159265 * rc * rc, 0.0, 0.95);
        let mean = mix(0.3, 0.45 + 0.9 * (2.0 / 3.0) * max(sun_t.z, 0.0), share);
        colour *= mix(1.0, pattern / max(mean, 0.05), detail);
    }
    // Crown groups (~30 m) and stands (~90 m): light and shade that carry
    // the canopy's roughness once single crowns are sub-pixel, each fading
    // to its mean once the grid is too coarse to carry it.
    let groups = mix(sc_value(face, c.x / 3.0, c.y / 3.0, 44u), 0.5, smoothstep(0.75, 1.5, footprint));
    let clumps = mix(sc_value(face, c.x / 9.0, c.y / 9.0, 45u), 0.5, smoothstep(2.25, 4.5, footprint));
    colour *= 1.0 + 0.7 * (groups - 0.5) * (1.0 - 0.5 * detail) + 0.7 * (clumps - 0.5);
    // Once single crowns are sub-pixel (far views, orbit) the tint stands
    // for the canopy's average under light: crowns plus the shade between
    // and inside them, so darker and less saturated than the crown-top
    // albedo, and the gaps let some ground through.
    let far = 1.0 - detail;
    let luma = dot(colour, vec3<f32>(0.2126, 0.7152, 0.0722));
    colour = mix(colour, mix(vec3<f32>(luma), colour, FL_FAR_CHROMA) * FL_FAR_SHADE, far);
    return vec4<f32>(colour, clamp(site.cover * mix(1.0, FL_FAR_COVER, far), 0.0, 0.98));
}

// Far canopy average (forest_cover): chroma kept, brightness kept, share of
// the ground the crowns hide once single crowns are sub-pixel.
const FL_FAR_CHROMA: f32 = 0.6;
const FL_FAR_SHADE: f32 = 0.7;
const FL_FAR_COVER: f32 = 0.85;

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

// View distance to the node centre on the ground (`anchor` is at the
// reference radius, which can lie kilometres below the terrain).
fn sc_node_distance(inst: Instance) -> f32 {
    let centre = vec2<f32>(0.5);
    return length(to_view(inst, body_position(inst, centre, blended_height(inst, centre, 0.0))));
}

// One placed plant for the draw, in view space: stem foot and scale, yawed
// horizontal axis and rank fade (0..1), up and tier split (flora), then
// kind (bits 0..7 shape, 8..15 species + 1, 16.. flora bucket) and three
// hash seeds (variant, brightness, hue).
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
    if sc_node_distance(inst) - 0.75 * width / q0 * radius > SCATTER_MAX_DISTANCE_M {
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
    // Shape page: hardness (.y) picks the rock archetype, sediment (.z) is soil.
    let shape = textureSampleLevel(shape_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    let fsite = FlSite(climate.x, climate.y, ground, slope, shape.z);
    let site = sc_forest(face, f32(ci), f32(cj), 0.0, fsite, vec3<f32>(0.0, 0.0, 1.0));
    let roll = sc_unit(h.z);
    // kind: low byte = procedural far shape (0 conifer, 1 broadleaf, 2 shrub,
    // 3 boulder), bits 8.. = species index + 1 (0 = none).
    var kind = 3u;
    var scale = 1.0;
    // Low bit picks the grown variant (scatter_cull.wgsl).
    var variant_bits = h.x ^ h.y;
    if roll < site.tree + site.shrub {
        let canopy = roll < site.tree;
        let pick = fl_pick(select(FL_SHRUB_MASK, FL_CANOPY_MASK, canopy), fsite, sc_unit(h2.x));
        if pick.x < 0.0 {
            return out;
        }
        let k = u32(pick.x);
        let n = fl_niche(k);
        kind = n.far_kind | ((k + 1u) << 8u);
        scale = mix(n.scale.x, n.scale.y, sc_unit(h2.y));
        if canopy {
            scale *= mix(0.45, 1.0, pick.y) * mix(0.6, 1.0, site.core);
        }
    } else if roll < site.tree + site.shrub + site.boulder {
        kind = 3u;
        scale = mix(0.5, 2.2, sc_unit(h2.y) * sc_unit(h2.x));
        // Grown rock of the bedrock's archetype; the wet variant (weathered,
        // mossy) where the climate is moist.
        let r = fl_pick_rock(shape.y, sc_unit(h2.x ^ 0x51ed27u));
        if r >= 0 {
            kind = 3u | ((FL_ROCK_ENTRY + u32(r) + 1u) << 8u);
            scale = mix(0.35, 1.3, sc_unit(h2.y) * sc_unit(h2.x));
            variant_bits = (variant_bits & ~1u) | select(0u, 1u, climate.y > 0.5);
        }
    } else {
        return out;
    }
    // A plant keeps its size at every distance (no grow-in, no sideways
    // spread): as keep(d) passes its rank it dissolves with a screen-door
    // dither (`fade` in e1.w), and the canopy tint takes over its coverage.
    let fade = smoothstep(0.0, 1.0, grow);
    let yaw = sc_unit(h2.z) * 6.2831853;
    let e1r = normalize(inst.face_u.xyz - up_body * dot(inst.face_u.xyz, up_body));
    let e2r = cross(up_body, e1r);
    let e1 = e1r * cos(yaw) + e2r * sin(yaw);
    let to_view_dir = mat3x3<f32>(inst.b2v_x.xyz, inst.b2v_y.xyz, inst.b2v_z.xyz);
    out.plant.base = vec4<f32>(to_view(inst, body_position(inst, st, ground - 0.15 * scale)), scale);
    out.plant.e1 = vec4<f32>(to_view_dir * e1, fade);
    out.plant.up = vec4<f32>(to_view_dir * up_body, 1.0);
    out.plant.info = vec4<u32>(kind, variant_bits, h2.x ^ h2.y, h.y ^ h2.z);
    out.ok = true;
    return out;
}
// ---------------------------------------------------------------- grass

// PROTOTYPE (M5 Life): grass tufts on a 1 m world grid near the camera, the
// same hierarchical rank scheme as the plants (keep(d) = (GRASS_FULL / d)²,
// none beyond GRASS_MAX), so tufts thin smoothly with distance and never pop
// on refinement. Meadows get dense grass; forest cores, steep rock, water
// and dry desert little or none.
const GRASS_CELL_BITS: u32 = 19u;
const GRASS_FULL_M: f32 = 22.0;
const GRASS_MAX_M: f32 = 70.0;
const GRASS_VERTS: u32 = 18u;

fn grass_keep(d: f32) -> f32 {
    let ratio = GRASS_FULL_M / max(d, 0.5);
    return min(1.0, ratio * ratio) * (1.0 - smoothstep(0.75 * GRASS_MAX_M, GRASS_MAX_M, d));
}

fn sc_place_grass(index: u32) -> Placed {
    var out: Placed;
    out.ok = false;
    let inst = instances[index / SCATTER_SLOTS];
    let slot = index % SCATTER_SLOTS;
    let mode = u32(inst.b2v_x.w + 0.5);
    if inst.surface.x < 0.5 || !(mode == 0u || mode == 9u || mode == 11u) {
        return out;
    }
    let radius = inst.anchor.w;
    let width = inst.face_u.w;
    let q0 = inst.n0.w;
    if sc_node_distance(inst) - 0.75 * width / q0 * radius > GRASS_MAX_M {
        return out;
    }
    let u_min = dot(inst.n0.xyz, inst.face_u.xyz) * q0 - 0.5 * width;
    let v_min = dot(inst.n0.xyz, inst.face_v.xyz) * q0 - 0.5 * width;
    let cells_per_unit = f32(1u << (GRASS_CELL_BITS - 1u));
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
    let salt = face * 64u + 32u;
    for (var level = k; level > 0u; level = level - 1u) {
        let q = sc_pcg3d(vec3<u32>(ci, cj, salt + level)).x >> 30u;
        ci = ci * 2u + (q & 1u);
        cj = cj * 2u + (q >> 1u);
    }
    var lambda = k;
    loop {
        let next = lambda + 1u;
        if next > SCATTER_MAX_LEVEL {
            break;
        }
        let q = sc_pcg3d(vec3<u32>(ci >> next, cj >> next, salt + next)).x >> 30u;
        let own = ((ci >> lambda) & 1u) | (((cj >> lambda) & 1u) << 1u);
        if q != own {
            break;
        }
        lambda = next;
    }
    let h = sc_pcg3d(vec3<u32>(ci, cj, salt + 31u));
    let h2 = sc_pcg3d(h ^ vec3<u32>(0x9a55u));
    let rank = exp2(-2.0 * f32(lambda)) * (0.25 + 0.75 * sc_unit(h2.z ^ h.x));
    let u = (f32(ci) + sc_unit(h.x)) / cells_per_unit - 1.0;
    let v = (f32(cj) + sc_unit(h.y)) / cells_per_unit - 1.0;
    let st = vec2<f32>((u - u_min) / width, (v - v_min) / width);
    if any(st < vec2<f32>(0.0)) || any(st > vec2<f32>(1.0)) {
        return out;
    }
    let ground = blended_height(inst, st, 0.0);
    let keep = grass_keep(length(to_view(inst, body_position(inst, st, ground))));
    let grow = clamp((keep - rank) / (0.35 * keep + 1.0e-12), 0.0, 1.0);
    if grow <= 0.0 {
        return out;
    }
    let own_uv = normal_uv(own_st(inst, st));
    let page_n = textureSampleLevel(normal_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    if page_n.w > -0.3 {
        return out;
    }
    let climate = textureSampleLevel(climate_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    let up_body = normalize(inst.n0.xyz + chart_diff(inst, st));
    let slope = acos(clamp(dot(normalize(page_n.xyz), up_body), -1.0, 1.0));
    // Forest field on the plant grid (8 m cells) for the shade of forest cores.
    let plant_cells = f32(1u << (SCATTER_CELL_BITS - 1u));
    let sediment = textureSampleLevel(shape_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0).z;
    let site = sc_forest(face, (u + 1.0) * plant_cells, (v + 1.0) * plant_cells, 0.0, FlSite(climate.x, climate.y, ground, slope, sediment), vec3<f32>(0.0, 0.0, 1.0));
    let meadow = (1.0 - 0.75 * site.core)
        * smoothstep(-3.0, 4.0, climate.x)
        * smoothstep(0.06, 0.3, climate.y)
        * (1.0 - smoothstep(30.0 * SCATTER_DEG, 45.0 * SCATTER_DEG, slope));
    if sc_unit(h.z) >= 0.95 * meadow {
        return out;
    }
    let scale = mix(0.6, 1.4, sc_unit(h2.y)) * smoothstep(0.0, 1.0, grow);
    let yaw = sc_unit(h2.x) * 6.2831853;
    let e1r = normalize(inst.face_u.xyz - up_body * dot(inst.face_u.xyz, up_body));
    let e2r = cross(up_body, e1r);
    let e1 = e1r * cos(yaw) + e2r * sin(yaw);
    let to_view_dir = mat3x3<f32>(inst.b2v_x.xyz, inst.b2v_y.xyz, inst.b2v_z.xyz);
    let dry = 1.0 - smoothstep(0.08, 0.35, climate.y);
    out.plant.base = vec4<f32>(to_view(inst, body_position(inst, st, ground - 0.03)), scale);
    out.plant.e1 = vec4<f32>(to_view_dir * e1, dry);
    out.plant.up = vec4<f32>(to_view_dir * up_body, 0.0);
    // Blade colour from the page albedo (sRGB) so grass matches the biome,
    // slightly greener and more saturated, drying to straw in dry climates.
    let page_a = textureSampleLevel(albedo_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    let ground_lin = pow(page_a.rgb, vec3<f32>(2.2));
    let lush = ground_lin * vec3<f32>(0.85, 1.1, 0.75) * (0.85 + 0.3 * sc_unit(h2.x));
    let straw = vec3<f32>(0.2, 0.17, 0.08);
    let blade = mix(lush, straw, dry * 0.6);
    out.plant.info = vec4<u32>(4u, h.x ^ h2.y, pack4x8unorm(vec4<f32>(sqrt(clamp(blade, vec3<f32>(0.0), vec3<f32>(1.0))), 0.0)), h.y ^ h2.z);
    out.ok = true;
    return out;
}
