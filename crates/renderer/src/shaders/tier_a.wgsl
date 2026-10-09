// Tier A world-map bake (ASTRUM_TERRAIN_PIPELINE.md §6) on the GPU, the terrain
// authority under ADR 0023. Mirrors crates/world/src/terrain/tier_a.rs (the CPU
// test oracle) stage by stage. Prepended with terrain_noise.wgsl and
// cube_map.wgsl.
//
// All fields of one body live in `fields` as runs of L = 6·n² values:
// 0 raw noise, 1 elevation, 2 temperature, 3 moisture, 4 wind east,
// 5 wind north, 6/7 ocean mask ping-pong, 8/9 carried moisture ping-pong,
// 10 accumulated precipitation. Runs 1–5 are the baked result.

struct Params {
    grid: vec4<u32>,        // n, stage flags, L, unused
    continents: vec4<f32>,  // radius, continent frequency, warp frequency, warp scale
    octaves: vec4<f32>,     // octaves, lacunarity, gain, unused
    seeds: vec4<u32>,       // continent, warp, temperature, unused
    relief: vec4<f32>,      // ocean coverage, land height, land exponent, ocean depth
    shelf: vec4<f32>,       // shelf depth, shelf fraction, unused, unused
    climate: vec4<f32>,     // equator °C, pole °C, axial tilt rad, lapse °C/km
    moderation: vec4<f32>,  // ocean moderation, blur step rad, noise °C, noise frequency
    wind: vec4<f32>,        // cells per hemisphere, meridional fraction, evaporation, rain
    moisture: vec4<f32>,    // step rad, spread, precipitation scale, unused
    pole: vec4<f32>,        // body-fixed rotation axis
}

struct Pass {
    src: u32,
    dst: u32,
    unused: vec2<u32>,
}

const HISTOGRAM_BINS: u32 = 4096u;
const STATS_SEA_LEVEL: u32 = 4096u;
const STATS_LOW: u32 = 4097u;
const STATS_HIGH: u32 = 4098u;
const RUN_RAW: u32 = 0u;
const RUN_ELEVATION: u32 = 1u;
const RUN_TEMPERATURE: u32 = 2u;
const RUN_MOISTURE: u32 = 3u;
const RUN_WIND_EAST: u32 = 4u;
const RUN_WIND_NORTH: u32 = 5u;
const RUN_PRECIPITATION: u32 = 10u;
const STAGE_CONTINENTS: u32 = 1u;
const STAGE_SEA_LEVEL: u32 = 2u;
const STAGE_SHELF: u32 = 4u;
const STAGE_TEMPERATURE: u32 = 8u;
const STAGE_WIND: u32 = 16u;
const STAGE_MOISTURE: u32 = 32u;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> fields: array<f32>;
@group(0) @binding(2) var<storage, read_write> stats: array<atomic<u32>>;
@group(0) @binding(3) var<uniform> pass_info: Pass;

fn cube_field(index: u32) -> f32 {
    return fields[index];
}

fn cube_n() -> u32 {
    return params.grid.x;
}

fn has(stage: u32) -> bool {
    return (params.grid.y & stage) != 0u;
}

fn run(index: u32) -> u32 {
    return index * params.grid.z;
}

struct Texel {
    k: u32,
    face: u32,
    i: u32,
    j: u32,
    valid: bool,
}

