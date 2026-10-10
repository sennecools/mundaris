// Landform relief on world maps (M2 Shape Step 6a; pipeline §8.1–§8.3,
// §9.6). PROTOTYPE: interprets the packed landform set
// (`astrum_world::terrain::landform::gpu`) appended to `world_surface` after
// the biome LUT, per sample: weight bytecode over level-0 Tier A fields,
// then each landform's recipe ops. Mirrors the CPU oracle
// (`landform::world_source::compose`, `landform::eval`) in f32. Step 6b
// replaces the recipe interpreter with generated WGSL and moves the weights
// into the Tier A bake. Appended to terrain_atlas_produce.wgsl.

var<private> lf_base: u32;
// Value registers of one program: (value, gradient per metre in body axes).
var<private> lf_reg: array<vec4<f32>, 16>;
// Per-octave (effective frequency, gradient contribution) of a stack.
var<private> lf_contrib: array<vec4<f32>, 24>;

fn lf_word(i: u32) -> u32 {
    return world_surface[lf_base + i];
}

fn lf_f32(i: u32) -> f32 {
    return bitcast<f32>(lf_word(i));
}

fn lf_i32(i: u32) -> i32 {
    return bitcast<i32>(lf_word(i));
}

// `landform::eval::node_seed`.
fn lf_node_seed(body_seed: u32, salt: u32, lane: u32) -> u32 {
    return pcg3d(vec3<u32>(body_seed, salt, lane ^ 0x4c460000u)).x;
}

fn lf_tangential(g: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    return g - normal * dot(normal, g);
}

// ---------------------------------------------------------------- fields

// Level-0 Tier A run and decoding (`cube_decode`) of a recipe field
// (`RecipeField::id`): uplift, sediment, flow, hardness (aux0 bytes 0, 2, 3,
// 1), moisture, temperature, volcanic (aux1 byte 2), elevation,
// boundary_coord (i32, 1/16 m).
fn lf_field_run(field: u32) -> vec2<u32> {
    switch field {
        case 0u: { return vec2<u32>(6u, 2u); }
        case 1u: { return vec2<u32>(6u, 4u); }
        case 2u: { return vec2<u32>(6u, 5u); }
        case 3u: { return vec2<u32>(6u, 3u); }
        case 4u: { return vec2<u32>(2u, 0u); }
        case 5u: { return vec2<u32>(1u, 0u); }
        case 6u: { return vec2<u32>(7u, 4u); }
        case 7u: { return vec2<u32>(0u, 0u); }
        case 9u, 10u, 11u, 12u: { return vec2<u32>(8u, field - 7u); }
        default: { return vec2<u32>(5u, 1u); }
    }
}

// Field sampling (prototype). Fields at one direction share the cube-map
// addressing (face choice, cross-face texels, weights), so four fields are
// sampled per addressing pass, and every field at the sample direction is
// cached once per sample (`lf_fill_cache`). Same formulas as cube_sample and
// `MapsFields` (bicubic level 0; gradients by central differences over a
// quarter texel). Few call sites, because drivers inline every one.

// Per-sample cache: (value, gradient per metre) by field id at the sample
// direction.
var<private> lf_cache: array<vec4<f32>, 9>;
// True once `lf_fill_cache` ran for this sample.
var<private> lf_cache_valid: bool;
// Field ids of the four lanes of a 4-field sample (> 8: unused lane).
var<private> lf4_ids: vec4<u32>;

// The four lanes' decoded values of texel `index` of a level-0 run.
fn lf4_fetch(index: u32) -> vec4<f32> {
    let n = tile.noise.y;
    let len = 6u * n * n;
    var out = vec4<f32>(0.0);
    for (var c = 0u; c < 4u; c = c + 1u) {
        let id = lf4_ids[c];
        if id > 12u {
            continue;
        }
        let rd = lf_field_run(id);
        let word = world_fields[rd.x * len + index];
        var v = bitcast<f32>(word);
        if rd.y == 1u {
            v = f32(bitcast<i32>(word)) / 16.0;
        } else if rd.y >= 2u {
            v = f32((word >> (8u * (rd.y - 2u))) & 255u) / 255.0;
        }
        out[c] = v;
    }
    return out;
}

