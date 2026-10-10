// Tier A world-map bake (ASTRUM_TIER_A_PIPELINE.md §6) on the GPU, the terrain
// authority under ADR 0023. Mirrors crates/world/src/terrain/tier_a/ (the CPU
// test oracle) stage by stage. Prepended with terrain_noise.wgsl and
// cube_map.wgsl.
//
// All fields of one body live in `fields` (u32 words; float runs travel as
// bit patterns through bitcast, so packed and integer runs are never rounded
// as f32). Every pass names the runs it reads and writes as word offsets in
// `pass_info.o` (laid out by renderer::tier_a), and its texel grid in
// `pass_info.n`: coarse erosion levels and mips use the same entry points.

struct Params {
    grid: vec4<u32>,        // n, stage flags, plates, unused
    continents: vec4<f32>,  // radius, continent frequency, warp frequency, warp scale
    octaves: vec4<f32>,     // octaves, lacunarity, gain, crust weight
    seeds: vec4<u32>,       // continent, warp, temperature, tectonic
    relief: vec4<f32>,      // ocean coverage, land height, land exponent, ocean depth
    shelf: vec4<f32>,       // shelf depth, shelf fraction, height bound, flow normaliser
    climate: vec4<f32>,     // equator °C, pole °C, axial tilt rad, lapse °C/km
    moderation: vec4<f32>,  // ocean moderation, unused, noise °C, noise frequency
    wind: vec4<f32>,        // cells per hemisphere, meridional fraction, evaporation, rain
    moisture: vec4<f32>,    // step rad, spread, precipitation scale, rain convergence
    pole: vec4<f32>,        // body-fixed rotation axis
    plates: vec4<f32>,      // plate warp frequency, plate warp scale, softness m, clamp m
    heights: vec4<f32>,     // collision, arc, trench, ridge (m)
    heights2: vec4<f32>,    // rift m, transform m, roughness, roughness frequency
    widths: vec4<f32>,      // orogen, arc width, arc offset, trench width (m)
    widths2: vec4<f32>,     // ridge, rift, transform, crust widths (m)
    hardness: vec4<f32>,    // noise amplitude, noise frequency, unused, unused
    seeds2: vec4<u32>,      // hardness seed, unused...
    shadow: vec4<f32>,      // unused, deflection, deflection slope, slowdown
    orographic: vec4<f32>,  // orographic rain, lee drying, reference slope, unused
    erosion: vec4<f32>,     // strength K, uplift per iteration (m), tan talus, deposition
    erosion2: vec4<f32>,    // capacity, area exponent m, slope exponent n, MFD exponent p
    erosion3: vec4<f32>,    // thermal rate, sediment depth (m), unused, unused
}

struct Plate {
    centre: vec4<f32>,      // unit seed direction, power weight
    spin: vec4<f32>,        // ω·axis, continental 0/1
    props: vec4<f32>,       // rock-family hardness, unused...
    unused: vec4<f32>,
}

struct Pass {
    n: u32,                 // face cells of this pass's texel grid
    sample_n: u32,          // face cells of cube_* lookups (cube_n())
    mode: u32,
    flags: u32,
    scalars: vec4<f32>,
    o: array<vec4<u32>, 3>, // run word offsets o(0)..o(11)
}

const HISTOGRAM_BINS: u32 = 4096u;
const STATS_SEA_LEVEL: u32 = 4096u;
const STATS_LOW: u32 = 4097u;
const STATS_HIGH: u32 = 4098u;
const STATS_HISTOGRAM_2: u32 = 4100u;
const STATS_SEA_LEVEL_2: u32 = 8196u;
const STAGE_CONTINENTS: u32 = 1u;
const STAGE_SEA_LEVEL: u32 = 2u;
const STAGE_SHELF: u32 = 4u;
const STAGE_TEMPERATURE: u32 = 8u;
const STAGE_WIND: u32 = 16u;
const STAGE_MOISTURE: u32 = 32u;
const STAGE_TECTONICS: u32 = 64u;
const STAGE_RAIN_SHADOW: u32 = 128u;
const STAGE_EROSION: u32 = 256u;
const UNFILLED_M: f32 = 1.0e7;
const PIT_FILL_M: f32 = 0.1;
const INCISION_CLEARANCE_M: f32 = 0.01;
const BOUNDARY_UNITS_PER_M: f32 = 16.0;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> fields: array<u32>;
@group(0) @binding(2) var<storage, read_write> stats: array<atomic<u32>>;
@group(0) @binding(3) var<uniform> pass_info: Pass;
@group(0) @binding(4) var<uniform> plates: array<Plate, 32>;
// Landform weight-rule set bytecode (`landform::expr::encode_set`).
@group(0) @binding(5) var<storage, read> landform_rules: array<u32>;

fn cube_field(index: u32) -> f32 {
    return bitcast<f32>(fields[index]);
}

fn cube_n() -> u32 {
    return pass_info.sample_n;
}

fn has(stage: u32) -> bool {
    return (params.grid.y & stage) != 0u;
}

// Run word offset `k` of this pass.
fn o(k: u32) -> u32 {
    return pass_info.o[k / 4u][k % 4u];
}

fn read_f(run: u32, k: u32) -> f32 {
    return bitcast<f32>(fields[run + k]);
}

fn write_f(run: u32, k: u32, value: f32) {
    fields[run + k] = bitcast<u32>(value);
}

struct Texel {
    k: u32,
    face: u32,
    i: u32,
    j: u32,
    valid: bool,
}

fn texel(id: vec3<u32>) -> Texel {
    let n = pass_info.n;
    var out: Texel;
    out.k = id.x + id.y * 65535u * 256u;
    out.valid = out.k < 6u * n * n;
    out.face = out.k / (n * n);
    out.j = (out.k / n) % n;
    out.i = out.k % n;
    return out;
}

fn fbm(p: vec3<f32>, frequency: f32, octaves: u32, lacunarity: f32, gain: f32, seed: u32) -> f32 {
    var sum = 0.0;
    var norm = 0.0;
    var amplitude = 1.0;
    var f = frequency;
    for (var k = 0u; k < octaves; k = k + 1u) {
        sum += amplitude * gradient_noise(p * f, seed + k);
        norm += amplitude;
        amplitude *= gain;
        f *= lacunarity;
    }
    return sum / norm;
}

