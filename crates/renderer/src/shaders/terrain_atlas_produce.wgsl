// GPU producer for terrain atlas tiles (ADR 0016). Under ADR 0023 this is the
// terrain authority; the world crate's band-limited CPU evaluation
// (`terrain::producer`) is its test oracle. All positions are evaluated
// relative to the tile's chart centre so f32 keeps sub-texel precision at deep
// levels. Prepended with terrain_noise.wgsl.

struct Tile {
    n0: vec4<f32>,          // chart-centre direction (body axes), w = |q0|
    face_u: vec4<f32>,      // w = chart width on the cube face
    face_v: vec4<f32>,      // w = cells
    info: vec4<u32>,        // atlas layer, kind (0 profile, 1 fields), macro count, detail count
    scale: vec4<f32>,       // radius, unused, unused, unused
    layer_origin: array<vec4<f32>, 5>, // profile: chart origin x/y/z modulo width, w = K
    layer_info: array<vec4<f32>, 5>,   // profile: mip, width at mip, amplitude, cubic flag
    band_cell: array<vec4<i32>, 6>,    // fields: base cell per band * 2 + layout
    band_frac: array<vec4<f32>, 6>,    // fields: base cell fraction, w = edge
    band_weight: vec4<f32>,            // fields: band footprint weights
    noise: vec4<u32>,                  // x = detail octave count (origins in `octaves`); world: y = Tier A face cells, z = elevation mip offset, w = mip cells
}

struct Dispatch {
    base: u32,
    side: u32,
    cells: u32,
    bounds_base: u32,
}

struct ProfileConstants {
    range: vec4<f32>, // low, high - low
}

struct FieldsConstants {
    axes: array<vec4<f32>, 4>,
    basins: array<vec4<f32>, 8>,          // xyz centre, w unused
    basin_params: array<vec4<f32>, 8>,    // scale, depth
    rot_x: vec4<f32>,                     // columns of the field rotation
    rot_y: vec4<f32>,
    rot_z: vec4<f32>,
    structure: vec4<f32>,                 // structure weights
    plains: vec4<f32>,                    // offset, low, high, basin rim strength
    relief: vec4<f32>,                    // R * relief * weights[0..3], radius
    bands: array<vec4<f32>, 3>,           // edge, budget, support
    salts: array<vec4<u32>, 3>,           // lattice salts (lo, hi) for layouts 0 and 1
    regional_strength: vec4<f32>,         // start, span
    crater_a: vec4<f32>,                  // bowl base, bowl freshness, rim base, rim freshness
    crater_b: vec4<f32>,                  // ejecta base, ejecta freshness, degradation offset, low
    crater_c: vec4<f32>,                  // degradation high, strength, jitter, shell
}

@group(0) @binding(0) var<storage, read> tiles: array<Tile>;
@group(0) @binding(1) var height_out: texture_storage_2d_array<r32float, write>;
@group(0) @binding(2) var normal_out: texture_storage_2d_array<rgba8snorm, write>;
@group(0) @binding(3) var<storage, read_write> bounds: array<atomic<u32>>;
@group(0) @binding(4) var<uniform> dispatch: Dispatch;
@group(0) @binding(5) var<storage, read> octaves: array<OctaveOrigin>;
// Collision pages: (height, normal xyz) per sample (pipeline §15.1).
@group(0) @binding(6) var<storage, read_write> collision_out: array<vec4<f32>>;
// Page albedo: sRGB-encoded linear colour; alpha 1 where the page owns its
// colour (world maps), 0 where the draw uses the instance material.
@group(0) @binding(7) var albedo_out: texture_storage_2d_array<rgba8unorm, write>;
// Page climate: (temperature °C, moisture, wind east, wind north) for world
// maps, zero elsewhere.
@group(0) @binding(8) var climate_out: texture_storage_2d_array<rgba16float, write>;
@group(1) @binding(0) var macro_image: texture_2d<u32>;
@group(1) @binding(1) var detail_image: texture_2d<u32>;
@group(1) @binding(2) var<uniform> profile: ProfileConstants;
@group(1) @binding(3) var<uniform> fields: FieldsConstants;
// World sources: Tier A result fields (elevation mips; see tier_a.rs).
@group(1) @binding(4) var<storage, read> world_fields: array<f32>;
// World sources: surface colour constants and biome LUT (AtlasWorldSurface).
@group(1) @binding(5) var<storage, read> world_surface: array<u32>;