fn texel(id: vec3<u32>) -> Texel {
    let n = params.grid.x;
    var out: Texel;
    out.k = id.x + id.y * 65535u * 256u;
    out.valid = out.k < params.grid.z;
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

fn continent_noise(d: vec3<f32>) -> f32 {
    let warp_frequency = params.continents.z;
    let warp = vec3<f32>(
        fbm(d, warp_frequency, 3u, 2.0, 0.5, params.seeds.y),
        fbm(d, warp_frequency, 3u, 2.0, 0.5, params.seeds.y + 16u),
        fbm(d, warp_frequency, 3u, 2.0, 0.5, params.seeds.y + 32u));
    let warped = normalize(d + warp * params.continents.w);
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

// ---------------------------------------------------------------- continents

@compute @workgroup_size(256)
fn continents(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, params.grid.x);
    var r = 0.0;
    if has(STAGE_CONTINENTS) {
        r = continent_noise(d);
    }
    fields[run(RUN_RAW) + t.k] = r;
    atomicAdd(&stats[histogram_bin(r)], area_weight(t.i, t.j, params.grid.x));
}

// Percentile of the area-weighted histogram (linear within the crossing bin).
fn percentile(coverage: f32) -> f32 {
    var total = 0u;
    for (var b = 0u; b < HISTOGRAM_BINS; b = b + 1u) {
        total += atomicLoad(&stats[b]);
    }
    let target_weight = coverage * f32(total);
    var below = 0u;
    for (var b = 0u; b < HISTOGRAM_BINS; b = b + 1u) {
        let count = atomicLoad(&stats[b]);
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
        s = percentile(params.relief.x);
    }
    atomicStore(&stats[STATS_SEA_LEVEL], bitcast<u32>(s));
    atomicStore(&stats[STATS_LOW], bitcast<u32>(percentile(0.001)));
    atomicStore(&stats[STATS_HIGH], bitcast<u32>(percentile(0.999)));
}

fn shape_elevation(r: f32, s: f32, low: f32, high: f32) -> f32 {
    if r >= s {
        let t = clamp((r - s) / max(high - s, 1.0e-9), 0.0, 1.0);
        return params.relief.y * pow(t, params.relief.z);
    }
    let t = clamp((s - r) / max(s - low, 1.0e-9), 0.0, 1.0);
    let ocean_depth = params.relief.w;
    let shelf_depth = params.shelf.x;
    let shelf_fraction = params.shelf.y;
    var depth: f32;
    if has(STAGE_SHELF) && shelf_fraction > 0.0 {
        if t < shelf_fraction {
            depth = shelf_depth * t / shelf_fraction;
        } else {
            let u = clamp((t - shelf_fraction) / (1.0 - shelf_fraction), 0.0, 1.0);
            depth = shelf_depth + max(ocean_depth - shelf_depth, 0.0) * (1.0 - pow(1.0 - u, 3.0));
        }
    } else {
        depth = ocean_depth * (1.0 - pow(1.0 - t, 3.0));
    }
    return -depth;
}

@compute @workgroup_size(256)
fn shape(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let s = bitcast<f32>(atomicLoad(&stats[STATS_SEA_LEVEL]));
    let low = bitcast<f32>(atomicLoad(&stats[STATS_LOW]));
    let high = bitcast<f32>(atomicLoad(&stats[STATS_HIGH]));
    let h = shape_elevation(fields[run(RUN_RAW) + t.k], s, low, high);
    fields[run(RUN_ELEVATION) + t.k] = h;
    fields[run(pass_info.dst) + t.k] = select(0.0, 1.0, h < 0.0);
}

// ---------------------------------------------------------------- ocean blur

@compute @workgroup_size(256)
fn ocean_blur(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, params.grid.x);
    let frame = tangent_frame(d);
    let step = params.moderation.y;
    let src = run(pass_info.src);
    var sum = fields[src + t.k];
    sum += cube_sample(src, normalize(d + frame[0] * step), false);
    sum += cube_sample(src, normalize(d - frame[0] * step), false);
    sum += cube_sample(src, normalize(d + frame[1] * step), false);
    sum += cube_sample(src, normalize(d - frame[1] * step), false);
    fields[run(pass_info.dst) + t.k] = sum / 5.0;
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

fn wind_at(x: f32) -> vec2<f32> {
    let latitude = asin(clamp(x, -1.0, 1.0));
    let a = abs(latitude) / HALF_PI;
    let s = sin(params.wind.x * PI * a);
    return vec2<f32>(-s, -sign(latitude) * params.wind.y * s);
}

// Temperature (with the blurred ocean mask in run `src`) and wind.
@compute @workgroup_size(256)
fn climate(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, params.grid.x);
    let x = dot(d, params.pole.xyz);
    let middle = 0.5 * (params.climate.x + params.climate.y);
    var temperature = middle;
    if has(STAGE_TEMPERATURE) {
        let h = fields[run(RUN_ELEVATION) + t.k];
        let base = base_temperature(x) - params.climate.w * max(h, 0.0) / 1000.0;
        let ocean = fields[run(pass_info.src) + t.k];
        let moderated = middle + (base - middle) * (1.0 - params.moderation.x * ocean);
        let noise = params.moderation.z * fbm(d, params.moderation.w, 3u, 2.0, 0.5, params.seeds.z);
        temperature = moderated + noise;
    }
    fields[run(RUN_TEMPERATURE) + t.k] = temperature;
    var w = vec2<f32>(0.0);
    if has(STAGE_WIND) {
        w = wind_at(x);
    }
    fields[run(RUN_WIND_EAST) + t.k] = w.x;
    fields[run(RUN_WIND_NORTH) + t.k] = w.y;
}