// cube_value_across for the four lanes.
fn lf4_across(face: u32, i: i32, j: i32) -> vec4<f32> {
    let n = tile.noise.y;
    let ni = i32(n);
    if i >= 0 && i < ni && j >= 0 && j < ni {
        return lf4_fetch(cube_index(face, u32(i), u32(j), n));
    }
    let b = face_basis(face);
    let u = -1.0 + (2.0 * f32(i) + 1.0) / f32(n);
    let v = -1.0 + (2.0 * f32(j) + 1.0) / f32(n);
    let at = cube_locate(normalize(b[0] + u * b[1] + v * b[2]));
    let fx = clamp((at.u + 1.0) * 0.5 * f32(n) - 0.5, 0.0, f32(n) - 1.0);
    let fy = clamp((at.v + 1.0) * 0.5 * f32(n) - 0.5, 0.0, f32(n) - 1.0);
    let x0 = u32(floor(fx));
    let y0 = u32(floor(fy));
    let x1 = min(x0 + 1u, n - 1u);
    let y1 = min(y0 + 1u, n - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    var q: array<vec4<f32>, 4>;
    var xs = array<u32, 4>(x0, x1, x0, x1);
    var ys = array<u32, 4>(y0, y0, y1, y1);
    for (var k = 0u; k < 4u; k = k + 1u) {
        q[k] = lf4_fetch(cube_index(at.face, xs[k], ys[k], n));
    }
    return (q[0] * (1.0 - tx) + q[1] * tx) * (1.0 - ty) + (q[2] * (1.0 - tx) + q[3] * tx) * ty;
}

// cube_bicubic_on for the four lanes, with derivatives: [value, d/dfx,
// d/dfy] (`CubeMap::bicubic_d_on`).
fn lf4_bicubic_d_on(face: u32, u: f32, v: f32) -> array<vec4<f32>, 3> {
    let n = f32(tile.noise.y);
    let fx = (u + 1.0) * 0.5 * n - 0.5;
    let fy = (v + 1.0) * 0.5 * n - 0.5;
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let tx = fx - floor(fx);
    let ty = fy - floor(fy);
    let wx = catmull_rom(tx);
    let wy = catmull_rom(ty);
    let dx = 0.5 * vec4<f32>(-3.0 * tx * tx + 4.0 * tx - 1.0, 9.0 * tx * tx - 10.0 * tx,
        -9.0 * tx * tx + 8.0 * tx + 1.0, 3.0 * tx * tx - 2.0 * tx);
    let dy = 0.5 * vec4<f32>(-3.0 * ty * ty + 4.0 * ty - 1.0, 9.0 * ty * ty - 10.0 * ty,
        -9.0 * ty * ty + 8.0 * ty + 1.0, 3.0 * ty * ty - 2.0 * ty);
    var out: array<vec4<f32>, 3>;
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            let value = lf4_across(face, x0 - 1 + i, y0 - 1 + j);
            out[0] += wx[i] * wy[j] * value;
            out[1] += dx[i] * wy[j] * value;
            out[2] += wx[i] * dy[j] * value;
        }
    }
    return out;
}