var<private> tile: Tile;
// Face cells of the elevation mip sampled through cube_map.wgsl.
var<private> cube_level_n: u32;

fn cube_field(index: u32) -> f32 {
    return world_fields[index];
}

fn cube_n() -> u32 {
    return cube_level_n;
}
// Index of `tile` in the job list; its octave origins start at slot * MAX_OCTAVES.
var<private> tile_slot: u32;
const MAX_OCTAVES: u32 = 16u;

// ---------------------------------------------------------------- chart

struct ChartPoint {
    n: vec3<f32>,
    diff: vec3<f32>,
}

fn chart_point(st: vec2<f32>) -> ChartPoint {
    let n0 = tile.n0.xyz;
    let tangent = (tile.face_u.xyz * ((st.x - 0.5) * tile.face_u.w)
        + tile.face_v.xyz * ((st.y - 0.5) * tile.face_u.w)) / tile.n0.w;
    let s = 2.0 * dot(n0, tangent) + dot(tangent, tangent);
    let root = sqrt(1.0 + s);
    let k = 1.0 / root;
    var p: ChartPoint;
    p.diff = tangent * k - n0 * (s * k / (1.0 + root));
    p.n = n0 + p.diff;
    return p;
}

// ---------------------------------------------------------------- profile

struct Sampled {
    value: f32,
    gradient: vec3<f32>,
}

fn wrap(x: f32, width: f32) -> f32 {
    return x - floor(x / width) * width;
}

fn profile_value(image: u32, coord: vec2<i32>, mip: i32) -> f32 {
    var raw: u32;
    if image == 0u {
        raw = textureLoad(macro_image, coord, mip).x;
    } else {
        raw = textureLoad(detail_image, coord, mip).x;
    }
    if profile.range.y <= 0.0 {
        return 0.5;
    }
    return (f32(raw) - profile.range.x) / profile.range.y;
}

fn bspline(t: f32) -> mat2x4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    let u = 1.0 - t;
    let w = vec4<f32>(u * u * u / 6.0, (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
        (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0, t3 / 6.0);
    let d = vec4<f32>(-0.5 * u * u, 1.5 * t2 - 2.0 * t, -1.5 * t2 + t + 0.5, 0.5 * t2);
    return mat2x4<f32>(w, d);
}

// Returns value and (d/dx, d/dy) in texel units.
fn sample_grid(image: u32, x_in: f32, y_in: f32, width: f32, mip: i32, cubic: bool) -> vec3<f32> {
    let iw = i32(width);
    let x = wrap(x_in, width);
    let y = wrap(y_in, width);
    let xf = floor(x);
    let yf = floor(y);
    let tx = x - xf;
    let ty = y - yf;
    let x0 = i32(xf);
    let y0 = i32(yf);
    if cubic {
        var wx = bspline(tx);
        var wy = bspline(ty);
        var value = 0.0;
        var dx = 0.0;
        var dy = 0.0;
        for (var row = 0; row < 4; row = row + 1) {
            let py = ((y0 + row - 1) % iw + iw) % iw;
            for (var column = 0; column < 4; column = column + 1) {
                let px = ((x0 + column - 1) % iw + iw) % iw;
                let sample_value = profile_value(image, vec2<i32>(px, py), mip);
                value += wx[0][column] * wy[0][row] * sample_value;
                dx += wx[1][column] * wy[0][row] * sample_value;
                dy += wx[0][column] * wy[1][row] * sample_value;
            }
        }
        return vec3<f32>(value, dx, dy);
    }
    let x1 = (x0 + 1) % iw;
    let y1 = (y0 + 1) % iw;
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);
    let dsx = 6.0 * tx * (1.0 - tx);
    let dsy = 6.0 * ty * (1.0 - ty);
    let a = profile_value(image, vec2<i32>(x0, y0), mip);
    let b = profile_value(image, vec2<i32>(x1, y0), mip);
    let c = profile_value(image, vec2<i32>(x0, y1), mip);
    let d = profile_value(image, vec2<i32>(x1, y1), mip);
    let low = a + (b - a) * sx;
    let high = c + (d - c) * sx;
    return vec3<f32>(low + (high - low) * sy, ((b - a) * (1.0 - sy) + (d - c) * sy) * dsx,
        (high - low) * dsy);
}

