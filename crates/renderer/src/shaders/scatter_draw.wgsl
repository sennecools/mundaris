// PROTOTYPE (M5 Life): procedural conifers, shrubs and boulders drawn on the
// selected world-map terrain nodes (pipeline §12), appended to the terrain
// draw shader (same bindings and helpers).
//
// Placement: base cells are a fixed 2^SCATTER_CELL_BITS grid per cube face
// (~10 m on Rust), so trees are anchored to the world, not to nodes. Every
// drawn node owns SCATTER_SIDE² slots; a node wider than SCATTER_SIDE base
// cells maps each slot to one coarse cell and descends to a base cell by
// hashed quadrant choices, so a coarse node draws a subset of the trees its
// children draw (no pop on refinement, thinning with distance). One candidate
// per base cell, jittered and accepted by a density from the page climate,
// slope and height (species ranges from scatter.wgsl). Water pages (sea,
// lakes, rivers) carry none. Geometry is procedural (48 vertices: crown or
// rock top, trunk or rock base), flat shaded, lit like terrain, no shadows.

const SCATTER_SIDE: u32 = 16u;
const SCATTER_SLOTS: u32 = 256u;
const SCATTER_VERTS: u32 = 48u;
const SCATTER_CELL_BITS: u32 = 16u;
// Nodes wider than this (m) draw nothing.
const SCATTER_MAX_NODE_M: f32 = 2000.0;
const SCATTER_DEG: f32 = 0.017453292;

struct ScatterOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) @interpolate(flat) normal: vec3<f32>,
    @location(2) @interpolate(flat) up: vec3<f32>,
    @location(3) @interpolate(flat) albedo: vec3<f32>,
}

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