// `d` displaced by three 3-octave fBm fields (seeds seed, +16, +32).
fn warp_direction(d: vec3<f32>, frequency: f32, scale: f32, seed: u32) -> vec3<f32> {
    let warp = vec3<f32>(
        fbm(d, frequency, 3u, 2.0, 0.5, seed),
        fbm(d, frequency, 3u, 2.0, 0.5, seed + 16u),
        fbm(d, frequency, 3u, 2.0, 0.5, seed + 32u));
    return normalize(d + warp * scale);
}

fn continent_noise(d: vec3<f32>) -> f32 {
    let warped = warp_direction(d, params.continents.z, params.continents.w, params.seeds.y);
    return fbm(warped, params.continents.y, u32(params.octaves.x), params.octaves.y, params.octaves.z,
        params.seeds.x);
}

fn area_weight(i: u32, j: u32, n: u32) -> u32 {
    let u = -1.0 + (2.0 * f32(i) + 1.0) / f32(n);
    let v = -1.0 + (2.0 * f32(j) + 1.0) / f32(n);
    return u32(round(1024.0 / pow(1.0 + u * u + v * v, 1.5)));
}

fn histogram_bin(r: f32) -> u32 {
    return min(u32(max(floor((r + 1.0) * 0.5 * f32(HISTOGRAM_BINS)), 0.0)), HISTOGRAM_BINS - 1u);
}

fn tangent_frame(d: vec3<f32>) -> mat2x3<f32> {
    let pole = params.pole.xyz;
    var east = cross(pole, d);
    if dot(east, east) < 1.0e-12 {
        // glam's any_orthonormal_vector (Duff et al.), as the CPU oracle uses.
        let sign = select(-1.0, 1.0, d.z >= 0.0);
        let a = -1.0 / (sign + d.z);
        east = vec3<f32>(d.x * d.y * a, sign + d.y * d.y * a, -d.y);
    } else {
        east = normalize(east);
    }
    return mat2x3<f32>(east, cross(d, east));
}

// ---------------------------------------------------------------- tectonics

fn bump(x: f32) -> f32 {
    if abs(x) < 1.0 {
        let y = 1.0 - x * x;
        return y * y;
    }
    return 0.0;
}

struct Profile {
    dh: f32,
    orogenic: f32,
    volcanic: f32,
    kind: u32,
}

// Boundary profiles for own plate `i`, neighbour `j` at distance `delta`
// (`tectonics::profiles`).
fn profiles(i: u32, j: u32, a: Plate, b: Plate, delta: f32, conv: f32, div: f32, trans: f32) -> Profile {
    var out: Profile;
    out.dh = 0.0;
    out.orogenic = 0.0;
    out.volcanic = 0.0;
    let ca = a.spin.w > 0.5;
    let cb = b.spin.w > 0.5;
    let arc = bump((delta - params.widths.z) / params.widths.y);
    let trench = -params.heights.z * conv * bump(delta / params.widths.w);
    if ca && cb {
        let belt = params.heights.x * conv * bump(delta / params.widths.x);
        out.dh += belt;
        out.orogenic += belt;
    } else if ca {
        out.dh += params.heights.y * conv * arc + trench;
        out.orogenic += params.heights.y * conv * arc;
        out.volcanic = conv * arc;
    } else if cb {
        out.dh += trench;
    } else {
        out.dh += trench;
        if i > j {
            out.dh += params.heights.y * conv * arc;
            out.orogenic += params.heights.y * conv * arc;
            out.volcanic = conv * arc;
        }
    }
    if ca && cb {
        out.dh -= params.heights2.x * div * bump(delta / params.widths2.y);
    } else if !ca && !cb {
        out.dh += params.heights.w * div * exp(-delta / params.widths2.x);
    }
    let transform = params.heights2.y * trans * bump(delta / params.widths2.z);
    out.dh += transform;
    out.orogenic += transform;
    if max(max(conv, div), trans) < 0.25 {
        out.kind = 0u;
    } else if conv >= div && conv >= trans {
        if ca && cb {
            out.kind = 1u;
        } else if !ca && !cb {
            out.kind = 3u;
        } else {
            out.kind = 2u;
        }
    } else if div >= trans {
        out.kind = select(4u, 5u, ca && cb);
    } else {
        out.kind = 6u;
    }
    return out;
}

// Boundary distance to plate `j` from own plate `i` at warped direction `w`
// and the tangent difference of their seeds.
fn boundary_delta(w: vec3<f32>, a: Plate, b: Plate) -> vec4<f32> {
    let dc = a.centre.xyz - b.centre.xyz;
    let along = dot(w, dc);
    let f = along + (a.centre.w - b.centre.w);
    let tangent = dc - w * along;
    return vec4<f32>(tangent, params.continents.x * f / max(length(tangent), 1.0e-12));
}