fn sample_triplanar(image: u32, layer: u32, p: ChartPoint, weights: vec3<f32>,
    wgrad: mat3x3<f32>) -> Sampled {
    let origin = tile.layer_origin[layer];
    let info = tile.layer_info[layer];
    let k = origin.w;
    let mip = i32(info.x);
    let width = info.y;
    let cubic = info.w > 0.5;
    // Chart coordinate of each body-axis component, relative to the tile.
    let cx = origin.x + p.diff.x * k;
    let cy = origin.y + p.diff.y * k;
    let cz = origin.z + p.diff.z * k;
    let s0 = sample_grid(image, cy, cz, width, mip, cubic); // axis 0: (y, z)
    let s1 = sample_grid(image, cx, cz, width, mip, cubic); // axis 1: (x, z)
    let s2 = sample_grid(image, cx, cy, width, mip, cubic); // axis 2: (x, y)
    var out: Sampled;
    out.value = weights.x * s0.x + weights.y * s1.x + weights.z * s2.x;
    let g0 = vec3<f32>(0.0, s0.y, s0.z) * k;
    let g1 = vec3<f32>(s1.y, 0.0, s1.z) * k;
    let g2 = vec3<f32>(s2.y, s2.z, 0.0) * k;
    out.gradient = wgrad[0] * s0.x + g0 * weights.x
        + wgrad[1] * s1.x + g1 * weights.y
        + wgrad[2] * s2.x + g2 * weights.z;
    return out;
}

struct HeightSample {
    height: f32,
    gradient: vec3<f32>,
}

fn evaluate_profile(p: ChartPoint) -> HeightSample {
    let n = p.n;
    let raw = vec3<f32>(n.x * n.x * n.x * n.x, n.y * n.y * n.y * n.y, n.z * n.z * n.z * n.z);
    let total = raw.x + raw.y + raw.z;
    let raw_g = mat3x3<f32>(vec3<f32>(4.0 * n.x * n.x * n.x, 0.0, 0.0),
        vec3<f32>(0.0, 4.0 * n.y * n.y * n.y, 0.0), vec3<f32>(0.0, 0.0, 4.0 * n.z * n.z * n.z));
    let total_g = raw_g[0] + raw_g[1] + raw_g[2];
    let weights = raw / total;
    let wgrad = mat3x3<f32>(
        (raw_g[0] * total - total_g * raw.x) / (total * total),
        (raw_g[1] * total - total_g * raw.y) / (total * total),
        (raw_g[2] * total - total_g * raw.z) / (total * total));
    var out: HeightSample;
    out.height = 0.0;
    out.gradient = vec3<f32>(0.0);
    var coarse = 0.0;
    var coarse_g = vec3<f32>(0.0);
    let macro_count = tile.info.z;
    for (var i = 0u; i < macro_count; i = i + 1u) {
        let s = sample_triplanar(0u, i, p, weights, wgrad);
        let amplitude = tile.layer_info[i].z;
        out.height += (s.value - 0.5) * amplitude;
        out.gradient += s.gradient * amplitude;
        if i == 0u {
            coarse = s.value;
            coarse_g = s.gradient;
        }
    }
    for (var j = 0u; j < tile.info.w; j = j + 1u) {
        let layer = macro_count + j;
        let s = sample_triplanar(1u, layer, p, weights, wgrad);
        let amplitude = tile.layer_info[layer].z;
        let centered = s.value - 0.5;
        let geology = 0.35 + 0.65 * coarse;
        out.height += centered * amplitude * geology;
        out.gradient += (s.gradient * geology + coarse_g * (centered * 0.65)) * amplitude;
    }
    return out;
}