// Four lanes' bicubic values and tangent gradients per unit direction at
// `d` (`CubeMap::bicubic_gradient`): [value, ∂x, ∂y, ∂z], one vec4 lane each.
fn lf4_sample_grad(d: vec3<f32>) -> array<vec4<f32>, 4> {
    let nf = f32(tile.noise.y);
    let band = 1.0 / nf;
    let half_n = 0.5 * nf;
    var sum = vec4<f32>(0.0);
    var g = array<vec4<f32>, 3>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var vdw = array<vec4<f32>, 3>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var total = 0.0;
    var total_dw = vec3<f32>(0.0);
    var out: array<vec4<f32>, 4>;
    var done = false;
    for (var f = 0u; f < 6u; f = f + 1u) {
        let b = face_basis(f);
        let w = dot(d, b[0]);
        if w <= 0.0 {
            continue;
        }
        let u = dot(d, b[1]) / w;
        let v = dot(d, b[2]) / w;
        let t = (1.0 - max(abs(u), abs(v)) + band) / (2.0 * band);
        if t <= 0.0 {
            continue;
        }
        let r = lf4_bicubic_d_on(f, u, v);
        let grad_u = (b[1] - b[0] * u) / w;
        let grad_v = (b[2] - b[0] * v) / w;
        var fg: array<vec4<f32>, 3>;
        for (var a = 0u; a < 3u; a = a + 1u) {
            fg[a] = (grad_u[a] * r[1] + grad_v[a] * r[2]) * half_n;
        }
        if t >= 1.0 {
            out[0] = r[0];
            out[1] = fg[0];
            out[2] = fg[1];
            out[3] = fg[2];
            done = true;
            break;
        }
        let weight = t * t * (3.0 - 2.0 * t);
        var grad_m = grad_v * select(-1.0, 1.0, v >= 0.0);
        if abs(u) >= abs(v) {
            grad_m = grad_u * select(-1.0, 1.0, u >= 0.0);
        }
        let dw = grad_m * (-6.0 * t * (1.0 - t) / (2.0 * band));
        sum += weight * r[0];
        total += weight;
        total_dw += dw;
        for (var a = 0u; a < 3u; a = a + 1u) {
            g[a] += weight * fg[a];
            vdw[a] += dw[a] * r[0];
        }
    }
    if !done {
        let tot = max(total, 1.0e-30);
        out[0] = sum / tot;
        for (var a = 0u; a < 3u; a = a + 1u) {
            out[a + 1u] = (g[a] + vdw[a]) / tot - total_dw[a] * (sum / (tot * tot));
        }
    }
    // Tangent projection per lane.
    let along = d.x * out[1] + d.y * out[2] + d.z * out[3];
    out[1] -= d.x * along;
    out[2] -= d.y * along;
    out[3] -= d.z * along;
    return out;
}

// Values and gradients (per metre) of the four lanes `ids` at direction `d`;
// lane c is (value, gradient) in out[c].
fn lf4_field(ids: vec4<u32>, d: vec3<f32>) -> array<vec4<f32>, 4> {
    lf4_ids = ids;
    let s = lf4_sample_grad(d);
    var out: array<vec4<f32>, 4>;
    for (var c = 0u; c < 4u; c = c + 1u) {
        out[c] = vec4<f32>(s[0][c], vec3<f32>(s[1][c], s[2][c], s[3][c]) / tile.scale.x);
    }
    return out;
}

// Fill `lf_cache` at direction `d` with the fields in bit mask `used`
// (`RecipeField::id` bits, `landform::gpu` header word 1): passes of four
// lanes, skipping groups no recipe reads. Group 0 (uplift, sediment, flow,
// hardness) always runs: the shape overlay page reads it.
fn lf_fill_cache(d: vec3<f32>, used: u32) {
    var groups = array<vec4<u32>, 3>(
        vec4<u32>(0u, 1u, 2u, 3u),
        vec4<u32>(4u, 5u, 7u, 8u),
        vec4<u32>(6u, 99u, 99u, 99u),
    );
    var masks = array<u32, 3>(0xfu, 0x1b0u, 0x40u);
    for (var gi = 0u; gi < 3u; gi = gi + 1u) {
        if gi > 0u && (used & masks[gi]) == 0u {
            continue;
        }
        let ids = groups[gi];
        let f = lf4_field(ids, d);
        for (var c = 0u; c < 4u; c = c + 1u) {
            if ids[c] <= 8u {
                lf_cache[ids[c]] = f[c];
            }
        }
    }
}

