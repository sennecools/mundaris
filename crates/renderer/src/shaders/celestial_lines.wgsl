// Anti-aliased screen-space guide lines, drawn after tonemapping in display
// space. `edge.x` is the signed distance from the centre line in pixels and
// `edge.y` the half width; coverage fades over one pixel at each edge.
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) edge: vec2<f32>,
}
@vertex fn vs_main(
    @location(0) clip_position: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) edge: vec4<f32>,
) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = clip_position;
    output.color = color;
    output.edge = edge.xy;
    return output;
}
@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let coverage = clamp(input.edge.y + 0.5 - abs(input.edge.x), 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}