// Plates, boundary distance and profiles, crust, hardness (M2 design §1).
// o0 crust, o1 dh, o2 uplift, o3 hardness, o4 boundary distance (m),
// o5 boundary_coord (i32), o6 aux1 (plate, class, volcanic).
@compute @workgroup_size(256)
fn tectonics(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    if !has(STAGE_TECTONICS) {
        write_f(o(0u), t.k, 0.0);
        write_f(o(1u), t.k, 0.0);
        write_f(o(2u), t.k, 0.0);
        write_f(o(3u), t.k, 0.5);
        write_f(o(4u), t.k, 0.0);
        fields[o(5u) + t.k] = 0u;
        fields[o(6u) + t.k] = 0u;
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    var w = d;
    if params.plates.y > 0.0 {
        w = warp_direction(d, params.plates.x, params.plates.y, params.seeds.w);
    }
    let count = params.grid.z;
    var own = 0u;
    var best = -1.0e30;
    for (var k = 0u; k < count; k = k + 1u) {
        let s = dot(w, plates[k].centre.xyz) + plates[k].centre.w;
        if s > best {
            best = s;
            own = k;
        }
    }
    let a = plates[own];
    var delta_min = 1.0e30;
    for (var j = 0u; j < count; j = j + 1u) {
        if j != own {
            delta_min = min(delta_min, boundary_delta(w, a, plates[j]).w);
        }
    }
    let tau = max(min(params.plates.z, delta_min), 1.0);
    var total = 0.0;
    var dh = 0.0;
    var orogenic = 0.0;
    var volcanic = 0.0;
    var coord = 0.0;
    var crust = 0.0;
    var hardness = 0.0;
    var nearest = 1.0e30;
    var kind = 0u;
    let crust_a = select(-1.0, 1.0, a.spin.w > 0.5);
    for (var j = 0u; j < count; j = j + 1u) {
        if j == own {
            continue;
        }
        let b = plates[j];
        let bd = boundary_delta(w, a, b);
        let delta = bd.w;
        let weight = exp(-(delta - delta_min) / tau);
        let normal = -bd.xyz / max(length(bd.xyz), 1.0e-12);
        let v = cross(a.spin.xyz - b.spin.xyz, w);
        let c = dot(v, normal);
        let shear = abs(dot(v, cross(w, normal)));
        let conv = smoothstep(0.05, 0.4, c);
        let div = smoothstep(0.05, 0.4, -c);
        let trans = smoothstep(0.05, 0.4, shear) * (1.0 - conv - div);
        let profile = profiles(own, j, a, b, delta, conv, div, trans);
        let blend = 0.5 * (1.0 - smoothstep(0.0, params.widths2.w, delta));
        let crust_b = select(-1.0, 1.0, b.spin.w > 0.5);
        total += weight;
        dh += weight * profile.dh;
        orogenic += weight * profile.orogenic;
        volcanic += weight * profile.volcanic;
        coord += weight * select(-1.0, 1.0, j > own);
        crust += weight * (crust_a + (crust_b - crust_a) * blend);
        hardness += weight * (a.props.x + (b.props.x - a.props.x) * blend);
        if delta < nearest {
            nearest = delta;
            kind = profile.kind;
        }
    }
    total = max(total, 1.0e-30);
    let rough = params.heights2.z * fbm(d, params.heights2.w, 4u, 2.0, 0.5, params.seeds.w + 64u);
    let oro = orogenic / total;
    let uplift = clamp(oro * (1.0 + rough) / max(params.heights.x, 1.0), 0.0, 1.0);
    let volc = clamp(volcanic / total, 0.0, 1.0);
    let bc = clamp(delta_min * coord / total, -params.plates.w, params.plates.w);
    let noise = fbm(d, params.hardness.y, 3u, 2.0, 0.5, params.seeds2.x);
    let hard = clamp(max(hardness / total, 0.8 * volc) + params.hardness.x * noise, 0.0, 1.0);
    write_f(o(0u), t.k, crust / total);
    write_f(o(1u), t.k, dh / total + rough * oro);
    write_f(o(2u), t.k, uplift);
    write_f(o(3u), t.k, hard);
    write_f(o(4u), t.k, delta_min);
    fields[o(5u) + t.k] = bitcast<u32>(i32(floor(bc * BOUNDARY_UNITS_PER_M + 0.5)));
    fields[o(6u) + t.k] = own | (kind << 8u) | ((pack4x8unorm(vec4<f32>(volc, 0.0, 0.0, 0.0)) & 0xffu) << 16u);
}

// ---------------------------------------------------------------- continents

// o0 raw noise (out), o1 crust.
@compute @workgroup_size(256)
fn continents(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    var noise = 0.0;
    if has(STAGE_CONTINENTS) {
        noise = continent_noise(d);
    }
    let beta = params.octaves.w;
    let r = (1.0 - beta) * noise + beta * 0.5 * read_f(o(1u), t.k);
    write_f(o(0u), t.k, r);
    atomicAdd(&stats[histogram_bin(r)], area_weight(t.i, t.j, pass_info.n));
}

// Percentile of the area-weighted histogram at `base` (linear within the
// crossing bin), in [-1, 1].
fn percentile(base: u32, coverage: f32) -> f32 {
    var total = 0u;
    for (var b = 0u; b < HISTOGRAM_BINS; b = b + 1u) {
        total += atomicLoad(&stats[base + b]);
    }
    let target_weight = coverage * f32(total);
    var below = 0u;
    for (var b = 0u; b < HISTOGRAM_BINS; b = b + 1u) {
        let count = atomicLoad(&stats[base + b]);
        let next = below + count;
        if f32(next) >= target_weight && count > 0u {
            let fraction = (target_weight - f32(below)) / f32(count);
            return -1.0 + 2.0 * (f32(b) + fraction) / f32(HISTOGRAM_BINS);
        }
        below = next;
    }
    return 1.0;
}

@compute @workgroup_size(1)
fn sea_level() {
    var s = -1.0;
    if has(STAGE_SEA_LEVEL) {
        s = percentile(0u, params.relief.x);
    }
    atomicStore(&stats[STATS_SEA_LEVEL], bitcast<u32>(s));
    atomicStore(&stats[STATS_LOW], bitcast<u32>(percentile(0u, 0.001)));
    atomicStore(&stats[STATS_HIGH], bitcast<u32>(percentile(0u, 0.999)));
}

fn deep(t: f32) -> f32 {
    return 1.0 - pow(1.0 - clamp(t, 0.0, 1.0), 3.0);
}

// Elevation without the shelf (applied after the second histogram).
fn shape_elevation(r: f32, s: f32, low: f32, high: f32) -> f32 {
    if r >= s {
        let t = clamp((r - s) / max(high - s, 1.0e-9), 0.0, 1.0);
        return params.relief.y * pow(t, params.relief.z);
    }
    let t = clamp((s - r) / max(s - low, 1.0e-9), 0.0, 1.0);
    return -params.relief.w * deep(t);
}

// o0 raw, o1 dh, o2 pre-erosion elevation (out), o3 max(h, 0) (out).
@compute @workgroup_size(256)
fn shape(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let s = bitcast<f32>(atomicLoad(&stats[STATS_SEA_LEVEL]));
    let low = bitcast<f32>(atomicLoad(&stats[STATS_LOW]));
    let high = bitcast<f32>(atomicLoad(&stats[STATS_HIGH]));
    let h = shape_elevation(read_f(o(0u), t.k), s, low, high) + read_f(o(1u), t.k);
    write_f(o(2u), t.k, h);
    write_f(o(3u), t.k, max(h, 0.0));
}

// ---------------------------------------------------------------- blur

// The ocean-blur kernel: the texel and four bilinear samples `scalars.x`
// radians east, west, north and south, averaged. o0 source, o1 destination.
@compute @workgroup_size(256)
fn blur(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    let frame = tangent_frame(d);
    let step = pass_info.scalars.x;
    let src = o(0u);
    var sum = read_f(src, t.k);
    sum += cube_sample(src, normalize(d + frame[0] * step), false);
    sum += cube_sample(src, normalize(d - frame[0] * step), false);
    sum += cube_sample(src, normalize(d + frame[1] * step), false);
    sum += cube_sample(src, normalize(d - frame[1] * step), false);
    write_f(o(1u), t.k, sum / 5.0);
}

// Slope of the smoothed relief (rain shadow) and the ocean mask.
// o0 smoothed relief, o1 pre-erosion elevation, o2/o3 slope east/north (out),
// o4 ocean mask (out); `scalars.x` the smoothing step.
@compute @workgroup_size(256)
fn slope(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    write_f(o(4u), t.k, select(0.0, 1.0, read_f(o(1u), t.k) < 0.0));
    var g = vec2<f32>(0.0);
    if has(STAGE_RAIN_SHADOW) {
        let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
        let frame = tangent_frame(d);
        let step = pass_info.scalars.x;
        let src = o(0u);
        let scale = 1.0 / (2.0 * step * params.continents.x);
        g.x = (cube_sample(src, normalize(d + frame[0] * step), false)
            - cube_sample(src, normalize(d - frame[0] * step), false)) * scale;
        g.y = (cube_sample(src, normalize(d + frame[1] * step), false)
            - cube_sample(src, normalize(d - frame[1] * step), false)) * scale;
    }
    write_f(o(2u), t.k, g.x);
    write_f(o(3u), t.k, g.y);
}

// ---------------------------------------------------------------- climate

fn insolation(x: f32, tilt: f32) -> f32 {
    let p2_tilt = 0.5 * (3.0 * cos(tilt) * cos(tilt) - 1.0);
    let p2_x = 0.5 * (3.0 * x * x - 1.0);
    return 1.0 - 0.625 * p2_tilt * p2_x;
}

fn base_temperature(x: f32) -> f32 {
    let tilt = params.climate.z;
    let equator = insolation(0.0, tilt);
    let pole = insolation(1.0, tilt);
    let span = equator - pole;
    var t = 0.5;
    if abs(span) >= 1.0e-9 {
        t = (insolation(x, tilt) - pole) / span;
    }
    return params.climate.y + (params.climate.x - params.climate.y) * t;
}

const HALF_PI: f32 = 1.5707963267948966;
const PI: f32 = 3.141592653589793;

// Rain scale by the circulation cells (`tier_a::rain_modulation`): wetter where
// the cells converge (cos(2 · cells · |lat|) > 0), drier where they diverge.
fn rain_modulation(x: f32) -> f32 {
    let latitude = abs(asin(clamp(x, -1.0, 1.0)));
    return 1.0 + params.moisture.w * cos(2.0 * params.wind.x * latitude);
}

fn wind_at(x: f32) -> vec2<f32> {
    let latitude = asin(clamp(x, -1.0, 1.0));
    let a = abs(latitude) / HALF_PI;
    let s = sin(params.wind.x * PI * a);
    return vec2<f32>(-s, -sign(latitude) * params.wind.y * s);
}

fn temperature_at(d: vec3<f32>, h: f32, ocean: f32) -> f32 {
    let middle = 0.5 * (params.climate.x + params.climate.y);
    if !has(STAGE_TEMPERATURE) {
        return middle;
    }
    let base = base_temperature(dot(d, params.pole.xyz)) - params.climate.w * max(h, 0.0) / 1000.0;
    let moderated = middle + (base - middle) * (1.0 - params.moderation.x * ocean);
    let noise = params.moderation.z * fbm(d, params.moderation.w, 3u, 2.0, 0.5, params.seeds.z);
    return moderated + noise;
}

// Temperature and wind bands. o0 elevation, o1 blurred ocean mask,
// o2 temperature, o3/o4 wind east/north (out).
@compute @workgroup_size(256)
fn climate(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    write_f(o(2u), t.k, temperature_at(d, read_f(o(0u), t.k), read_f(o(1u), t.k)));
    var w = vec2<f32>(0.0);
    if has(STAGE_WIND) {
        w = wind_at(dot(d, params.pole.xyz));
    }
    write_f(o(3u), t.k, w.x);
    write_f(o(4u), t.k, w.y);
}

// Wind deflected around the smoothed relief (`climate::deflect`), in place.
// o0/o1 wind east/north, o2/o3 slope east/north.
@compute @workgroup_size(256)
fn wind_deflect(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let w = vec2<f32>(read_f(o(0u), t.k), read_f(o(1u), t.k));
    let g = vec2<f32>(read_f(o(2u), t.k), read_f(o(3u), t.k));
    let length_g = length(g);
    if length_g <= 0.0 {
        return;
    }
    let u = g / length_g;
    let s = length_g / (length_g + params.shadow.z);
    let along = max(dot(w, u), 0.0) * params.shadow.y * s;
    let deflected = w - along * u;
    let speed = length(w) * (1.0 - params.shadow.w * s);
    let size = length(deflected);
    var out = vec2<f32>(0.0);
    if size > 1.0e-12 {
        out = deflected * (speed / size);
    }
    write_f(o(0u), t.k, out.x);
    write_f(o(1u), t.k, out.y);
}

// Vector blur of the wind: each sample is a 3-D tangent vector in its own
// east/north frame, averaged and projected onto the texel's frame.
// o0/o1 source east/north, o2/o3 destination; `scalars.x` the step.
@compute @workgroup_size(256)
fn wind_blur(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    let frame = tangent_frame(d);
    let step = pass_info.scalars.x;
    var sum = frame[0] * read_f(o(0u), t.k) + frame[1] * read_f(o(1u), t.k);
    var offsets = array<vec3<f32>, 4>(frame[0], -frame[0], frame[1], -frame[1]);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let s = normalize(d + offsets[k] * step);
        let f = tangent_frame(s);
        sum += f[0] * cube_sample(o(0u), s, false) + f[1] * cube_sample(o(1u), s, false);
    }
    let v = sum / 5.0;
    write_f(o(2u), t.k, dot(v, frame[0]));
    write_f(o(3u), t.k, dot(v, frame[1]));
}

