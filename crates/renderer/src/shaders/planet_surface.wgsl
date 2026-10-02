struct Projection { matrix: mat4x4<f32> }
struct Sample { position: vec4<f32>, normal: vec4<f32> }
struct Instance { data: vec4<u32>, color: vec4<f32>, padding0: vec4<u32>, padding1: vec4<u32> }
// Surface -> sun in body-fixed axes. strengths = diffuse, mode, sRGB target, unused.
struct Lighting { sun_ambient: vec4<f32>, strengths: vec4<f32> }
@group(0) @binding(0) var<uniform> projection: Projection;
@group(1) @binding(0) var<storage, read> samples: array<Sample>;
@group(1) @binding(1) var<storage, read> instances: array<Instance>;
@group(1) @binding(2) var<uniform> lighting: Lighting;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) borders: u32,
    @location(4) elevation: f32,
}
@vertex fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) index: u32) -> Output {
    let instance = instances[index];
    let sample = samples[instance.data.x + vertex];
    var output: Output;
    output.position = projection.matrix * sample.position;
    output.normal = sample.normal.xyz;
    output.color = instance.color;
    output.uv = vec2<f32>(f32(vertex % 17u), f32(vertex / 17u)) / 16.0;
    output.borders = instance.data.w;
    output.elevation = sample.normal.w;
    return output;
}
@vertex fn vs_clipped(@location(0) clip: vec4<f32>, @location(1) normal: vec4<f32>, @location(2) color: vec4<f32>, @location(3) uv_flags: vec4<f32>) -> Output {
    var output: Output;
    output.position = clip;
    output.normal = normal.xyz;
    output.color = color;
    output.uv = uv_flags.xy;
    output.borders = u32(uv_flags.z);
    output.elevation = normal.w;
    return output;
}
// The debug elevation palette is display/sRGB authored, not a material system.
fn decode_srgb(color: vec3<f32>) -> vec3<f32> {
    return select(pow((color + 0.055) / 1.055, vec3<f32>(2.4)), color / 12.92, color <= vec3<f32>(0.04045));
}
fn encode_srgb(color: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055, 12.92 * color, color <= vec3<f32>(0.0031308));
}
@fragment fn fs_main(input: Output) -> @location(0) vec4<f32> {
    // Re-normalize after interpolation; degenerate diagnostics stay finite.
    let normal = input.normal * inverseSqrt(max(dot(input.normal, input.normal), 1.0e-20));
    let legacy_shade = 0.2 + 0.8 * max(dot(normal, normalize(vec3<f32>(0.3, 0.6, 1.0))), 0.0);
    let distance = min(input.uv, vec2<f32>(1.0) - input.uv);
    let width = max(fwidth(input.uv), vec2<f32>(0.000001));
    let border = min(distance.x / width.x, distance.y / width.y);
    var base = input.color.rgb;
    if (input.borders & 2u) != 0u {
        base = mix(vec3<f32>(0.04, 0.12, 0.4), vec3<f32>(0.5, 0.65, 0.25), clamp(0.5 + 2.0 * input.elevation, 0.0, 1.0));
    }
    var color = base * legacy_shade;
    if (input.borders & 4u) != 0u {
        let diffuse = max(dot(normal, normalize(lighting.sun_ambient.xyz)), 0.0);
        let mode = u32(lighting.strengths.y);
        color = decode_srgb(base);
        if mode == 1u {
            color *= lighting.sun_ambient.w + lighting.strengths.x * diffuse;
        } else if mode == 2u {
            color = decode_srgb(clamp(normal * 0.5 + vec3<f32>(0.5), vec3<f32>(0.0), vec3<f32>(1.0)));
        } else if mode == 3u {
            // Raw diffuse is displayed as a linear grayscale diagnostic.
            color = vec3<f32>(diffuse);
        }
        if lighting.strengths.z == 0.0 { color = encode_srgb(color); }
    }
    if (input.borders & 1u) != 0u && border < 1.0 { color = mix(vec3<f32>(0.03), color, clamp(border, 0.0, 1.0)); }
    return vec4<f32>(color, input.color.a);
}