// ---------------------------------------------------------------- 64-bit hashing

fn mul32(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    return vec2<u32>((p00 & 0xffffu) | (mid << 16u), p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u));
}

fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    var r = mul32(a.x, b.x);
    r.y = r.y + a.x * b.y + a.y * b.x;
    return r;
}

fn shr64(a: vec2<u32>, k: u32) -> vec2<u32> {
    if k >= 32u {
        return vec2<u32>(a.y >> (k - 32u), 0u);
    }
    return vec2<u32>((a.x >> k) | (a.y << (32u - k)), a.y >> k);
}

fn shl64(a: vec2<u32>, k: u32) -> vec2<u32> {
    if k >= 32u {
        return vec2<u32>(0u, a.x << (k - 32u));
    }
    return vec2<u32>(a.x << k, (a.y << k) | (a.x >> (32u - k)));
}

fn rotl64(a: vec2<u32>, k: u32) -> vec2<u32> {
    return shl64(a, k) | shr64(a, 64u - k);
}

fn mix64(input: vec2<u32>) -> vec2<u32> {
    var x = input;
    x = x ^ shr64(x, 30u);
    x = mul64(x, vec2<u32>(0x1ce4e5b9u, 0xbf58476du));
    x = x ^ shr64(x, 27u);
    x = mul64(x, vec2<u32>(0x133111ebu, 0x94d049bbu));
    return x ^ shr64(x, 31u);
}

fn from_i32(x: i32) -> vec2<u32> {
    return vec2<u32>(bitcast<u32>(x), select(0u, 0xffffffffu, x < 0));
}

// f32 approximation of `unit(key)`: the leading 24 of its 53 random bits.
fn unit(key: vec2<u32>) -> f32 {
    return f32(mix64(key).y >> 8u) / 16777216.0;
}

fn cell_hash(salt: vec2<u32>, cell: vec3<i32>) -> vec2<u32> {
    return mix64(salt ^ mix64(from_i32(cell.x)) ^ mix64(rotl64(from_i32(cell.y), 21u))
        ^ mix64(rotl64(from_i32(cell.z), 42u)));
}

// ---------------------------------------------------------------- fields

struct D {
    v: f32,
    g: vec3<f32>,
}

fn dconst(v: f32) -> D {
    return D(v, vec3<f32>(0.0));
}
fn dadd(a: D, b: D) -> D {
    return D(a.v + b.v, a.g + b.g);
}
fn dsub(a: D, b: D) -> D {
    return D(a.v - b.v, a.g - b.g);
}
fn dmul(a: D, b: D) -> D {
    return D(a.v * b.v, a.g * b.v + b.g * a.v);
}
fn dscale(a: D, s: f32) -> D {
    return D(a.v * s, a.g * s);
}
fn dsmooth01(a: D) -> D {
    if a.v <= 0.0 {
        return dconst(0.0);
    }
    if a.v >= 1.0 {
        return dconst(1.0);
    }
    return D(a.v * a.v * (3.0 - 2.0 * a.v), a.g * (6.0 * a.v * (1.0 - a.v)));
}
fn drange(a: D, low: f32, high: f32) -> D {
    return dsmooth01(dscale(dsub(a, dconst(low)), 1.0 / (high - low)));
}
fn dreciprocal(a: D) -> D {
    let inv = 1.0 / a.v;
    return D(inv, a.g * (-inv * inv));
}

struct Global {
    height: D,
    regional: D,
    highlands: f32,
}