// `RecipeField::range`.
fn lf_field_range(field: u32) -> vec2<f32> {
    switch field {
        case 5u: { return vec2<f32>(-250.0, 500.0); }
        case 7u: { return vec2<f32>(-30000.0, 30000.0); }
        case 8u: { return vec2<f32>(-300000.0, 300000.0); }
        default: { return vec2<f32>(0.0, 1.0); }
    }
}

// `eval::clamped_field` on a sampled (value, gradient): out of range gives
// the constant bound.
fn lf_clamp_field(field: u32, f: vec4<f32>) -> vec4<f32> {
    let range = lf_field_range(field);
    if f.x < range.x {
        return vec4<f32>(range.x, vec3<f32>(0.0));
    }
    if f.x > range.y {
        return vec4<f32>(range.y, vec3<f32>(0.0));
    }
    return f;
}

// ---------------------------------------------------------------- weights

// cube_bicubic_on with cubic B-spline weights for the four lanes
// (`CubeMap::bspline_on`).
fn lf4_bspline_on(face: u32, u: f32, v: f32) -> vec4<f32> {
    let n = f32(tile.noise.y);
    let fx = (u + 1.0) * 0.5 * n - 0.5;
    let fy = (v + 1.0) * 0.5 * n - 0.5;
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let tx = fx - floor(fx);
    let ty = fy - floor(fy);
    let sx = 1.0 - tx;
    let sy = 1.0 - ty;
    let wx = vec4<f32>(sx * sx * sx, 3.0 * tx * tx * tx - 6.0 * tx * tx + 4.0,
        -3.0 * tx * tx * tx + 3.0 * tx * tx + 3.0 * tx + 1.0, tx * tx * tx) / 6.0;
    let wy = vec4<f32>(sy * sy * sy, 3.0 * ty * ty * ty - 6.0 * ty * ty + 4.0,
        -3.0 * ty * ty * ty + 3.0 * ty * ty + 3.0 * ty + 1.0, ty * ty * ty) / 6.0;
    var sum = vec4<f32>(0.0);
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            sum += wx[i] * wy[j] * lf4_across(face, x0 - 1 + i, y0 - 1 + j);
        }
    }
    return sum;
}

// The four landform weights at `d` (`MapsFields::weights`): the Tier A weight
// run (rules evaluated per texel in the bake) as a cubic B-spline, which
// never overshoots and keeps normalised weights summing to one.
fn lf_weights(d: vec3<f32>) -> vec4<f32> {
    lf4_ids = vec4<u32>(9u, 10u, 11u, 12u);
    let band = 1.0 / f32(tile.noise.y);
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var f = 0u; f < 6u; f = f + 1u) {
        let b = face_basis(f);
        let w = dot(d, b[0]);
        if w <= 0.0 {
            continue;
        }
        let u = dot(d, b[1]) / w;
        let v = dot(d, b[2]) / w;
        let t = (1.0 - max(abs(u), abs(v)) + band) / (2.0 * band);
        if t <= 0.0 {
            continue;
        }
        let value = lf4_bspline_on(f, u, v);
        if t >= 1.0 {
            return value;
        }
        let weight = t * t * (3.0 - 2.0 * t);
        sum += weight * value;
        total += weight;
    }
    return sum / max(total, 1.0e-30);
}

// ---------------------------------------------------------------- stacks

fn lf_shape(kind: u32, sharpness: f32, n: f32, g: vec3<f32>) -> vec4<f32> {
    if kind == 0u {
        return vec4<f32>(n, g);
    }
    // Rounded crease `√(n² + k²) − k` (`eval::CREASE_ROUNDING` = 0.08).
    let s = sqrt(n * n + 0.0064);
    let a = s - 0.08;
    let da = n / s;
    if kind == 2u {
        return vec4<f32>(a, g * da);
    }
    let r = 1.0 - a;
    if r <= 0.0 {
        return vec4<f32>(0.0);
    }
    let v = pow(r, sharpness);
    return vec4<f32>(v, g * (-da * sharpness * v / r));
}

