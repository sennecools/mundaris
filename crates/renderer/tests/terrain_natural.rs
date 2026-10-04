#[test]
fn natural_terrain_shader_validates_and_decodes_radial_direction_before_interpolation() {
    let source = include_str!("../src/shaders/planet_surface.wgsl");
    let module = naga::front::wgsl::parse_str(source).expect("planet surface WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("planet surface WGSL validates");

    assert!(source.contains("@location(6) body_direction: vec3<f32>"));
    assert!(source.contains("output.body_direction = decode_octahedral(sample.classification.zw)"));
    assert!(source.contains("let body_direction = normalize(input.body_direction)"));
    assert!(!source.contains("decode_octahedral(input.classification.zw)"));
    assert!(source.contains("if (input.borders & 8u) != 0u"));
    assert!(source.contains("let detail_width = length(fwidth(body_direction))*71.0"));
    assert!(source.contains("fine * (1.0-smoothstep(0.25,0.8,detail_width))"));
}