// ---------------------------------------------------------------- moisture

fn evaporation_factor(t_c: f32) -> f32 {
    return clamp((t_c + 10.0) / 40.0, 0.1, 1.0);
}

// One semi-Lagrangian advection step: carried moisture `src` → `dst`.
@compute @workgroup_size(256)
fn moisture_step(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    let d = cube_texel_direction(t.face, t.i, t.j, params.grid.x);
    let frame = tangent_frame(d);
    let flow = frame[0] * fields[run(RUN_WIND_EAST) + t.k] + frame[1] * fields[run(RUN_WIND_NORTH) + t.k];
    let step = params.moisture.x;
    let src = run(pass_info.src);
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
    if fields[run(RUN_ELEVATION) + t.k] < 0.0 {
        evaporate = params.wind.z * evaporation_factor(fields[run(RUN_TEMPERATURE) + t.k]);
    }
    let rain = upwind * params.wind.w;
    fields[run(pass_info.dst) + t.k] = upwind + evaporate - rain;
    fields[run(RUN_PRECIPITATION) + t.k] += rain;
}

@compute @workgroup_size(256)
fn moisture_finish(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = texel(id);
    if !t.valid {
        return;
    }
    var m = 0.5;
    if has(STAGE_MOISTURE) {
        if fields[run(RUN_ELEVATION) + t.k] < 0.0 {
            m = 1.0;
        } else {
            m = 1.0 - exp(-fields[run(RUN_PRECIPITATION) + t.k] / params.moisture.z);
        }
    }
    fields[run(RUN_MOISTURE) + t.k] = m;
}

// ---------------------------------------------------------------- field mips

// Offset of mip `level` of elevation in `fields`; temperature and moisture
// follow at +6·m² and +12·m² (m = n >> level). Level 0 is runs 1–3; levels
// 1.. follow the 11 scratch runs, three fields of 6·m² values per level.
fn mip_offset(level: u32) -> u32 {
    if level == 0u {
        return run(RUN_ELEVATION);
    }
    var offset = 11u * params.grid.z;
    for (var k = 1u; k < level; k = k + 1u) {
        let m = params.grid.x >> k;
        offset += 18u * m * m;
    }
    return offset;
}

// 2×2 box average of field `pass_info.dst` (0 elevation, 1 temperature,
// 2 moisture) at level `pass_info.src` into the next level, per face.
@compute @workgroup_size(256)
fn field_mip(@builtin(global_invocation_id) id: vec3<u32>) {
    let level = pass_info.src;
    let fine_n = params.grid.x >> level;
    let n = fine_n >> 1u;
    let k = id.x + id.y * 65535u * 256u;
    if k >= 6u * n * n {
        return;
    }
    let face = k / (n * n);
    let j = (k / n) % n;
    let i = k % n;
    let src = mip_offset(level) + pass_info.dst * 6u * fine_n * fine_n;
    let a = fields[src + cube_index(face, 2u * i, 2u * j, fine_n)];
    let b = fields[src + cube_index(face, 2u * i + 1u, 2u * j, fine_n)];
    let c = fields[src + cube_index(face, 2u * i, 2u * j + 1u, fine_n)];
    let d = fields[src + cube_index(face, 2u * i + 1u, 2u * j + 1u, fine_n)];
    fields[mip_offset(level + 1u) + pass_info.dst * 6u * n * n + k] = 0.25 * (a + b + c + d);
}
