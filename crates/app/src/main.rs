#![deny(unsafe_code)]

//! Mundaris Studio: the native entry point. winit owns the window and event
//! loop, egui draws the Studio UI, and the engine renders the scene into a
//! texture shown in the viewport on one shared wgpu device (docs/STUDIO_UI.md §3).

use anyhow::{Context, Result, anyhow};
use mundaris_app::{GravityOrbitsDemo, studio::view::StudioAction};
use mundaris_renderer::{GpuContext, Renderer};
use tracing::info;
use tracing_subscriber::EnvFilter;
use winit::event_loop::{ControlFlow, EventLoop};

mod profiler_ui;
mod studio_ui;

use studio_ui::{AppEvent, Engine, StudioApp};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,wgpu_hal=warn")),
        )
        .try_init()
        .map_err(|error| anyhow!("initializing structured logging: {error}"))?;

    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("usage: mundaris_app [--dev-interface]");
        return Ok(());
    }
    let mut dev_interface = false;
    for argument in &arguments {
        match argument.as_str() {
            "--dev-interface" if !dev_interface => dev_interface = true,
            "--dev-interface" => anyhow::bail!("duplicate --dev-interface"),
            other => {
                anyhow::bail!("unknown argument: {other}; usage: mundaris_app [--dev-interface]")
            }
        }
    }
    let dev_interface = dev_interface
        || cfg!(feature = "developer-tools")
            && (std::env::var("MUNDARIS_PERFORMANCE_LAB").is_ok_and(|value| value == "1")
                || std::env::var_os("MUNDARIS_CAPTURE_DIR").is_some());
    #[cfg(not(feature = "developer-tools"))]
    anyhow::ensure!(
        !dev_interface,
        "--dev-interface requires the developer-tools build feature"
    );

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("creating the Studio event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let gpu = GpuContext::new().context("initializing the GPU")?;
    let renderer = Renderer::new(&gpu);
    let demo = GravityOrbitsDemo::shared_test_system()?;
    #[allow(unused_mut)]
    let mut engine = Engine::new(demo, renderer);
    #[cfg(feature = "developer-tools")]
    if dev_interface {
        // Developer observation suppresses automatic GPU timestamp sampling.
        engine.renderer.set_developer_observation_mode(true);
        let proxy = event_loop.create_proxy();
        let service = mundaris_app::developer_service::DeveloperService::start(
            "test-solar-system",
            move || {
                let _ = proxy.send_event(AppEvent::DeveloperWake);
            },
        )?;
        engine
            .demo
            .developer_set_session(&service.descriptor.session_id);
        engine.developer = Some(service);
    }
    engine.demo.reset_wall_capture();
    // The Studio records CPU spans by default so the profiler timeline is live.
    engine
        .demo
        .studio_action(StudioAction::ProfilerEnabled(true));

    let mut app = StudioApp::new(gpu, engine);
    event_loop
        .run_app(&mut app)
        .map_err(studio_ui::loop_error)?;
    app.finish()?;
    info!("Mundaris shut down cleanly");
    Ok(())
}
