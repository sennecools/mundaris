// Cube-map field sampling, a port of crates/world/src/terrain/world_map/cube.rs
// (the CPU test oracle). Faces follow CubeFace::ALL; each face is n × n,
// row-major with rows along +v. A field is a run of 6·n² values starting at
// `base` in a storage buffer; the including shader defines
// `cube_field(index) -> f32` and `cube_n() -> u32`.

fn face_basis(face: u32) -> mat3x3<f32> {
    switch face {
        case 0u: { return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(0.0, 1.0, 0.0)); }
        case 1u: { return mat3x3<f32>(vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0)); }
        case 2u: { return mat3x3<f32>(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, -1.0)); }
        case 3u: { return mat3x3<f32>(vec3<f32>(0.0, -1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0)); }
        case 4u: { return mat3x3<f32>(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0)); }
        default: { return mat3x3<f32>(vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0)); }
    }
}

fn cube_texel_direction(face: u32, i: u32, j: u32, n: u32) -> vec3<f32> {
    let b = face_basis(face);
    let u = -1.0 + (2.0 * f32(i) + 1.0) / f32(n);
    let v = -1.0 + (2.0 * f32(j) + 1.0) / f32(n);
    return normalize(b[0] + u * b[1] + v * b[2]);
}

struct Located {
    face: u32,
    u: f32,
    v: f32,
}

// Face whose normal has the largest dot product (ties to the earlier face).
fn cube_locate(d: vec3<f32>) -> Located {
    var best = 0u;
    var best_dot = -2.0;
    for (var f = 0u; f < 6u; f = f + 1u) {
        let dot_f = dot(d, face_basis(f)[0]);
        if dot_f > best_dot {
            best = f;
            best_dot = dot_f;
        }
    }
    let b = face_basis(best);
    var out: Located;
    out.face = best;
    out.u = clamp(dot(d, b[1]) / best_dot, -1.0, 1.0);
    out.v = clamp(dot(d, b[2]) / best_dot, -1.0, 1.0);
    return out;
}

fn cube_index(face: u32, i: u32, j: u32, n: u32) -> u32 {
    return (face * n + j) * n + i;
}

// Texel (i, j) of `face`; outside the face, the adjacent face is sampled
// bilinearly (clamped within it) at the extended chart position.
fn cube_value_across(base: u32, face: u32, i: i32, j: i32) -> f32 {
    let n = cube_n();
    let ni = i32(n);
    if i >= 0 && i < ni && j >= 0 && j < ni {
        return cube_field(base + cube_index(face, u32(i), u32(j), n));
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
    let a = cube_field(base + cube_index(at.face, x0, y0, n));
    let bb = cube_field(base + cube_index(at.face, x1, y0, n));
    let c = cube_field(base + cube_index(at.face, x0, y1, n));
    let dd = cube_field(base + cube_index(at.face, x1, y1, n));
    return (a * (1.0 - tx) + bb * tx) * (1.0 - ty) + (c * (1.0 - tx) + dd * tx) * ty;
}

fn cube_bilinear_on(base: u32, face: u32, u: f32, v: f32) -> f32 {
    let n = f32(cube_n());
    let fx = (u + 1.0) * 0.5 * n - 0.5;
    let fy = (v + 1.0) * 0.5 * n - 0.5;
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let tx = fx - floor(fx);
    let ty = fy - floor(fy);
    let top = cube_value_across(base, face, x0, y0) * (1.0 - tx) + cube_value_across(base, face, x0 + 1, y0) * tx;
    let bottom = cube_value_across(base, face, x0, y0 + 1) * (1.0 - tx) + cube_value_across(base, face, x0 + 1, y0 + 1) * tx;
    return top * (1.0 - ty) + bottom * ty;
}

fn catmull_rom(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4<f32>(0.5 * (-t3 + 2.0 * t2 - t), 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t), 0.5 * (t3 - t2));
}

fn cube_bicubic_on(base: u32, face: u32, u: f32, v: f32) -> f32 {
    let n = f32(cube_n());
    let fx = (u + 1.0) * 0.5 * n - 0.5;
    let fy = (v + 1.0) * 0.5 * n - 0.5;
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let wx = catmull_rom(fx - floor(fx));
    let wy = catmull_rom(fy - floor(fy));
    var sum = 0.0;
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            sum += wx[i] * wy[j] * cube_value_across(base, face, x0 - 1 + i, y0 - 1 + j);
        }
    }
    return sum;
}

// Blend over every face whose extended chart lies within one texel of `d`,
// weighted by the distance inside that face's square (exactly continuous
// across edges). `cubic` selects Catmull-Rom over bilinear.
fn cube_sample(base: u32, d: vec3<f32>, cubic: bool) -> f32 {
    let band = 1.0 / f32(cube_n());
    var sum = 0.0;
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
        var value: f32;
        if cubic {
            value = cube_bicubic_on(base, f, u, v);
        } else {
            value = cube_bilinear_on(base, f, u, v);
        }
        if t >= 1.0 {
            return value;
        }
        let weight = t * t * (3.0 - 2.0 * t);
        sum += weight * value;
        total += weight;
    }
    return sum / max(total, 1.0e-30);
}