// Unit-normalised gain-½ fBm (`eval::warp_fbm`) at chart-local `local`.
fn lf_warp_fbm(local: vec3<f32>, ladder: u32, level: i32, octaves: u32, seed: u32) -> vec4<f32> {
    let anchor = tile_anchor_cell(ladder);
    let residual = tile_anchor_residual(ladder);
    var total = 0.0;
    for (var k = 0u; k < octaves; k = k + 1u) {
        total += ldexp(1.0, -i32(k));
    }
    var sum = vec4<f32>(0.0);
    for (var k = 0u; k < octaves; k = k + 1u) {
        let lv = level - i32(k);
        let w = octave_weight(ladder_frequency(ladder, lv), tile.scale.y);
        if w <= 0.0 {
            continue;
        }
        let a = ldexp(1.0, -i32(k)) / total * w;
        sum += ladder_noise3(anchor, residual, ladder, lv, ladder_octave_seed(seed, ladder, lv), local) * a;
    }
    return sum;
}

// Gully kernel (`eval::gully`) of one octave at chart-local `q`, stripes
// along unit `contour`: value and gradient per metre.
fn lf_gully(q: vec3<f32>, ladder: u32, level: i32, seed: u32, contour: vec3<f32>) -> vec4<f32> {
    let split = ladder_split(tile_anchor_cell(ladder), tile_anchor_residual(ladder), ladder, level, q);
    let x_all = split.q + ladder_octave_offset(seed);
    let fl = floor(x_all);
    let cell = split.cell + vec3<i32>(fl);
    let x = x_all - fl;
    let r2 = 1.25 * 1.25;
    var num = 0.0;
    var den = 0.0;
    var dnum = vec3<f32>(0.0);
    var dden = vec3<f32>(0.0);
    for (var dz = -1; dz <= 1; dz = dz + 1) {
        for (var dy = -1; dy <= 1; dy = dy + 1) {
            for (var dx = -1; dx <= 1; dx = dx + 1) {
                let o = vec3<i32>(dx, dy, dz);
                let bits = lattice_bits(cell + o, seed);
                let u = vec3<f32>(bits) * (1.0 / 4294967296.0);
                let d = x - (vec3<f32>(o) + vec3<f32>(0.5) + (u - vec3<f32>(0.5)) * 0.4);
                let d2 = dot(d, d);
                if d2 >= r2 {
                    continue;
                }
                let t = 1.0 - d2 / r2;
                let kernel = t * t;
                let dkernel = d * (-4.0 * t / r2);
                let phase = 6.2831855 * dot(d, contour);
                let s = sin(phase);
                let c = cos(phase);
                num += kernel * c;
                den += kernel;
                dnum += dkernel * c + contour * (-s * 6.2831855 * kernel);
                dden += dkernel;
            }
        }
    }
    if den <= 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(num / den, (dnum * den - dden * num) / (den * den) * split.freq);
}

