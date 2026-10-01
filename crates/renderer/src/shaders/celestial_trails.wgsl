struct Projection { matrix: mat4x4<f32> }
@group(0) @binding(0) var<uniform> projection: Projection;
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
}
@vertex fn vs_main(@location(0) view_position: vec4<f32>, @location(1) color: vec4<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = projection.matrix * view_position;
    output.color = color;
    return output;
}
@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> { return input.color; }