// ---------------------------------------------------------------- moisture

fn evaporation_factor(t_c: f32) -> f32 {
    return clamp((t_c + 10.0) / 40.0, 0.1, 1.0);
}

// Rain-out share per step (`climate::rain_rate`).
fn rain_rate(x: f32, uphill: f32) -> f32 {
    let base = params.wind.w * rain_modulation(x);
    if !has(STAGE_RAIN_SHADOW) {
        return base;
    }
    let reference = params.orographic.z;
    let orographic = params.orographic.x * max(uphill, 0.0) / reference;
    let lee = min(params.orographic.y * max(-uphill, 0.0) / reference, 1.0);
    return min((base + orographic) * (1.0 - lee), 1.0);
}

// One semi-Lagrangian advection step. o0 carried (source), o1 carried (out),
// o2/o3 wind east/north, o4 elevation, o5 temperature, o6 precipitation
// (accumulated), o7/o8 slope east/north.
@compute @workgroup_size(256)
fn moisture_step(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    let frame = tangent_frame(d);
    let w = vec2<f32>(read_f(o(2u), t.k), read_f(o(3u), t.k));
    let flow = frame[0] * w.x + frame[1] * w.y;
    let step = params.moisture.x;
    let src = o(0u);
    let source = d - flow * step;
    var upwind = cube_sample(src, normalize(source), false);
    let spread = params.moisture.y;
    if spread > 0.0 {
        let side = 0.5 * step;
        let lateral = cube_sample(src, normalize(source + frame[0] * side), false)
            + cube_sample(src, normalize(source - frame[0] * side), false)
            + cube_sample(src, normalize(source + frame[1] * side), false)
            + cube_sample(src, normalize(source - frame[1] * side), false);
        upwind = (1.0 - spread) * upwind + spread * 0.25 * lateral;
    }
    var evaporate = 0.0;
    if read_f(o(4u), t.k) < 0.0 {
        evaporate = params.wind.z * evaporation_factor(read_f(o(5u), t.k));
    }
    let uphill = w.x * read_f(o(7u), t.k) + w.y * read_f(o(8u), t.k);
    let rain = upwind * rain_rate(dot(d, params.pole.xyz), uphill);
    write_f(o(1u), t.k, upwind + evaporate - rain);
    write_f(o(6u), t.k, read_f(o(6u), t.k) + rain);
}

