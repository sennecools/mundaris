// Ground-truth ambient occlusion (Jimenez et al. 2016) over infinite
// reverse-Z depth and octahedral view normals. Output: AO, linear view depth.

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var normal_tex: texture_2d<f32>;

const SLICES: i32 = 3;
const STEPS: i32 = 6;

fn decode_normal(f: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    let t = clamp(-n.z, 0.0, 1.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}

// Infinite reverse-Z stores near / view depth.
fn depth_at(pixel: vec2<i32>) -> f32 {
    let d = textureLoad(depth_tex, pixel, 0);
    return select(-1.0, post.camera.x / d, d > 0.0);
}

fn view_position(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let c = post.size.xy * 0.5;
    let focal = post.camera.y;
    return vec3<f32>((pixel.x - c.x) * depth / focal, -(pixel.y - c.y) * depth / focal, -depth);
}

@fragment
fn fs_gtao(input: FullscreenOut) -> @location(0) vec4<f32> {
    let scale = post.size.xy / post.size.zw;
    let full = vec2<i32>(input.position.xy * scale);
    let dims = vec2<i32>(post.size.xy);
    let depth = depth_at(full);
    if depth <= 0.0 {
        return vec4<f32>(1.0, 0.0, 0.0, 0.0);
    }
    let pixel = vec2<f32>(full) + vec2<f32>(0.5);
    let p = view_position(pixel, depth);
    let n = decode_normal(textureLoad(normal_tex, full, 0).xy);
    let v = normalize(-p);
    let radius = max(post.ao.x, depth * post.ao.y);
    let focal = post.camera.y;
    let screen_radius = min(radius * focal / depth, 0.15 * post.size.y);
    if screen_radius < 1.5 {
        return vec4<f32>(1.0, depth, 0.0, 0.0);
    }
    // 4×4 ordered pattern (Bayer) for slice rotation and step jitter; the
    // composite's 4×4 average cancels it exactly.
    let cell = vec2<u32>(input.position.xy) & vec2<u32>(3u);
    var bayer = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    let noise = (f32(bayer[cell.y * 4u + cell.x]) + 0.5) / 16.0;
    let jitter = fract(noise * 4.0 + 0.125);
    var visibility = 0.0;
    for (var s = 0; s < SLICES; s++) {
        let phi = (f32(s) + noise) / f32(SLICES) * PI;
        let omega = vec2<f32>(cos(phi), -sin(phi));
        let dir = vec3<f32>(cos(phi), sin(phi), 0.0);
        let d = normalize(dir - dot(dir, v) * v);
        let axis = cross(d, v);
        let proj_n = n - axis * dot(n, axis);
        let proj_len = length(proj_n);
        if proj_len < 1.0e-4 {
            visibility += 1.0;
            continue;
        }
        let cos_n = clamp(dot(proj_n, v) / proj_len, -1.0, 1.0);
        let gamma = sign(dot(proj_n, d)) * acos(cos_n);
        var horizon = vec2<f32>(-1.0, -1.0);
        for (var side = 0; side < 2; side++) {
            let towards = select(-1.0, 1.0, side == 1);
            for (var j = 0; j < STEPS; j++) {
                var t = (f32(j) + jitter) / f32(STEPS);
                t = t * t;
                let offset = omega * towards * max(t * screen_radius, 1.0 + f32(j));
                let sp = pixel + offset;
                let st = vec2<i32>(floor(sp));
                if any(st < vec2<i32>(0)) || any(st >= dims) {
                    break;
                }
                let sd = depth_at(st);
                if sd <= 0.0 {
                    continue;
                }
                let delta = view_position(vec2<f32>(st) + vec2<f32>(0.5), sd) - p;
                let len = length(delta);
                if len < 1.0e-6 {
                    continue;
                }
                let weight = clamp(1.0 - (len * len) / (radius * radius), 0.0, 1.0);
                let h = mix(-1.0, dot(delta / len, v), weight);
                if side == 1 {
                    horizon.y = max(horizon.y, h);
                } else {
                    horizon.x = max(horizon.x, h);
                }
            }
        }
        var h0 = -acos(clamp(horizon.x, -1.0, 1.0));
        var h1 = acos(clamp(horizon.y, -1.0, 1.0));
        h0 = gamma + max(h0 - gamma, -0.5 * PI);
        h1 = gamma + min(h1 - gamma, 0.5 * PI);
        let sin_g = sin(gamma);
        let arc0 = 0.25 * (cos_n + 2.0 * h0 * sin_g - cos(2.0 * h0 - gamma));
        let arc1 = 0.25 * (cos_n + 2.0 * h1 * sin_g - cos(2.0 * h1 - gamma));
        visibility += proj_len * (arc0 + arc1);
    }
    let ao = pow(clamp(visibility / f32(SLICES), 0.0, 1.0), max(post.ao.z, 0.0));
    return vec4<f32>(ao, depth, 0.0, 0.0);
}