// Density of species `k` (0 conifer, 1 shrub, 2 boulder) at a site; the
// ranges follow scatter.wgsl's species_def.
fn sc_density(k: u32, t: f32, m: f32, slope: f32, h: f32) -> f32 {
    let rock = smoothstep(25.0 * SCATTER_DEG, 45.0 * SCATTER_DEG, slope);
    if k == 0u {
        return 0.85 * sc_range(-8.0, 16.0, 4.0, t) * sc_range(0.3, 1.0, 0.1, m)
            * sc_range(0.0, 3000.0, 200.0, h) * sc_range(0.0, 38.0 * SCATTER_DEG, 8.0 * SCATTER_DEG, slope)
            * (1.0 - rock);
    }
    if k == 1u {
        return 0.7 * sc_range(-2.0, 32.0, 6.0, t) * sc_range(0.2, 1.0, 0.1, m)
            * sc_range(0.0, 2400.0, 300.0, h) * sc_range(0.0, 45.0 * SCATTER_DEG, 10.0 * SCATTER_DEG, slope)
            * (1.0 - 0.5 * rock);
    }
    return 0.35 * sc_range(0.0, 60.0 * SCATTER_DEG, 10.0 * SCATTER_DEG, slope) * (0.25 + 0.75 * rock);
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

// Local corner `corner` of triangle `tri` (0..15) of species `k` at `scale`:
// triangles 0..7 the crown (or rock top), 8..15 the trunk (or rock base).
fn sc_corner(k: u32, tri: u32, corner: u32, scale: f32) -> vec3<f32> {
    var crown_base = 2.0;
    var crown_top = 11.0;
    var crown_r = 2.3;
    var trunk_r = 0.3;
    var trunk_bottom = -0.6;
    if k == 1u {
        crown_base = 0.3;
        crown_top = 2.4;
        crown_r = 1.7;
        trunk_r = 0.15;
    } else if k == 2u {
        crown_base = 0.35;
        crown_top = 1.1;
        crown_r = 1.25;
        trunk_r = 1.25;
        trunk_bottom = -0.4;
    }
    if tri < 8u {
        if corner == 0u {
            return vec3<f32>(0.0, 0.0, crown_top) * scale;
        }
        let a = f32((tri + corner - 1u) % 8u) * (6.2831853 / 8.0);
        return vec3<f32>(cos(a) * crown_r, sin(a) * crown_r, crown_base) * scale;
    }
    // Trunk / rock base: 4 quads (two triangles each) around 4 sides.
    let q = (tri - 8u) / 2u;
    let second = (tri - 8u) % 2u;
    let a0 = f32(q) * (6.2831853 / 4.0);
    let a1 = f32(q + 1u) * (6.2831853 / 4.0);
    let top = crown_base;
    var r_top = trunk_r;
    if k == 2u {
        r_top = crown_r;
    }
    let p00 = vec3<f32>(cos(a0) * trunk_r, sin(a0) * trunk_r, trunk_bottom);
    let p10 = vec3<f32>(cos(a1) * trunk_r, sin(a1) * trunk_r, trunk_bottom);
    let p01 = vec3<f32>(cos(a0) * r_top, sin(a0) * r_top, top);
    let p11 = vec3<f32>(cos(a1) * r_top, sin(a1) * r_top, top);
    var p = p00;
    if second == 0u {
        p = select(select(p01, p10, corner == 1u), p00, corner == 0u);
    } else {
        p = select(select(p01, p11, corner == 1u), p10, corner == 0u);
    }
    return p * scale;
}

fn sc_hidden() -> ScatterOut {
    var out: ScatterOut;
    out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    out.view_pos = vec3<f32>(0.0);
    out.normal = vec3<f32>(0.0, 0.0, 1.0);
    out.up = vec3<f32>(0.0, 0.0, 1.0);
    out.albedo = vec3<f32>(0.0);
    return out;
}

@vertex
fn vs_scatter(@builtin(vertex_index) vertex: u32, @builtin(instance_index) index: u32) -> ScatterOut {
    let inst = instances[index / SCATTER_SLOTS];
    let slot = index % SCATTER_SLOTS;
    let mode = u32(inst.b2v_x.w + 0.5);
    // World maps only (flat ocean flag), lit views only.
    if inst.surface.x < 0.5 || !(mode == 0u || mode == 9u || mode == 11u) {
        return sc_hidden();
    }
    let radius = inst.anchor.w;
    let width = inst.face_u.w;
    let q0 = inst.n0.w;
    let node_m = width / q0 * radius;
    if node_m > SCATTER_MAX_NODE_M {
        return sc_hidden();
    }
    // Chart (face-plane) coordinates of the node's lower corner.
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
        return sc_hidden();
    }
    // Coarse cell, then hashed quadrant choices down to a base cell.
    var ci = (i0 >> k) + a;
    var cj = (j0 >> k) + b;
    for (var level = k; level > 0u; level = level - 1u) {
        let q = sc_pcg3d(vec3<u32>(ci, cj, face * 64u + level)).x >> 30u;
        ci = ci * 2u + (q & 1u);
        cj = cj * 2u + (q >> 1u);
    }
    let h = sc_pcg3d(vec3<u32>(ci, cj, face * 64u + 63u));
    let h2 = sc_pcg3d(h ^ vec3<u32>(0x5ca77e5u));
    let u = (f32(ci) + 0.15 + 0.7 * sc_unit(h.x)) / cells_per_unit - 1.0;
    let v = (f32(cj) + 0.15 + 0.7 * sc_unit(h.y)) / cells_per_unit - 1.0;
    let st = vec2<f32>((u - u_min) / width, (v - v_min) / width);
    if any(st < vec2<f32>(0.0)) || any(st > vec2<f32>(1.0)) {
        return sc_hidden();
    }
    // Site from the node's own page.
    let own_uv = normal_uv(own_st(inst, st));
    let page_n = textureSampleLevel(normal_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    if page_n.w > -0.2 {
        // Water (or its shore band).
        return sc_hidden();
    }
    let climate = textureSampleLevel(climate_atlas, normal_sampler, own_uv, i32(inst.own.x), 0.0);
    let ground = blended_height(inst, st, 0.0);
    let diff = chart_diff(inst, st);
    let up_body = normalize(inst.n0.xyz + diff);
    let normal_body = normalize(page_n.xyz);
    let slope = acos(clamp(dot(normal_body, up_body), -1.0, 1.0));
    // Clearings and forest edges: two octaves of value noise (~500 m and
    // ~120 m) gate trees and shrubs.
    let cover = 0.65 * sc_value(face, f32(ci) / 48.0, f32(cj) / 48.0, 40u)
        + 0.35 * sc_value(face, f32(ci) / 12.0, f32(cj) / 12.0, 41u);
    let wooded = smoothstep(0.38, 0.6, cover);
    let d0 = sc_density(0u, climate.x, climate.y, slope, ground) * wooded;
    let d1 = sc_density(1u, climate.x, climate.y, slope, ground) * mix(0.3, 1.0, wooded);
    let d2 = sc_density(2u, climate.x, climate.y, slope, ground);
    let total = d0 + d1 + d2;
    if sc_unit(h.z) >= min(total, 1.0) {
        return sc_hidden();
    }
    let pick = sc_unit(h2.x) * total;
    var species = 2u;
    if pick < d0 {
        species = 0u;
    } else if pick < d0 + d1 {
        species = 1u;
    }
    let scale = mix(0.7, 1.35, sc_unit(h2.y)) * select(1.0, mix(0.6, 1.8, sc_unit(h2.z)), species == 2u);
    let yaw = sc_unit(h2.z) * 6.2831853;
    // Local frame: up, and a yawed horizontal pair from the face U axis.
    let e1r = normalize(inst.face_u.xyz - up_body * dot(inst.face_u.xyz, up_body));
    let e2r = cross(up_body, e1r);
    let e1 = e1r * cos(yaw) + e2r * sin(yaw);
    let e2 = cross(up_body, e1);
    let tri = vertex / 3u;
    let corner = vertex % 3u;
    let c0 = sc_corner(species, tri, 0u, scale);
    let c1 = sc_corner(species, tri, 1u, scale);
    let c2 = sc_corner(species, tri, 2u, scale);
    let local = sc_corner(species, tri, corner, scale);
    var local_n = normalize(cross(c1 - c0, c2 - c0));
    // Face outward (away from the stem axis).
    let centre = (c0 + c1 + c2) / 3.0;
    if dot(local_n, vec3<f32>(centre.x, centre.y, 0.0)) < 0.0 {
        local_n = -local_n;
    }
    let to_body = mat3x3<f32>(e1, e2, up_body);
    let base = body_position(inst, st, ground - 0.2);
    let view_position = to_view(inst, base + to_body * local);
    let n_body = to_body * local_n;
    var out: ScatterOut;
    out.clip_position = projection.matrix * vec4<f32>(view_position, 1.0);
    out.view_pos = view_position;
    out.normal = normalize(inst.b2v_x.xyz * n_body.x + inst.b2v_y.xyz * n_body.y + inst.b2v_z.xyz * n_body.z);
    out.up = normalize(inst.b2v_x.xyz * up_body.x + inst.b2v_y.xyz * up_body.y + inst.b2v_z.xyz * up_body.z);
    let tint = 0.8 + 0.4 * sc_unit(h2.x ^ h2.y);
    if species == 2u {
        out.albedo = vec3<f32>(0.2, 0.19, 0.17) * tint;
    } else if tri >= 8u {
        out.albedo = vec3<f32>(0.08, 0.05, 0.03);
    } else if species == 0u {
        out.albedo = vec3<f32>(0.03, 0.065, 0.03) * tint;
    } else {
        out.albedo = vec3<f32>(0.06, 0.09, 0.035) * tint;
    }
    return out;
}

@fragment
fn fs_scatter(input: ScatterOut) -> SceneOut {
    return shade(input.view_pos, input.normal, input.up, input.albedo, 0.0, true);
}
