//! Diagnostic (ignored by default): first-compile cost of the producer
//! pipelines per entry point, with and without the landform and river
//! terms. Run with `--ignored --nocapture`.
mod common;

#[test]
#[ignore]
fn producer_pipeline_compile_time() {
    let Some(context) = common::gpu() else {
        return;
    };
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        | 1;
    for (variant, name) in [(3, "base"), (2, "+landforms"), (1, "+rivers"), (0, "full")] {
        let times = astrum_renderer::producer_pipeline_compile_seconds(
            &context.device,
            salt.wrapping_add(variant),
            variant,
        );
        let total: f64 = times.iter().map(|t| t.1).sum();
        println!("{name:>11}: total {total:6.2} s  {times:?}");
    }
}