// One stack op (`eval::evaluate_stack`) at chart-local `local`; constants at
// word `at` (`gpu::pack_stack`).
fn lf_stack(at: u32, local: vec3<f32>, amplitude: f32, body_seed: u32) -> vec4<f32> {
    let w0 = lf_word(at);
    let kind = w0 & 255u;
    let flags = w0 >> 8u;
    let ladder = lf_word(at + 1u);
    let base_level = lf_i32(at + 2u);
    let octaves = min(lf_word(at + 3u), 24u);
    let gain = lf_f32(at + 4u);
    let salt = lf_word(at + 5u);
    let sharpness = lf_f32(at + 6u);
    let mean3 = lf_f32(at + 7u);
    let mean4 = lf_f32(at + 8u);
    let radius = tile.scale.x;
    // Warp: q = p + d(p); rows[c] = ∇d_c.
    var q = local;
    var rows = array<vec3<f32>, 3>(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    if (flags & 2u) != 0u {
        let strength = lf_f32(at + 13u);
        for (var c = 0u; c < 3u; c = c + 1u) {
            let seed = lf_node_seed(body_seed, lf_word(at + 17u), 1u + c);
            let v = lf_warp_fbm(local, lf_word(at + 14u), lf_i32(at + 15u), lf_word(at + 16u), seed);
            q[c] += strength * v.x;
            rows[c] = v.yzw * strength;
        }
    }
    let normal = normalize(tile.n0.xyz + q / radius);
    // Boundary and hardness at the (warped) stack position, one 4-lane pass.
    var stack_fields: array<vec4<f32>, 4>;
    if (flags & 9u) != 0u {
        stack_fields = lf4_field(vec4<u32>(8u, 3u, 99u, 99u), normal);
    }
    let aniso = (flags & 1u) != 0u;
    let stretch_log2 = lf_word(at + 9u);
    let kappa = lf_f32(at + 10u);
    let aniso_octaves = lf_word(at + 11u);
    var delta = vec4<f32>(0.0);
    if aniso {
        let clamp_m = lf_f32(at + 12u);
        delta = lf_clamp_field(8u, stack_fields[0]);
        if abs(delta.x) > clamp_m {
            delta = vec4<f32>(sign(delta.x) * clamp_m, vec3<f32>(0.0));
        }
    }
    let relief_seed = lf_node_seed(body_seed, salt, 0u);
    let anchor = tile_anchor_cell(ladder);
    let residual = tile_anchor_residual(ladder);
    let stretch = f32(1u << stretch_log2);
    var total = 0.0;
    for (var k = 0u; k < octaves; k = k + 1u) {
        total += pow(gain, f32(k));
    }
    var value = 0.0;
    var gradient = vec3<f32>(0.0);
    for (var k = 0u; k < octaves; k = k + 1u) {
        let level = base_level - i32(k);
        let f = ladder_frequency(ladder, level);
        let four = aniso && k < aniso_octaves;
        let f_eff = select(f, f * sqrt(1.0 / (stretch * stretch) + kappa * kappa), four);
        let w = octave_weight(f_eff, tile.scale.y);
        if w <= 0.0 {
            lf_contrib[k] = vec4<f32>(f_eff, vec3<f32>(0.0));
            continue;
        }
        let seed = ladder_octave_seed(relief_seed, ladder, level);
        var n = 0.0;
        var g = vec3<f32>(0.0);
        if four {
            let fk = f * kappa;
            let qw = fk * delta.x;
            let qw_floor = floor(qw);
            let n4 = ladder_noise4(anchor, residual, ladder, level + i32(stretch_log2), seed, q,
                i32(qw_floor), qw - qw_floor);
            n = n4.value;
            g = n4.gradient + delta.yzw * (fk * n4.dw);
        } else {
            let n3 = ladder_noise3(anchor, residual, ladder, level, seed, q);
            n = n3.x;
            g = n3.yzw;
        }
        let shaped = lf_shape(kind, sharpness, n, g);
        var damp = 1.0;
        if (flags & 4u) != 0u {
            var steer = vec3<f32>(0.0);
            for (var j = 0u; j < k; j = j + 1u) {
                if lf_contrib[j].x <= 0.5 * f_eff * (1.0 + 1.0e-6) {
                    steer += lf_contrib[j].yzw;
                }
            }
            let slope = amplitude * length(lf_tangential(steer, normal));
            damp = 1.0 / (1.0 + lf_f32(at + 18u) * slope * slope);
        }
        let scale = pow(gain, f32(k)) / total * w * damp;
        value += scale * (shaped.x - select(mean3, mean4, four));
        let contribution = shaped.yzw * scale;
        gradient += contribution;
        lf_contrib[k] = vec4<f32>(f_eff, contribution);
    }
    if (flags & 8u) != 0u {
        let strength = lf_f32(at + 19u);
        let e_ladder = lf_word(at + 20u);
        let e_level = lf_i32(at + 21u);
        let e_octaves = lf_word(at + 22u);
        let e_mean = lf_f32(at + 24u);
        let gully_seed = lf_node_seed(body_seed, lf_word(at + 23u), 4u);
        let hardness = lf_clamp_field(3u, stack_fields[1]);
        // `eval::GULLY_HARDNESS_FADE` = 0.6.
        let fade = vec4<f32>(1.0 - 0.6 * hardness.x, -0.6 * hardness.yzw);
        var gullies = vec3<f32>(0.0);
        for (var k = 0u; k < e_octaves; k = k + 1u) {
            let level = e_level - i32(k);
            let f = ladder_frequency(e_ladder, level);
            let w = octave_weight(f, tile.scale.y);
            if w <= 0.0 {
                break;
            }
            var steer = gullies;
            for (var j = 0u; j < octaves; j = j + 1u) {
                if lf_contrib[j].x <= 0.5 * f * (1.0 + 1.0e-6) {
                    steer += lf_contrib[j].yzw;
                }
            }
            let g_t = lf_tangential(steer, normal);
            let slope = length(g_t);
            if !(slope > 0.0) {
                continue;
            }
            let contour = cross(normal, g_t / slope);
            let s = lf_gully(q, e_ladder, level, ladder_octave_seed(gully_seed, e_ladder, level), contour);
            var cap = 3.4e38;
            if amplitude > 0.0 {
                cap = 1.0 / amplitude;
            }
            let depth = strength / (f * 6.2831855) * min(slope, cap) * w;
            let centred = s.x - e_mean;
            value += depth * fade.x * centred;
            let contribution = (s.yzw * fade.x + fade.yzw * centred) * depth;
            gradient += contribution;
            gullies += contribution;
        }
    }
    // Pull back through the warp: ∇_p = ∇_q + Σ_c (∇_q)_c ∇d_c.
    let pulled = gradient + rows[0] * gradient.x + rows[1] * gradient.y + rows[2] * gradient.z;
    return vec4<f32>(value, pulled);
}

// ---------------------------------------------------------------- programs

// Monotone cubic curve (`ir::CurveOp::evaluate`) with `n` points at `at`.
fn lf_curve(at: u32, n: u32, x: f32) -> vec2<f32> {
    let xs = at;
    let ys = at + n;
    let ts = at + 2u * n;
    if x <= lf_f32(xs) {
        return vec2<f32>(lf_f32(ys), 0.0);
    }
    if x >= lf_f32(xs + n - 1u) {
        return vec2<f32>(lf_f32(ys + n - 1u), 0.0);
    }
    var i = 0u;
    for (var j = 0u; j + 1u < n; j = j + 1u) {
        if lf_f32(xs + j) <= x {
            i = j;
        }
    }
    let x0 = lf_f32(xs + i);
    let h = lf_f32(xs + i + 1u) - x0;
    let t = (x - x0) / h;
    let t2 = t * t;
    let t3 = t2 * t;
    let y0 = lf_f32(ys + i);
    let y1 = lf_f32(ys + i + 1u);
    let m0 = lf_f32(ts + i);
    let m1 = lf_f32(ts + i + 1u);
    let value = (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * m0
        + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * h * m1;
    let dy = (y1 - y0) / h;
    let a = -6.0 * dy + 3.0 * m0 + 3.0 * m1;
    let b = 6.0 * dy - 4.0 * m0 - 2.0 * m1;
    return vec2<f32>(value, a * t2 + b * t + m0);
}

// Landform program `index` in metres (`Program::evaluate`).
fn lf_program(index: u32, local: vec3<f32>, d: vec3<f32>) -> vec4<f32> {
    let entry = 4u + 4u * index;
    let amplitude = lf_f32(entry);
    let body_seed = lf_word(entry + 1u);
    var at = lf_word(entry + 2u);
    let count = min(lf_word(entry + 3u), 16u);
    for (var k = 0u; k < count; k = k + 1u) {
        let header = lf_word(at);
        at = at + 1u;
        let opcode = header & 255u;
        let a = lf_reg[min((header >> 8u) & 255u, 15u)];
        let b = lf_reg[min((header >> 16u) & 255u, 15u)];
        let c = lf_reg[min(header >> 24u, 15u)];
        var r = vec4<f32>(0.0);
        switch opcode {
            case 1u: {
                r = lf_stack(at, local, amplitude, body_seed);
                at = at + 25u;
            }
            case 2u: {
                let id = min((header >> 8u) & 255u, 8u);
                r = lf_clamp_field(id, lf_cache[id]);
            }
            case 3u: {
                r = vec4<f32>(lf_f32(at), vec3<f32>(0.0));
                at = at + 1u;
            }
            case 4u: { r = a + b; }
            case 5u: { r = vec4<f32>(a.x * b.x, a.yzw * b.x + b.yzw * a.x); }
            case 6u: {
                r = vec4<f32>(a.x + (b.x - a.x) * c.x,
                    a.yzw + (b.yzw - a.yzw) * c.x + c.yzw * (b.x - a.x));
            }
            case 7u: { r = select(a, b, b.x < a.x); }
            case 8u: { r = select(a, b, b.x > a.x); }
            case 9u: {
                let kk = lf_f32(at);
                at = at + 1u;
                let h = max(kk - abs(a.x - b.x), 0.0) / kk;
                let lo = select(b, a, a.x <= b.x);
                let hi = select(a, b, a.x <= b.x);
                r = vec4<f32>(lo.x - h * h * kk * 0.25, lo.yzw * (1.0 - 0.5 * h) + hi.yzw * (0.5 * h));
            }
            case 10u: {
                let lo = lf_f32(at);
                let hi = lf_f32(at + 1u);
                at = at + 2u;
                r = a;
                if a.x < lo {
                    r = vec4<f32>(lo, vec3<f32>(0.0));
                } else if a.x > hi {
                    r = vec4<f32>(hi, vec3<f32>(0.0));
                }
            }
            case 11u: {
                let n = lf_word(at);
                let y = lf_curve(at + 1u, n, a.x);
                at = at + 1u + 3u * n;
                r = vec4<f32>(y.x, a.yzw * y.y);
            }
            default: {
                r = a * lf_f32(at);
                at = at + 1u;
            }
        }
        lf_reg[k] = r;
    }
    return lf_reg[count - 1u] * amplitude;
}

// Landform relief Σ wᵢ·landformᵢ (metres, gradient per metre) at chart-local
// `local` (direction `d`); zero without a landform block.
fn landform_relief(local: vec3<f32>, d: vec3<f32>) -> vec4<f32> {
    lf_base = SURFACE_LUT + world_surface[0] * world_surface[0];
    let count = lf_word(0u);
    if tile.info.y != 2u || tile.noise.y == 0u || count == 0u {
        return vec4<f32>(0.0);
    }
    lf_fill_cache(d, lf_word(1u));
    lf_cache_valid = true;
    let weights = lf_weights(d);
    var sum = vec4<f32>(0.0);
    for (var i = 0u; i < min(count, 4u); i = i + 1u) {
        let w = weights[i];
        if w <= 0.0 {
            continue;
        }
        let h = lf_program(i, local, d);
        sum += vec4<f32>(w * h.x, h.yzw * w);
    }
    return sum;
}

// Shape overlay texel (uplift, hardness, sediment, flow) from the field
// cache of this sample; zero when no landforms were evaluated.
fn page_shape() -> vec4<f32> {
    if !lf_cache_valid {
        return vec4<f32>(0.0);
    }
    return clamp(
        vec4<f32>(lf_cache[0].x, lf_cache[3].x, lf_cache[1].x, lf_cache[2].x),
        vec4<f32>(0.0),
        vec4<f32>(1.0),
    );
}
