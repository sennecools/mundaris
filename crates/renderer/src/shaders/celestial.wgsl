struct Projection { matrix: mat4x4<f32> }
struct Draw { color: vec4<f32>, flags: vec4<u32> }
@group(0) @binding(0) var<uniform> projection: Projection;
@group(1) @binding(0) var<uniform> draw: Draw;
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
}
@vertex fn vs_main(@location(0) view_position: vec4<f32>, @location(1) view_normal: vec4<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = projection.matrix * view_position;
    output.normal = view_normal.xyz;
    return output;
}
@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    var shade = 1.0;
    if draw.flags.x == 0u {
        shade = 0.2 + 0.8 * max(dot(normalize(input.normal), normalize(vec3<f32>(0.3, 0.6, 1.0))), 0.0);
    }
    return vec4<f32>(draw.color.rgb * shade, draw.color.a);
}