fn global_field(n: vec3<f32>) -> Global {
    let a0 = fields.axes[0].xyz;
    let a1 = fields.axes[1].xyz;
    let a2 = fields.axes[2].xyz;
    let a3 = fields.axes[3].xyz;
    let x = D(dot(n, a0), a0);
    let y = D(dot(n, a1), a1);
    let z = D(dot(n, a2), a2);
    let cross_term = D(dot(n, a3), a3);
    let w = fields.structure;
    let structure = dadd(dadd(dadd(dscale(dmul(x, x), w.x), dscale(dmul(y, z), w.y)),
        dscale(dmul(cross_term, cross_term), w.z)), dconst(w.w));
    var basin = dconst(0.0);
    for (var i = 0; i < 8; i = i + 1) {
        let center = fields.basins[i].xyz;
        let q = D(1.0 - dot(n, center), -center);
        let scale = fields.basin_params[i].x;
        var bowl = drange(dsub(dconst(1.0), dscale(q, 1.0 / scale)), 0.0, 1.0);
        bowl = dmul(bowl, bowl);
        let rim = drange(dscale(q, 1.0 / scale), 0.62, 0.95);
        basin = dadd(dsub(basin, dscale(bowl, fields.basin_params[i].y)),
            dscale(dmul(rim, bowl), fields.plains.w));
    }
    let plains = drange(dadd(structure, dconst(fields.plains.x)), fields.plains.y, fields.plains.z);
    var out: Global;
    out.height = dsub(dadd(dscale(structure, fields.relief.x), dscale(basin, fields.relief.y)),
        dscale(plains, fields.relief.z));
    out.regional = dadd(structure, basin);
    out.highlands = 1.0 - plains.v;
    return out;
}

fn crater_profile(q2: D, freshness: f32, strength: f32, regional: D) -> D {
    let one = dconst(1.0);
    let inside = dsub(one, q2);
    let fresh = 1.0 - freshness;
    let a = fields.crater_a;
    let b = fields.crater_b;
    let c = fields.crater_c;
    let inside2 = dmul(inside, inside);
    let inside3 = dmul(inside2, inside);
    let bowl = dscale(inside2, -(a.x + a.y * fresh) * strength);
    let rim = dscale(dmul(q2, inside3), (256.0 / 27.0) * (a.z + a.w * fresh) * strength);
    let ejecta = dscale(dmul(drange(q2, 0.42, 0.98), inside2), b.x + b.y * fresh);
    let degradation = drange(dadd(regional, dconst(b.z)), b.w, c.x);
    let subdued = dsub(one, dscale(degradation, c.y * freshness));
    return dmul(dadd(dadd(bowl, rim), ejecta), subdued);
}

fn evaluate_fields(p: ChartPoint) -> HeightSample {
    let rot = mat3x3<f32>(fields.rot_x.xyz, fields.rot_y.xyz, fields.rot_z.xyz);
    let radius = fields.relief.w;
    let n = transpose(rot) * p.n;
    let diff = transpose(rot) * p.diff;
    let origin = transpose(rot) * tile.n0.xyz * radius;
    let local = diff * radius;
    let global = global_field(n);
    var total = global.height;
    for (var band = 0; band < 3; band = band + 1) {
        let weight = tile.band_weight[band];
        if weight <= 0.0 {
            continue;
        }
        let edge = fields.bands[band].x;
        let budget = fields.bands[band].y;
        let support = fields.bands[band].z;
        var numerator = dconst(0.0);
        var denominator = dconst(1.0);
        for (var lattice = 0; lattice < 2; lattice = lattice + 1) {
            let slot = band * 2 + lattice;
            let base_cell = tile.band_cell[slot].xyz;
            let base_frac = tile.band_frac[slot].xyz;
            let salt_word = fields.salts[band];
            var salt = salt_word.xy;
            if lattice == 1 {
                salt = salt_word.zw;
            }
            let relative = vec3<i32>(floor(base_frac + local / edge));
            for (var dx = -1; dx <= 1; dx = dx + 1) {
                for (var dy = -1; dy <= 1; dy = dy + 1) {
                    for (var dz = -1; dz <= 1; dz = dz + 1) {
                        let offset = relative + vec3<i32>(dx, dy, dz);
                        let key = cell_hash(salt, base_cell + offset);
                        let jitter = vec3<f32>(unit(key ^ vec2<u32>(1u, 0u)),
                            unit(key ^ vec2<u32>(2u, 0u)), unit(key ^ vec2<u32>(3u, 0u))) * 2.0
                            - vec3<f32>(1.0);
                        let raw_local = (vec3<f32>(offset) + vec3<f32>(0.5) + jitter * fields.crater_c.z
                            - base_frac) * edge;
                        let e = 2.0 * dot(origin, raw_local) + dot(raw_local, raw_local);
                        let distance = radius * sqrt(1.0 + e / (radius * radius));
                        let above = e / (distance + radius);
                        if abs(above) > edge * fields.crater_c.w {
                            continue;
                        }
                        let f = radius / distance;
                        let center_local = raw_local * f - origin * (above / distance);
                        let delta = local - center_local;
                        let q2 = D(dot(delta, delta) / (support * support),
                            delta * (2.0 * radius / (support * support)));
                        if q2.v >= 1.0 {
                            continue;
                        }
                        let freshness = unit(key ^ vec2<u32>(0x46524553u, 0x4d4f4f4eu));
                        let center_n = normalize(origin + center_local);
                        let strength = clamp(fields.regional_strength.x
                            + fields.regional_strength.y * global_field(center_n).highlands, 0.0, 1.0);
                        let inside = dsub(dconst(1.0), q2);
                        let window = dmul(dmul(inside, inside), inside);
                        let profile_value = crater_profile(q2, freshness, strength, global.regional);
                        numerator = dadd(numerator, dscale(dmul(window, profile_value), budget));
                        denominator = dadd(denominator, window);
                    }
                }
            }
        }
        total = dadd(total, dscale(dmul(numerator, dreciprocal(denominator)), weight));
    }
    var out: HeightSample;
    out.height = total.v;
    out.gradient = rot * total.g;
    return out;
}

