struct Projection { matrix: mat4x4<f32> }
struct Sample { position: vec4<f32>, normal: vec4<f32> }
struct Instance { data: vec4<u32>, color: vec4<f32>, padding0: vec4<u32>, padding1: vec4<u32> }
@group(0) @binding(0) var<uniform> projection: Projection;
@group(1) @binding(0) var<storage, read> samples: array<Sample>;
@group(1) @binding(1) var<storage, read> instances: array<Instance>;
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
@fragment fn fs_main(input: Output) -> @location(0) vec4<f32> {
    let shade = 0.2 + 0.8 * max(dot(normalize(input.normal), normalize(vec3<f32>(0.3, 0.6, 1.0))), 0.0);
    let distance = min(input.uv, vec2<f32>(1.0) - input.uv);
    let width = max(fwidth(input.uv), vec2<f32>(0.000001));
    let border = min(distance.x / width.x, distance.y / width.y);
    var color = input.color.rgb * shade;
    if (input.borders & 2u) != 0u {
        color = mix(vec3<f32>(0.04, 0.12, 0.4), vec3<f32>(0.5, 0.65, 0.25), clamp(0.5 + 2.0 * input.elevation, 0.0, 1.0)) * shade;
    }
    if (input.borders & 1u) != 0u && border < 1.0 { color = mix(vec3<f32>(0.03), color, clamp(border, 0.0, 1.0)); }
    return vec4<f32>(color, input.color.a);
}
