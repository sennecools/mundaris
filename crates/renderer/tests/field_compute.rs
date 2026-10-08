use mundaris_renderer::field_compute::{
    FieldComputeJob, FieldComputeService, MAX_FIELD_OUTPUT_BYTES,
};

const AFFINE_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read> parameters: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<vec4<f32>>;

@compute @workgroup_size(4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= arrayLength(&output)) {
        return;
    }
    let value = input[id.x] * parameters[0] + parameters[1];
    output[id.x] = vec4<f32>(value, value + 1.0, value + 2.0, value + 3.0);
}
"#;

#[test]
fn affine_field_shader_parses_and_validates() {
    let module = naga::front::wgsl::parse_str(AFFINE_SHADER).expect("WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("WGSL validates");
}

#[test]
fn output_budget_is_a_fixed_bounded_cap() {
    assert_eq!(MAX_FIELD_OUTPUT_BYTES, 16 * 1024 * 1024);
}

#[test]
#[ignore = "requires a real GPU adapter; verifies bounded async field dispatch and explicit readback"]
fn affine_field_page_dispatches_and_reads_back_only_on_request() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .expect("GPU adapter required for field compute experiment");
        println!("adapter={:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Field compute experiment"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .expect("device creation");

        let service = FieldComputeService;
        let input = [0.0, 0.5, 1.0, 2.0, -1.0];
        let parameters = [2.0, 0.25];
        let page = service
            .dispatch(
                &device,
                &queue,
                FieldComputeJob {
                    wgsl: AFFINE_SHADER,
                    input: &input,
                    parameters: &parameters,
                    output_records: input.len() as u32,
                    workgroup_size_x: 4,
                    generation: 17,
                },
            )
            .await
            .expect("dispatch submission");
        assert_eq!(page.output_records(), input.len() as u32);
        assert_eq!(page.output_bytes(), input.len() as u64 * 16);
        assert_eq!(page.input_bytes(), input.len() as u64 * 4);
        assert_eq!(page.parameter_bytes(), parameters.len() as u64 * 4);
        assert_eq!(page.validation_readback_bytes(), page.output_bytes());
        assert_eq!(page.generation(), 17);
        assert_eq!(page.workgroups_x(), 2);

        let output = service
            .readback_validation(&device, &queue, &page)
            .expect("explicit validation readback");
        assert_eq!(output.len(), input.len());
        for (actual, source) in output.iter().zip(input) {
            let value = source * parameters[0] + parameters[1];
            assert_eq!(*actual, [value, value + 1.0, value + 2.0, value + 3.0]);
        }
    });
}
