// PROTOTYPE (M3 Water): river carve and lake/river water levels for world
// tiles, a port of crates/world/src/terrain/hydrology/carve.rs. The packed
// layout is documented in crates/world/src/terrain/hydrology/gpu.rs. Needs
// cube_map.wgsl (cube_locate, cube_index).

@group(1) @binding(6) var<storage, read> world_rivers: array<u32>;

fn rivers_f32(i: u32) -> f32 {
    return bitcast<f32>(world_rivers[i]);
}

struct RiverCarve {
    height: f32,
    // Gradient in metres per unit direction (as evaluate_world's).
    gradient: vec3<f32>,
    // Water surface (m), or -1e30 without water.
    water: f32,
    // 0..1: inside a river's riparian corridor (wetter ground, gallery
    // forests), strongest on the banks of large rivers.
    riparian: f32,
}

fn river_vertex(v: u32) -> u32 {
    return world_rivers[6u] + 8u * v;
}

fn river_position(v: u32) -> vec3<f32> {
    let b = river_vertex(v);
    return vec3<f32>(rivers_f32(b), rivers_f32(b + 1u), rivers_f32(b + 2u));
}

// 0.45..1.55 from a vertex index (integer hash).
fn river_swell(v: u32) -> f32 {
    var x = v * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    x = (x >> 22u) ^ x;
    return 0.45 + 1.1 * f32(x >> 8u) / 16777216.0;
}

fn river_cell(d: vec3<f32>, cells: u32) -> u32 {
    let at = cube_locate(d);
    let i = min(u32((at.u + 1.0) * 0.5 * f32(cells)), cells - 1u);
    let j = min(u32((at.v + 1.0) * 0.5 * f32(cells)), cells - 1u);
    return cube_index(at.face, i, j, cells);
}

// Carve height `h` (gradient `gradient`) at unit direction `d`; a channel
// narrower than `ribbon_m` is widened to it as a water ribbon so rivers stay
// visible at coarse levels.
fn river_carve(d: vec3<f32>, h: f32, gradient: vec3<f32>, ribbon_m: f32) -> RiverCarve {
    var out: RiverCarve;
    out.height = h;
    out.gradient = gradient;
    out.water = -1.0e30;
    out.riparian = 0.0;
    if arrayLength(&world_rivers) < 16u || world_rivers[7u] == 0u {
        return out;
    }
    let radius = rivers_f32(8u);
    let valley_factor = rivers_f32(9u);
    let valley_min = rivers_f32(10u);
    let valley_max = rivers_f32(11u);
    let floodplain_slope = rivers_f32(12u);
    let side_slope = rivers_f32(13u);
    let lake_n = world_rivers[5u];
    if lake_n > 0u {
        let level = rivers_f32(world_rivers[4u] + river_cell(d, lake_n));
        if level > -1.0e29 {
            out.water = level;
        }
    }
    let cells = world_rivers[1u];
    if cells == 0u || world_rivers[0u] == 0u {
        return out;
    }
    let c = river_cell(d, cells);
    let starts = world_rivers[2u];
    let first = world_rivers[starts + c];
    let last = world_rivers[starts + c + 1u];
    let segments = world_rivers[3u];
    for (var k = first; k < last; k = k + 1u) {
        let s = world_rivers[segments + k];
        let a = river_vertex(s);
        let down = world_rivers[a + 7u];
        if down == 0xffffffffu {
            continue;
        }
        let b = river_vertex(down);
        let pa = river_position(s);
        let pb = river_position(down);
        let ab = pb - pa;
        let t = clamp(dot(d - pa, ab) / max(dot(ab, ab), 1.0e-20), 0.0, 1.0);
        let q = normalize(pa + ab * t);
        let offset = d - q;
        let distance = length(offset) * radius;
        let width = mix(rivers_f32(a + 3u), rivers_f32(b + 3u), t);
        let depth = mix(rivers_f32(a + 4u), rivers_f32(b + 4u), t);
        let incision = mix(rivers_f32(a + 5u), rivers_f32(b + 5u), t);
        let bed = mix(rivers_f32(a + 6u), rivers_f32(b + 6u), t);
        let length_m = max(length(ab) * radius, 1.0);
        let slope = (rivers_f32(a + 6u) - rivers_f32(b + 6u)) / length_m;
        // Wide enough for the cut actually made here (the landform relief can
        // rise far above the macro surface the bed was set on), so deep cuts
        // get side slopes instead of a narrow trench.
        let cut = max(h - bed, 0.0);
        let half_valley = clamp(max(width * valley_factor, max(incision, cut) / side_slope), valley_min, valley_max);
        let edge = 0.5 * width;
        let span = max(half_valley - edge, 1.0);
        let x = max(distance - edge, 0.0) / span;
        let xc = min(x, 1.0);
        let plain = clamp(1.0 - slope / floodplain_slope, 0.0, 1.0);
        let pv = 1.0 - (1.0 - xc) * (1.0 - xc);
        let dpv = 2.0 * (1.0 - xc);
        let tt = clamp((xc - 0.5) / 0.5, 0.0, 1.0);
        let pf = 0.03 * xc + 0.97 * tt * tt * (3.0 - 2.0 * tt);
        let dpf = 0.03 + select(0.0, 0.97 * 6.0 * tt * (1.0 - tt) * 2.0, tt > 0.0 && tt < 1.0);
        let profile = mix(pv, pf, plain);
        let dprofile = select(mix(dpv, dpf, plain), 0.0, x >= 1.0 || distance <= edge);
        let carved = bed + (h - bed) * profile;
        if carved < out.height {
            out.height = carved;
            // Away-from-channel tangent direction (per unit direction).
            let tangent = offset - d * dot(d, offset);
            let away = select(vec3<f32>(0.0), normalize(tangent), dot(tangent, tangent) > 1.0e-20);
            // Where the cut sets the width, W = cut / side_slope also moves
            // with h: d carved / d h gains -(h - bed) P'(x) x / (span · side_slope).
            let w_cut = cut / side_slope;
            let cut_sets_width = w_cut >= width * valley_factor && w_cut >= incision / side_slope
                && w_cut > valley_min && w_cut < valley_max;
            let width_term = select(0.0, (h - bed) * dprofile * x / (span * side_slope), cut_sets_width);
            out.gradient = gradient * (profile - width_term) + away * ((h - bed) * dprofile * radius / span);
        }
        // Corridor width grows with the river; small streams get a thin one.
        // Per-vertex hashed widening, interpolated along the segment so the
        // corridor stays continuous but swells and narrows like real galleries.
        let swell = mix(river_swell(s), river_swell(down), t);
        let corridor = clamp(25.0 * width, 120.0, 2500.0) * swell;
        let size = smoothstep(2.0, 40.0, width);
        // Band limit: page texels wider than the corridor would point-sample it
        // into texel-sized blocks. Widen the falloff by the texel and scale by
        // the corridor's expected share of a texel, so coarse pages carry a
        // faint, smooth tint instead (texel = ribbon_m / 0.75).
        let texel = ribbon_m / 0.75;
        let inner = max(0.1 * corridor - texel, 0.0);
        let outer = corridor + texel;
        let coverage = min(1.0, corridor / max(texel, 1.0));
        out.riparian = max(out.riparian, size * coverage * (1.0 - smoothstep(inner, outer, distance)));
        if distance <= max(edge, ribbon_m) {
            var surface = bed + depth;
            if distance > edge {
                // Ribbon: float just above the carved ground.
                surface = max(surface, carved + 0.1);
            }
            out.water = max(out.water, surface);
        }
    }
    return out;
}