// o0 precipitation, o1 moisture (out).
@compute @workgroup_size(256)
fn moisture_finish(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    var m = 0.5;
    if has(STAGE_MOISTURE) {
        // Over the sea too (see the CPU oracle): no wet rim bleeds into coasts.
        m = 1.0 - exp(-read_f(o(0u), t.k) / params.moisture.z);
    }
    write_f(o(1u), t.k, m);
}

// ---------------------------------------------------------------- erosion
// Neighbours of the 8-neighbourhood cross cube faces through the integer
// face-edge table (`tier_a::erosion::FACE_EDGES`): per face and edge
// (0: i < 0, 1: i ≥ n, 2: j < 0, 3: j ≥ n) the neighbour face, the edge it
// enters by and whether the along-edge coordinate reverses.

var<private> FACE_EDGES: array<vec3<u32>, 24> = array<vec3<u32>, 24>(
    vec3<u32>(4u, 1u, 0u), vec3<u32>(5u, 0u, 0u), vec3<u32>(3u, 1u, 1u), vec3<u32>(2u, 1u, 0u),
    vec3<u32>(5u, 1u, 0u), vec3<u32>(4u, 0u, 0u), vec3<u32>(3u, 0u, 0u), vec3<u32>(2u, 0u, 1u),
    vec3<u32>(1u, 3u, 1u), vec3<u32>(0u, 3u, 0u), vec3<u32>(4u, 3u, 0u), vec3<u32>(5u, 3u, 1u),
    vec3<u32>(1u, 2u, 0u), vec3<u32>(0u, 2u, 1u), vec3<u32>(5u, 2u, 1u), vec3<u32>(4u, 2u, 0u),
    vec3<u32>(1u, 1u, 0u), vec3<u32>(0u, 0u, 0u), vec3<u32>(3u, 3u, 0u), vec3<u32>(2u, 2u, 0u),
    vec3<u32>(0u, 1u, 0u), vec3<u32>(1u, 0u, 0u), vec3<u32>(3u, 2u, 1u), vec3<u32>(2u, 3u, 1u));

var<private> NEIGHBOUR_STEPS: array<vec2<i32>, 8> = array<vec2<i32>, 8>(
    vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1),
    vec2<i32>(1, 1), vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1));

// Texel index reached from (face, i, j) by `step` on n-cell faces; the
// diagonal missing at a cube corner returns the texel itself.
fn face_step(face: u32, i: i32, j: i32, step: vec2<i32>, n: i32) -> u32 {
    let a = i + step.x;
    let b = j + step.y;
    let in_a = a >= 0 && a < n;
    let in_b = b >= 0 && b < n;
    let un = u32(n);
    if in_a && in_b {
        return (face * un + u32(b)) * un + u32(a);
    }
    if !in_a && !in_b {
        return (face * un + u32(j)) * un + u32(i);
    }
    var edge: u32;
    var along: i32;
    if !in_a {
        edge = select(1u, 0u, a < 0);
        along = b;
    } else {
        edge = select(3u, 2u, b < 0);
        along = a;
    }
    let entry = FACE_EDGES[4u * face + edge];
    var t = along;
    if entry.z == 1u {
        t = n - 1 - along;
    }
    var x: i32;
    var y: i32;
    switch entry.y {
        case 0u: { x = 0; y = t; }
        case 1u: { x = n - 1; y = t; }
        case 2u: { x = t; y = 0; }
        default: { x = t; y = n - 1; }
    }
    return (entry.x * un + u32(y)) * un + u32(x);
}

fn texel_dir(k: u32, n: u32) -> vec3<f32> {
    return cube_texel_direction(k / (n * n), k % n, (k / n) % n, n);
}

// Texel area (km²): gnomonic solid angle of a 2/n square times R².
fn texel_area(i: u32, j: u32, n: u32) -> f32 {
    let u = -1.0 + (2.0 * f32(i) + 1.0) / f32(n);
    let v = -1.0 + (2.0 * f32(j) + 1.0) / f32(n);
    let side = 2.0 * params.continents.x / f32(n);
    return side * side / pow(1.0 + u * u + v * v, 1.5) * 1.0e-6;
}

struct Neighbour {
    k: u32,
    distance: f32,
}