// ---------------------------------------------------------------- world (Tier A)

// glam's any_orthonormal_vector, as the CPU oracle (WorldMaps::sample) uses.
fn any_orthonormal(d: vec3<f32>) -> vec3<f32> {
    let sign = select(-1.0, 1.0, d.z >= 0.0);
    let a = -1.0 / (sign + d.z);
    return vec3<f32>(d.x * d.y * a, sign + d.y * d.y * a, -d.y);
}

// Bicubic macro elevation of the tile's mip and its tangent gradient (central
// differences over a quarter texel), mirroring WorldMaps::sample.
fn evaluate_world(p: ChartPoint) -> HeightSample {
    cube_level_n = tile.noise.w;
    let base = tile.noise.z;
    let d = normalize(p.n);
    let delta = 0.5 / f32(cube_level_n);
    let e1 = any_orthonormal(d);
    let e2 = cross(d, e1);
    var out: HeightSample;
    out.height = cube_sample(base, d, true);
    let g1 = cube_sample(base, normalize(d + e1 * delta), true) - cube_sample(base, normalize(d - e1 * delta), true);
    let g2 = cube_sample(base, normalize(d + e2 * delta), true) - cube_sample(base, normalize(d - e2 * delta), true);
    out.gradient = (e1 * g1 + e2 * g2) / (2.0 * delta);
    return out;
}

// ---------------------------------------------------------------- detail fBm

// Band-limited fBm (pipeline §9.3, App. A.3). Band-limit weights are folded into
// each origin's amplitude on the CPU; octaves above the node's limit are absent.
// The fade towards the parent level shares the draw shader's morph `t`, which
// blends this page with the parent page. Returns (height, d height / d local).
fn detail_fbm(local: vec3<f32>) -> vec4<f32> {
    var sum = vec4<f32>(0.0);
    let first = tile_slot * MAX_OCTAVES;
    for (var k = 0u; k < min(tile.noise.x, MAX_OCTAVES); k = k + 1u) {
        let o = octaves[first + k];
        sum += split_gradient_noise(o, local) * o.amplitude.x;
    }
    return sum;
}

// ---------------------------------------------------------------- surface colour

// World-map colour (pipeline §10, M1): biome LUT tint by temperature ×
// moisture from the climate mips matching the node, snow by temperature and
// slope, flat water below sea level. Mirrors `ProducerRecipe::albedo`.
const SURFACE_LUT: u32 = 24u;

fn surface_f32(i: u32) -> f32 {
    return bitcast<f32>(world_surface[i]);
}

