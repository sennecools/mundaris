// Exposure, bloom blend, display transform and dither into the scene texture.
// The target is an sRGB view, so this pass outputs display-referred linear.

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var hdr_tex: texture_2d<f32>;
@group(0) @binding(2) var bloom_tex: texture_2d<f32>;
@group(0) @binding(3) var linear_clamp: sampler;
@group(0) @binding(4) var<storage, read> exposure: Exposure;
@group(0) @binding(5) var ao_tex: texture_2d<f32>;
@group(0) @binding(6) var depth_tex: texture_depth_2d;

// Minimal AgX (Wrensch 2023, after Sobotka): inset, log2 encode, sigmoid, outset.
fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2
        + 0.1191 * x - vec3<f32>(0.00232);
}

fn agx(c: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        0.842479062253094, 0.0423282422610123, 0.0423756549057051,
        0.0784335999999992, 0.878468636469772, 0.0784336,
        0.0792237451477643, 0.0791661274605434, 0.879142973793104,
    );
    let outset = mat3x3<f32>(
        1.19687900512017, -0.0528968517574562, -0.0529716355144438,
        -0.0980208811401368, 1.15190312990417, -0.0980434501171241,
        -0.0990297440797205, -0.0989611768448433, 1.15107367264116,
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * max(c, vec3<f32>(1.0e-10));
    v = clamp(log2(v), vec3<f32>(min_ev), vec3<f32>(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    v = agx_contrast(v);
    v = outset * v;
    // AgX output is display-encoded (2.2); the sRGB view re-encodes linear.
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2));
}

// Stephen Hill's ACES fitted RRT+ODT (sRGB primaries).
fn aces_fitted(c: vec3<f32>) -> vec3<f32> {
    let input_mat = mat3x3<f32>(
        0.59719, 0.07600, 0.02840,
        0.35458, 0.90834, 0.13383,
        0.04823, 0.01566, 0.83777,
    );
    let output_mat = mat3x3<f32>(
        1.60475, -0.10208, -0.00327,
        -0.53108, 1.10813, -0.07276,
        -0.07367, -0.00605, 1.07602,
    );
    let v = input_mat * c;
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return clamp(output_mat * (a / b), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

fn from_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

// Stops relative to middle grey, -5..+5, as a perceptual ramp.
fn false_color(stops: f32) -> vec3<f32> {
    let t = clamp((stops + 5.0) / 10.0, 0.0, 1.0);
    let ramp = array<vec3<f32>, 6>(
        vec3<f32>(0.05, 0.0, 0.25), vec3<f32>(0.0, 0.25, 0.9), vec3<f32>(0.0, 0.75, 0.55),
        vec3<f32>(0.85, 0.85, 0.1), vec3<f32>(0.95, 0.35, 0.05), vec3<f32>(1.0, 1.0, 1.0),
    );
    let x = t * 5.0;
    let i = min(u32(x), 4u);
    var r = ramp;
    return mix(r[i], r[i + 1u], x - f32(i));
}

@fragment
fn fs_tonemap(input: FullscreenOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(input.position.xy);
    let hdr = textureLoad(hdr_tex, ip, 0).rgb;
    let mode = u32(post.camera.w + 0.5);
    if post.tonemap.w > 0.5 {
        return vec4<f32>(clamp(hdr, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    if mode == VIEW_AO_ONLY {
        // The same denoised AO the composite applies.
        return vec4<f32>(vec3<f32>(filtered_ao(ip)), 1.0);
    }
    var c = hdr;
    if post.bloom.x > 0.5 {
        let bloom = textureSampleLevel(bloom_tex, linear_clamp, input.uv, 0.0).rgb / max(post.bloom.w, 1.0);
        c = mix(c, bloom, post.bloom.y);
    }
    c *= exposure.value / max(exposure.previous, 1.0e-30);
    if mode == VIEW_LUMINANCE {
        return vec4<f32>(false_color(log2(max(luminance(c), 1.0e-6) / 0.18)), 1.0);
    }
    let op = u32(post.tonemap.x + 0.5);
    var out: vec3<f32>;
    if op == 0u {
        out = agx(c);
    } else if op == 1u {
        out = aces_fitted(c);
    } else {
        out = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if post.tonemap.y > 0.5 {
        let noise = ign(input.position.xy, post.tonemap.z) - 0.5;
        out = from_srgb(clamp(to_srgb(out) + noise / 255.0, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    return vec4<f32>(out, 1.0);
}