fn neighbour(t: Texel, slot: u32, d: vec3<f32>) -> Neighbour {
    let n = pass_info.n;
    var out: Neighbour;
    out.k = face_step(t.face, i32(t.i), i32(t.j), NEIGHBOUR_STEPS[slot], i32(n));
    out.distance = 0.0;
    if out.k != t.k {
        out.distance = length(d - texel_dir(out.k, n)) * params.continents.x;
    }
    return out;
}

// 2×2 reduction within each face to this pass's grid: mode 0 average, 1
// maximum. o0 fine source, o1 coarse destination.
@compute @workgroup_size(256)
fn reduce(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let fine = 2u * pass_info.n;
    let a = read_f(o(0u), cube_index(t.face, 2u * t.i, 2u * t.j, fine));
    let b = read_f(o(0u), cube_index(t.face, 2u * t.i + 1u, 2u * t.j, fine));
    let c = read_f(o(0u), cube_index(t.face, 2u * t.i, 2u * t.j + 1u, fine));
    let d = read_f(o(0u), cube_index(t.face, 2u * t.i + 1u, 2u * t.j + 1u, fine));
    if pass_info.mode == 1u {
        write_f(o(1u), t.k, max(max(a, b), max(c, d)));
    } else {
        write_f(o(1u), t.k, 0.25 * (a + b + c + d));
    }
}

// Per-texel set-up: mode 0 copy o0 → o1; mode 1 zero o1; mode 2 water
// surface seed (ocean: h, land: unfilled) of relief o0 into o1; mode 3
// difference o0 − o2 into o1.
@compute @workgroup_size(256)
fn erode_set(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    switch pass_info.mode {
        case 0u: { write_f(o(1u), t.k, read_f(o(0u), t.k)); }
        case 1u: { write_f(o(1u), t.k, 0.0); }
        case 2u: {
            let h = read_f(o(0u), t.k);
            write_f(o(1u), t.k, select(UNFILLED_M, h, h < 0.0));
        }
        default: { write_f(o(1u), t.k, read_f(o(0u), t.k) - read_f(o(2u), t.k)); }
    }
}

// Level transition: relief = base + bicubic upsampled eroded difference,
// discharge, sediment flux and deposit bilinear (non-negative). Sampled on
// the coarse grid (`sample_n`). o0 base, o1 difference, o2 discharge,
// o3 sediment flux, o4 deposit (coarse); o5 relief, o6 discharge,
// o7 sediment flux, o8 deposit (out).
@compute @workgroup_size(256)
fn erode_lift(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    write_f(o(5u), t.k, read_f(o(0u), t.k) + cube_sample(o(1u), d, true));
    write_f(o(6u), t.k, max(cube_sample(o(2u), d, false), 0.0));
    write_f(o(7u), t.k, max(cube_sample(o(3u), d, false), 0.0));
    write_f(o(8u), t.k, max(cube_sample(o(4u), d, false), 0.0));
}

// Fill pyramid refinement: water surface = max(relief, parent surface).
// o0 relief, o1 parent surface (half resolution), o2 surface (out).
@compute @workgroup_size(256)
fn fill_refine(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let h = read_f(o(0u), t.k);
    let half = pass_info.n / 2u;
    let parent = read_f(o(1u), cube_index(t.face, t.i / 2u, t.j / 2u, half));
    write_f(o(2u), t.k, select(max(h, parent), h, h < 0.0));
}

// One fill step (Planchon–Darboux, Jacobi): max(h, min(s + slack,
// min_m s_m + step)). o0 relief, o1 candidate surfaces, o2 surface (out);
// `scalars` (step, slack).
@compute @workgroup_size(256)
fn fill_step(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let h = read_f(o(0u), t.k);
    if h < 0.0 {
        write_f(o(2u), t.k, h);
        return;
    }
    let n = pass_info.n;
    var lowest = read_f(o(1u), t.k) + pass_info.scalars.y;
    for (var slot = 0u; slot < 8u; slot = slot + 1u) {
        let m = face_step(t.face, i32(t.i), i32(t.j), NEIGHBOUR_STEPS[slot], i32(n));
        if m != t.k {
            lowest = min(lowest, read_f(o(1u), m) + pass_info.scalars.x);
        }
    }
    write_f(o(2u), t.k, max(h, lowest));
}

// Route on the water surface: Σ s^p and the MFD slope. o0 surface;
// o1 Σ s^p, o2 slope (out).
@compute @workgroup_size(256)
fn erode_route(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    let w = read_f(o(0u), t.k);
    var total = 0.0;
    var weighted = 0.0;
    for (var slot = 0u; slot < 8u; slot = slot + 1u) {
        let nb = neighbour(t, slot, d);
        if nb.distance <= 0.0 {
            continue;
        }
        let s = (w - read_f(o(0u), nb.k)) / nb.distance;
        if s > 0.0 {
            let sp = pow(s, params.erosion2.w);
            total += sp;
            weighted += sp * s;
        }
    }
    write_f(o(1u), t.k, total);
    write_f(o(2u), t.k, select(0.0, weighted / total, total > 0.0));
}

// One Jacobi step of discharge and sediment flux; the ocean is a sink.
// o0 surface, o1 Σ s^p, o2 discharge (previous), o3 sediment flux out,
// o4 precipitation; o5 discharge, o6 sediment flux in (out).
@compute @workgroup_size(256)
fn erode_accumulate(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let n = pass_info.n;
    let d = cube_texel_direction(t.face, t.i, t.j, n);
    let w = read_f(o(0u), t.k);
    var q = read_f(o(4u), t.k) * texel_area(t.i, t.j, n);
    var qs = 0.0;
    for (var slot = 0u; slot < 8u; slot = slot + 1u) {
        let nb = neighbour(t, slot, d);
        if nb.distance <= 0.0 {
            continue;
        }
        let wm = read_f(o(0u), nb.k);
        let total = read_f(o(1u), nb.k);
        if wm < 0.0 || total <= 0.0 {
            continue;
        }
        let s = (wm - w) / nb.distance;
        if s > 0.0 {
            let share = pow(s, params.erosion2.w) / total;
            q += read_f(o(2u), nb.k) * share;
            qs += read_f(o(3u), nb.k) * share;
        }
    }
    write_f(o(5u), t.k, q);
    write_f(o(6u), t.k, qs);
}