fn surface_vec3(i: u32) -> vec3<f32> {
    return vec3<f32>(surface_f32(i), surface_f32(i + 1u), surface_f32(i + 2u));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055), x * 12.92, x <= vec3<f32>(0.0031308));
}

fn lut_texel(i: u32, j: u32, n: u32) -> vec3<f32> {
    let w = world_surface[SURFACE_LUT + j * n + i];
    let c = vec3<f32>(f32(w & 255u), f32((w >> 8u) & 255u), f32((w >> 16u) & 255u)) / 255.0;
    return srgb_to_linear(c);
}

fn biome_colour(t: f32, m: f32) -> vec3<f32> {
    let n = world_surface[0];
    let span = f32(n - 1u);
    let x = clamp((t - surface_f32(4u)) / (surface_f32(5u) - surface_f32(4u)) * span, 0.0, span);
    let y = clamp((m - surface_f32(6u)) / (surface_f32(7u) - surface_f32(6u)) * span, 0.0, span);
    let x0 = min(u32(floor(x)), n - 2u);
    let y0 = min(u32(floor(y)), n - 2u);
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    let top = mix(lut_texel(x0, y0, n), lut_texel(x0 + 1u, y0, n), fx);
    let bottom = mix(lut_texel(x0, y0 + 1u, n), lut_texel(x0 + 1u, y0 + 1u, n), fx);
    return mix(top, bottom, fy);
}

fn world_albedo(d: vec3<f32>, height: f32, normal: vec3<f32>) -> vec3<f32> {
    if height < 0.0 {
        let deep = 1.0 - exp(height / surface_f32(19u));
        return mix(surface_vec3(16u), surface_vec3(20u), deep);
    }
    cube_level_n = tile.noise.w;
    let stride = 6u * cube_level_n * cube_level_n;
    let t = cube_sample(tile.noise.z + stride, d, false);
    let m = cube_sample(tile.noise.z + 2u * stride, d, false);
    let slope = acos(clamp(dot(normal, d), -1.0, 1.0));
    let snow = (1.0 - smoothstep(surface_f32(8u) - surface_f32(9u), surface_f32(8u) + surface_f32(9u), t))
        * (1.0 - smoothstep(surface_f32(10u), surface_f32(11u), slope));
    return mix(biome_colour(t, m), surface_vec3(12u), snow);
}

// Page albedo texel for an evaluated sample `value` (normal, height) at `st`.
fn page_albedo(st: vec2<f32>, value: vec4<f32>) -> vec4<f32> {
    if tile.info.y != 2u {
        return vec4<f32>(0.0);
    }
    let d = normalize(chart_point(st).n);
    return vec4<f32>(linear_to_srgb(world_albedo(d, value.w, value.xyz)), 1.0);
}

// Page climate texel at `st`: temperature and moisture from the node's mip (as
// the albedo), wind from the level-0 runs (3 and 4 of the Tier A result).
fn page_climate(st: vec2<f32>) -> vec4<f32> {
    if tile.info.y != 2u || tile.noise.y == 0u {
        return vec4<f32>(0.0);
    }
    let d = normalize(chart_point(st).n);
    cube_level_n = tile.noise.w;
    let stride = 6u * cube_level_n * cube_level_n;
    let t = cube_sample(tile.noise.z + stride, d, false);
    let m = cube_sample(tile.noise.z + 2u * stride, d, false);
    cube_level_n = tile.noise.y;
    let run = 6u * cube_level_n * cube_level_n;
    let east = cube_sample(3u * run, d, false);
    let north = cube_sample(4u * run, d, false);
    return vec4<f32>(t, m, east, north);
}

// ---------------------------------------------------------------- entry points

