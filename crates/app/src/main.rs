#![forbid(unsafe_code)]

mod redraw;

use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result, anyhow};
use mundaris_renderer::Renderer;
use tracing::info;
use tracing_subscriber::EnvFilter;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

enum AppEvent {
    #[cfg(feature = "developer-tools")]
    DeveloperWake,
}

struct MundarisApp {
    #[cfg(feature = "developer-tools")]
    developer: Option<mundaris_app::developer_service::DeveloperService>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    application_error: Option<anyhow::Error>,
    occluded: bool,
    redraw_schedule: redraw::RedrawSchedule,
    gravity_demo: mundaris_app::GravityOrbitsDemo,
}

impl MundarisApp {
    fn new() -> Result<Self> {
        Ok(Self {
            #[cfg(feature = "developer-tools")]
            developer: None,
            window: None,
            renderer: None,
            application_error: None,
            occluded: false,
            redraw_schedule: redraw::RedrawSchedule::default(),
            gravity_demo: mundaris_app::GravityOrbitsDemo::shared_test_system()?,
        })
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.application_error = Some(error);
        event_loop.exit();
    }

    fn drawable(&self) -> bool {
        self.window.as_ref().is_some_and(|window| {
            let size = window.inner_size();
            !self.occluded
                && size.width > 0
                && size.height > 0
                && window.is_minimized() != Some(true)
        })
    }
}

impl ApplicationHandler<AppEvent> for MundarisApp {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: AppEvent) {
        #[cfg(feature = "developer-tools")]
        self.developer_turn(event_loop);
        #[cfg(not(feature = "developer-tools"))]
        let _ = event_loop;
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("Mundaris")
            .with_inner_size(PhysicalSize::new(1280, 800));
        #[cfg(feature = "developer-tools")]
        let attributes = attributes.with_active(self.developer.is_none());
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(
                    event_loop,
                    anyhow::Error::new(error).context("creating the native window"),
                );
                return;
            }
        };
        let renderer = match Renderer::new(Arc::clone(&window)) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(
                    event_loop,
                    anyhow::Error::new(error).context("initializing the renderer"),
                );
                return;
            }
        };

        #[cfg(feature = "developer-tools")]
        let renderer = {
            let mut renderer = renderer;
            renderer.set_developer_observation_mode(self.developer.is_some());
            renderer
        };
        info!("native window and renderer initialized");
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.gravity_demo.reset_wall_capture();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.gravity_demo.set_lifecycle_drawable(false);
        // Release presentation resources before the native window on suspension.
        self.renderer = None;
        self.window = None;
        self.occluded = false;
        self.redraw_schedule = redraw::RedrawSchedule::default();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self
            .window
            .as_ref()
            .filter(|window| window.id() == window_id)
        else {
            return;
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        #[cfg(feature = "developer-tools")]
        if matches!(
            &event,
            // Winit synthesizes keyboard state on Windows focus changes.
            // Those events do not represent a human taking automation control.
            WindowEvent::KeyboardInput {
                is_synthetic: false,
                ..
            } | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
        ) && let Some(service) = &mut self.developer
        {
            let demo = &mut self.gravity_demo;
            let input_kind = match &event {
                WindowEvent::KeyboardInput { .. } => "keyboard",
                WindowEvent::MouseInput { .. } => "mouse_button",
                WindowEvent::MouseWheel { .. } => "mouse_wheel",
                _ => unreachable!(),
            };
            info!(input_kind, "developer session received human input");
            let reason = match &event {
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == winit::event::ElementState::Pressed =>
                {
                    "human_input_keyboard_pressed"
                }
                WindowEvent::KeyboardInput { .. } => "human_input_keyboard_released",
                WindowEvent::MouseInput { .. } => "human_input_mouse_button",
                _ => "human_input_mouse_wheel",
            };
            service.interrupt(demo, renderer, reason);
        }

        let scene_consumed = self.gravity_demo.on_window_event(&event);
        let repaint = if scene_consumed {
            true
        } else {
            renderer.on_window_event(&event)
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
            WindowEvent::Occluded(occluded) => self.occluded = occluded,
            WindowEvent::RedrawRequested => {
                if self.occluded || window.is_minimized() == Some(true) {
                    return;
                }
                let size = window.inner_size();
                // Windows can expose the new client extent before delivering
                // Resized. Projection and the acquired target must agree even
                // when RedrawRequested arrives first.
                renderer.resize(size.width, size.height);
                let result = self.gravity_demo.render(renderer, size.width, size.height);
                if let Err(error) = result {
                    self.fail(event_loop, error.context("rendering a frame"));
                    return;
                }
                #[cfg(feature = "developer-tools")]
                if let Some(service) = &mut self.developer {
                    service.observe(&self.gravity_demo, renderer, true);
                }
            }
            _ => {}
        }

        if repaint && !self.occluded && window.is_minimized() != Some(true) {
            window.request_redraw();
        }
        let drawable = self.drawable();
        self.gravity_demo.set_lifecycle_drawable(drawable);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(feature = "developer-tools")]
        self.developer_turn(event_loop);
        let drawable = self.drawable();
        self.gravity_demo.set_lifecycle_drawable(drawable);
        let (control_flow, request_redraw) =
            self.redraw_schedule.update(Instant::now(), self.drawable());
        event_loop.set_control_flow(control_flow);
        if request_redraw && let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

#[cfg(feature = "developer-tools")]
impl MundarisApp {
    fn developer_turn(&mut self, event_loop: &ActiveEventLoop) {
        let drawable = self.drawable();
        if let (Some(service), Some(renderer)) = (&mut self.developer, &mut self.renderer) {
            service.turn(&mut self.gravity_demo, renderer, drawable);
            if service.shutdown_requested {
                event_loop.exit();
            }
        }
    }
}

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
        .context("creating the native event loop")?;
    let mut app = MundarisApp::new()?;
    #[cfg(feature = "developer-tools")]
    if dev_interface {
        let preset = "test-solar-system";
        let proxy = event_loop.create_proxy();
        let service =
            mundaris_app::developer_service::DeveloperService::start(preset, move || {
                let _ = proxy.send_event(AppEvent::DeveloperWake);
            })?;
        app.gravity_demo
            .developer_set_session(&service.descriptor.session_id);
        app.developer = Some(service);
    }
    event_loop
        .run_app(&mut app)
        .context("running the native application event loop")?;

    if let Some(error) = app.application_error {
        return Err(error);
    }

    info!("Mundaris shut down cleanly");
    Ok(())
}