// Incision, deposition, thermal talus and uplift (`erosion::iterate`).
// o0 relief, o1 surface, o2 discharge, o3 sediment flux in, o4 slope,
// o5 uplift, o6 hardness; o7 relief, o8 sediment flux out (out),
// o9 deposit (accumulated), o10 candidate surface (out).
@compute @workgroup_size(256)
fn erode_update(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let n = pass_info.n;
    let d = cube_texel_direction(t.face, t.i, t.j, n);
    let h = read_f(o(0u), t.k);
    let area = texel_area(t.i, t.j, n);
    var low = 3.0e38;
    var up = 3.0e38;
    var thermal = 0.0;
    for (var slot = 0u; slot < 8u; slot = slot + 1u) {
        let nb = neighbour(t, slot, d);
        if nb.distance <= 0.0 {
            continue;
        }
        let hm = read_f(o(0u), nb.k);
        low = min(low, hm);
        if hm > h {
            up = min(up, hm);
        }
        let talus = params.erosion.z * nb.distance;
        let share = params.erosion3.x * min(area, texel_area(nb.k % n, (nb.k / n) % n, n)) / area;
        let drop = h - hm;
        if drop > talus {
            thermal -= share * (drop - talus);
        } else if -drop > talus {
            thermal += share * (-drop - talus);
        }
    }
    let q = read_f(o(2u), t.k);
    let qs_in = read_f(o(3u), t.k);
    let w = read_f(o(1u), t.k);
    var erode = 0.0;
    var deposit = 0.0;
    var qs_out = 0.0;
    if h >= 0.0 {
        let slope = read_f(o(4u), t.k);
        var power = 0.0;
        if q > 0.0 && slope > 0.0 {
            power = pow(q, params.erosion2.y) * pow(slope, params.erosion2.z);
        }
        let rate = params.erosion.x * (1.0 - 0.8 * read_f(o(6u), t.k)) * power;
        erode = min(rate, max(h - low - INCISION_CLEARANCE_M, 0.0));
        let capacity = params.erosion2.x * params.erosion.x * power * q;
        var limit = w - h;
        if up < 3.0e38 {
            limit = max(limit, 0.5 * (up - h));
        }
        deposit = min(params.erosion.w * max(qs_in - capacity, 0.0) / area, limit);
        qs_out = max(qs_in - deposit * area, 0.0) + erode * area;
    } else {
        deposit = min(params.erosion.w * qs_in / area, max(-h - PIT_FILL_M, 0.0));
    }
    let h_new = h - erode + deposit + thermal + params.erosion.y * read_f(o(5u), t.k);
    write_f(o(7u), t.k, h_new);
    write_f(o(8u), t.k, qs_out);
    write_f(o(9u), t.k, read_f(o(9u), t.k) + deposit);
    write_f(o(10u), t.k, h_new + max(w - h, 0.0));
}

// Last level: lakes are filled to their surface (counted as deposit).
// o0 relief, o1 surface, o2 deposit (in place), o3 eroded elevation (out).
@compute @workgroup_size(256)
fn erode_finish(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let h = read_f(o(0u), t.k);
    let w = read_f(o(1u), t.k);
    write_f(o(2u), t.k, read_f(o(2u), t.k) + max(w - h, 0.0));
    write_f(o(3u), t.k, w);
}

// ---------------------------------------------------------------- re-zero

fn elevation_bin(h: f32) -> u32 {
    return min(u32(max(floor((h / params.shelf.z + 1.0) * 0.5 * f32(HISTOGRAM_BINS)), 0.0)),
        HISTOGRAM_BINS - 1u);
}

// Second histogram, in metres over ±height bound. o0 eroded elevation.
@compute @workgroup_size(256)
fn histogram_2(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let bin = elevation_bin(read_f(o(0u), t.k));
    atomicAdd(&stats[STATS_HISTOGRAM_2 + bin], area_weight(t.i, t.j, pass_info.n));
}

@compute @workgroup_size(1)
fn sea_level_2() {
    var s = 0.0;
    if has(STAGE_SEA_LEVEL) {
        s = percentile(STATS_HISTOGRAM_2, params.relief.x) * params.shelf.z;
    }
    atomicStore(&stats[STATS_SEA_LEVEL_2], bitcast<u32>(s));
}

fn shelf_depth(t: f32) -> f32 {
    let ocean_depth = params.relief.w;
    let shelf_depth = params.shelf.x;
    let shelf_fraction = params.shelf.y;
    if shelf_fraction <= 0.0 {
        return ocean_depth * deep(t);
    }
    if t < shelf_fraction {
        return shelf_depth * t / shelf_fraction;
    }
    let u = (t - shelf_fraction) / (1.0 - shelf_fraction);
    return shelf_depth + max(ocean_depth - shelf_depth, 0.0) * deep(u);
}

// The shelf as a monotone remap of an unshelved depth (`tier_a::shelf_remap`).
fn shelf_remap(depth: f32) -> f32 {
    let od = params.relief.w;
    if depth <= 0.0 || depth >= od {
        return depth;
    }
    let t = 1.0 - pow(1.0 - depth / od, 1.0 / 3.0);
    return shelf_depth(t);
}

// Re-zero, shelf and clamp; final temperature; aux0 packing.
// o0 eroded elevation, o1 blurred ocean mask, o2 uplift, o3 hardness,
// o4 deposit, o5 discharge; o6 elevation, o7 temperature, o8 aux0 (out).
@compute @workgroup_size(256)
fn finish_shape(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let s2 = bitcast<f32>(atomicLoad(&stats[STATS_SEA_LEVEL_2]));
    let bound = params.shelf.z;
    var z = read_f(o(0u), t.k) - s2;
    if z < 0.0 && has(STAGE_SHELF) {
        z = -shelf_remap(-z);
    }
    z = clamp(z, -bound, bound);
    write_f(o(6u), t.k, z);
    let d = cube_texel_direction(t.face, t.i, t.j, pass_info.n);
    if has(STAGE_TEMPERATURE) {
        write_f(o(7u), t.k, temperature_at(d, z, read_f(o(1u), t.k)));
    }
    var sediment = 0.0;
    var flow = 0.0;
    if has(STAGE_EROSION) {
        sediment = 1.0 - exp(-read_f(o(4u), t.k) / params.erosion3.y);
        flow = log(1.0 + read_f(o(5u), t.k)) * params.shelf.w;
    }
    fields[o(8u) + t.k] = pack4x8unorm(vec4<f32>(read_f(o(2u), t.k), read_f(o(3u), t.k), sediment, flow));
}