fn evaluate(st: vec2<f32>) -> vec4<f32> {
    let p = chart_point(st);
    var sample_value: HeightSample;
    if tile.info.y == 0u {
        sample_value = evaluate_profile(p);
    } else if tile.info.y == 1u {
        sample_value = evaluate_fields(p);
    } else {
        sample_value = evaluate_world(p);
    }
    if tile.noise.x > 0u {
        // Split lattice: the CPU origin is the exact f64 chart centre n0 * R, so
        // local = diff * R places samples at the true surface point.
        let detail = detail_fbm(p.diff * tile.scale.x);
        sample_value.height += detail.x;
        sample_value.gradient += detail.yzw * tile.scale.x;
    }
    let n = normalize(p.n);
    let tangent_gradient = sample_value.gradient - n * dot(n, sample_value.gradient);
    let normal = normalize(n - tangent_gradient / (tile.scale.x + sample_value.height));
    return vec4<f32>(normal, sample_value.height);
}

fn ordered(value: f32) -> u32 {
    let bits = bitcast<u32>(value);
    if (bits & 0x80000000u) != 0u {
        return ~bits;
    }
    return bits | 0x80000000u;
}

@compute @workgroup_size(8, 8, 1)
fn produce_heights(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= dispatch.side || id.y >= dispatch.side {
        return;
    }
    tile_slot = dispatch.base + id.z;
    tile = tiles[dispatch.base + id.z];
    let st = (vec2<f32>(id.xy) - vec2<f32>(1.0)) / f32(dispatch.cells);
    let value = evaluate(st);
    let layer = i32(tile.info.x);
    textureStore(height_out, vec2<i32>(id.xy), layer, vec4<f32>(value.w, 0.0, 0.0, 0.0));
    if dispatch.side == dispatch.cells + 3u {
        // Normal map at geometry resolution shares this evaluation.
        textureStore(normal_out, vec2<i32>(id.xy), layer, vec4<f32>(value.xyz, 0.0));
        textureStore(albedo_out, vec2<i32>(id.xy), layer, page_albedo(st, value));
        textureStore(climate_out, vec2<i32>(id.xy), layer, page_climate(st));
    }
    // 4x4 min/max grid over the chart; samples on shared cell edges and the
    // border apron count towards every adjacent cell.
    let base = (dispatch.bounds_base + id.z) * 32u;
    let key = ordered(value.w);
    let g = clamp(st, vec2<f32>(0.0), vec2<f32>(1.0)) * 4.0;
    let lo = clamp(vec2<i32>(floor(g - vec2<f32>(1.0e-4))), vec2<i32>(0), vec2<i32>(3));
    let hi = clamp(vec2<i32>(floor(g + vec2<f32>(1.0e-4))), vec2<i32>(0), vec2<i32>(3));
    for (var cy = lo.y; cy <= hi.y; cy = cy + 1) {
        for (var cx = lo.x; cx <= hi.x; cx = cx + 1) {
            let slot = base + u32(cy * 4 + cx) * 2u;
            atomicMin(&bounds[slot], key);
            atomicMax(&bounds[slot + 1u], key);
        }
    }
}

@compute @workgroup_size(8, 8, 1)
fn produce_normals(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= dispatch.side || id.y >= dispatch.side {
        return;
    }
    tile_slot = dispatch.base + id.z;
    tile = tiles[dispatch.base + id.z];
    let st = (vec2<f32>(id.xy) - vec2<f32>(1.0)) / f32(dispatch.cells);
    let value = evaluate(st);
    textureStore(normal_out, vec2<i32>(id.xy), i32(tile.info.x), vec4<f32>(value.xyz, 0.0));
    textureStore(albedo_out, vec2<i32>(id.xy), i32(tile.info.x), page_albedo(st, value));
    textureStore(climate_out, vec2<i32>(id.xy), i32(tile.info.x), page_climate(st));
}

// Collision page: (cells + 1)^2 samples at st = id / cells, read back to the
// CPU heightfield colliders. `dispatch.bounds_base` is the group's first page.
@compute @workgroup_size(8, 8, 1)
fn produce_collision(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= dispatch.side || id.y >= dispatch.side {
        return;
    }
    tile_slot = dispatch.base + id.z;
    tile = tiles[tile_slot];
    let st = vec2<f32>(id.xy) / f32(dispatch.cells);
    let value = evaluate(st);
    let side = dispatch.side;
    collision_out[(dispatch.bounds_base + id.z) * side * side + id.y * side + id.x] =
        vec4<f32>(value.w, value.xyz);
}