// Landform weights (run 8; M2 Shape): the set's rule bytecode
// (`landform::expr`, binding 5) over each texel's stored results, written as
// four unorm8 weights. Mirrors `tier_a::landform_rule_fields` and
// `expr::evaluate_set` in f32. o0 elevation, o1 temperature, o2 moisture,
// o3 boundary_coord, o4 aux0, o5 aux1, o6/o7 smoothed slope east/north,
// o8 weights (out).
var<private> lw_fields: array<f32, 12>;

fn lw_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e0 == e1 {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn lw_bump(x: f32, a: f32, b: f32) -> f32 {
    if b <= a {
        return 0.0;
    }
    let t = (2.0 * x - a - b) / (b - a);
    if abs(t) < 1.0 {
        let s = 1.0 - t * t;
        return s * s;
    }
    return 0.0;
}

// One rule at word `code` of the set (validated on the CPU).
fn lw_rule(code: u32) -> f32 {
    var stack: array<f32, 16>;
    var top = 0u;
    var at = code;
    for (var i = 0u; i < 64u; i = i + 1u) {
        let word = landform_rules[at];
        at = at + 1u;
        let opcode = word & 255u;
        if opcode == 0u {
            return stack[0];
        }
        if opcode == 1u {
            stack[top] = bitcast<f32>(landform_rules[at]);
            at = at + 1u;
            top = top + 1u;
            continue;
        }
        if opcode == 2u {
            stack[top] = lw_fields[min(word >> 8u, 11u)];
            top = top + 1u;
            continue;
        }
        if opcode == 7u || opcode == 10u {
            let a = stack[top - 1u];
            stack[top - 1u] = select(abs(a), -a, opcode == 7u);
            continue;
        }
        if opcode >= 12u {
            let a = stack[top - 3u];
            let b = stack[top - 2u];
            let c = stack[top - 1u];
            var r = 0.0;
            switch opcode {
                case 12u: { r = min(max(a, b), c); }
                case 13u: { r = a + (b - a) * c; }
                case 14u: { r = lw_smoothstep(a, b, c); }
                default: { r = lw_bump(a, b, c); }
            }
            top = top - 2u;
            stack[top - 1u] = r;
            continue;
        }
        let a = stack[top - 2u];
        let b = stack[top - 1u];
        var r = 0.0;
        switch opcode {
            case 3u: { r = a + b; }
            case 4u: { r = a - b; }
            case 5u: { r = a * b; }
            case 6u: { r = select(0.0, a / b, b != 0.0); }
            case 8u: { r = min(a, b); }
            case 9u: { r = max(a, b); }
            default: { r = select(0.0, pow(a, b), a > 0.0); }
        }
        top = top - 1u;
        stack[top - 1u] = r;
    }
    return 0.0;
}

@compute @workgroup_size(256)
fn landform_weights(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let elevation = read_f(o(0u), t.k);
    let temperature = read_f(o(1u), t.k);
    let m = clamp(read_f(o(2u), t.k), 0.0, 1.0);
    let bc = f32(bitcast<i32>(fields[o(3u) + t.k])) / BOUNDARY_UNITS_PER_M;
    let aux0 = unpack4x8unorm(fields[o(4u) + t.k]);
    let aux1 = unpack4x8unorm(fields[o(5u) + t.k]);
    let slope = length(vec2<f32>(read_f(o(6u), t.k), read_f(o(7u), t.k)));
    let x = clamp((temperature + 15.0) / 20.0, 0.0, 1.0);
    lw_fields[0] = aux0.x;
    lw_fields[1] = aux0.z;
    lw_fields[2] = m;
    lw_fields[3] = 1.0 - m;
    lw_fields[4] = temperature;
    lw_fields[5] = 1.0 - x * x * (3.0 - 2.0 * x);
    lw_fields[6] = aux0.y;
    lw_fields[7] = slope;
    lw_fields[8] = elevation;
    lw_fields[9] = abs(bc);
    lw_fields[10] = aux1.z;
    lw_fields[11] = select(0.0, 1.0, elevation < 0.0);
    // Set header: version, count, flags (bit 0 normalise), fallback, offsets.
    let count = min(landform_rules[1], 4u);
    let fallback = landform_rules[3];
    var w = vec4<f32>(0.0);
    var sum = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        var r = lw_rule(landform_rules[4u + i]);
        if !(r >= 0.0) {
            r = 0.0;
        }
        r = min(r, 1.0);
        w[i] = r;
        sum += r;
    }
    let lift = max(0.0625 - sum, 0.0);
    w[min(fallback, 3u)] += lift;
    if (landform_rules[2] & 1u) != 0u {
        w = w / (sum + lift);
    }
    fields[o(8u) + t.k] = pack4x8unorm(w);
}

// ---------------------------------------------------------------- field mips

// 2×2 reduction to the next mip (this pass's grid) of one field: mode 0 f32
// average, 1 i32 average `(a + b + c + d + 2) >> 2`, 2 per-byte average of
// packed unorm8. o0 fine source, o1 destination.
@compute @workgroup_size(256)
fn field_mip(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let fine = 2u * pass_info.n;
    let src = o(0u);
    let a = fields[src + cube_index(t.face, 2u * t.i, 2u * t.j, fine)];
    let b = fields[src + cube_index(t.face, 2u * t.i + 1u, 2u * t.j, fine)];
    let c = fields[src + cube_index(t.face, 2u * t.i, 2u * t.j + 1u, fine)];
    let d = fields[src + cube_index(t.face, 2u * t.i + 1u, 2u * t.j + 1u, fine)];
    var out: u32;
    switch pass_info.mode {
        case 0u: {
            out = bitcast<u32>(0.25 * (bitcast<f32>(a) + bitcast<f32>(b) + bitcast<f32>(c) + bitcast<f32>(d)));
        }
        case 1u: {
            out = bitcast<u32>((bitcast<i32>(a) + bitcast<i32>(b) + bitcast<i32>(c) + bitcast<i32>(d) + 2) >> 2u);
        }
        default: {
            out = 0u;
            for (var byte = 0u; byte < 4u; byte = byte + 1u) {
                let shift = 8u * byte;
                let sum = ((a >> shift) & 0xffu) + ((b >> shift) & 0xffu) + ((c >> shift) & 0xffu)
                    + ((d >> shift) & 0xffu);
                out |= ((sum + 2u) >> 2u) << shift;
            }
        }
    }
    fields[o(1u) + t.k] = out;
}
