#![forbid(unsafe_code)]

mod celestial_model;
mod redraw;
mod reference_frames;

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
    demo: Option<reference_frames::ReferenceFrameDemo>,
    celestial_demo: Option<celestial_model::CelestialModelDemo>,
    gravity_demo: Option<mundaris_app::GravityOrbitsDemo>,
}

impl MundarisApp {
    fn new(
        reference_frames: bool,
        celestial_model: bool,
        gravity_orbits: bool,
        solar_scale: Option<bool>,
    ) -> Result<Self> {
        Ok(Self {
            #[cfg(feature = "developer-tools")]
            developer: None,
            window: None,
            renderer: None,
            application_error: None,
            occluded: false,
            redraw_schedule: redraw::RedrawSchedule::default(),
            demo: if reference_frames {
                Some(reference_frames::ReferenceFrameDemo::new()?)
            } else {
                None
            },
            celestial_demo: if celestial_model {
                Some(celestial_model::CelestialModelDemo::new()?)
            } else {
                None
            },
            gravity_demo: if let Some(real_scale) = solar_scale {
                Some(mundaris_app::GravityOrbitsDemo::solar_system(real_scale)?)
            } else if gravity_orbits {
                Some(mundaris_app::GravityOrbitsDemo::new()?)
            } else {
                None
            },
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
        if let Some(demo) = &mut self.demo {
            demo.reset_wall_tick();
        }
        if let Some(demo) = &mut self.celestial_demo {
            demo.reset_wall_tick();
        }
        if let Some(demo) = &mut self.gravity_demo {
            demo.reset_wall_capture();
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(demo) = &mut self.demo {
            demo.set_lifecycle_drawable(false);
        }
        if let Some(demo) = &mut self.celestial_demo {
            demo.set_lifecycle_drawable(false);
        }
        if let Some(demo) = &mut self.gravity_demo {
            demo.set_lifecycle_drawable(false);
        }
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
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
        ) && let (Some(service), Some(demo)) = (&mut self.developer, &mut self.gravity_demo)
        {
            service.interrupt(demo, renderer, "human_input");
        }

        let scene_consumed = self
            .gravity_demo
            .as_mut()
            .is_some_and(|demo| demo.on_window_event(&event));
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
                let result = if let Some(demo) = &mut self.demo {
                    demo.render(renderer, size.width, size.height)
                } else if let Some(demo) = &mut self.celestial_demo {
                    demo.render(renderer, size.width, size.height)
                } else if let Some(demo) = &mut self.gravity_demo {
                    demo.render(renderer, size.width, size.height)
                } else {
                    renderer.render(bootstrap_ui).map_err(anyhow::Error::new)
                };
                if let Err(error) = result {
                    self.fail(event_loop, error.context("rendering a frame"));
                    return;
                }
                #[cfg(feature = "developer-tools")]
                if let (Some(service), Some(demo)) = (&mut self.developer, &self.gravity_demo) {
                    service.observe(demo, renderer, true);
                }
            }
            _ => {}
        }

        if repaint && !self.occluded && window.is_minimized() != Some(true) {
            window.request_redraw();
        }
        let drawable = self.drawable();
        if let Some(demo) = &mut self.gravity_demo {
            demo.set_lifecycle_drawable(drawable);
        }
        if let Some(demo) = &mut self.demo {
            demo.set_lifecycle_drawable(drawable);
        }
        if let Some(demo) = &mut self.celestial_demo {
            demo.set_lifecycle_drawable(drawable);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(feature = "developer-tools")]
        self.developer_turn(event_loop);
        let drawable = self.drawable();
        if let Some(demo) = &mut self.demo {
            demo.set_lifecycle_drawable(drawable);
        }
        if let Some(demo) = &mut self.celestial_demo {
            demo.set_lifecycle_drawable(drawable);
        }
        if let Some(demo) = &mut self.gravity_demo {
            demo.set_lifecycle_drawable(drawable);
        }
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
        if let (Some(service), Some(demo), Some(renderer)) = (
            &mut self.developer,
            &mut self.gravity_demo,
            &mut self.renderer,
        ) {
            service.turn(demo, renderer, drawable);
            if service.shutdown_requested {
                event_loop.exit();
            }
        }
    }
}

fn bootstrap_ui(context: &egui::Context) {
    egui::Window::new("Mundaris")
        .collapsible(false)
        .resizable(false)
        .show(context, |ui| {
            ui.label("Bootstrap environment");
            ui.label("Renderer initialized");
        });
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

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("creating the native event loop")?;
    let mut arguments: Vec<_> = std::env::args().skip(1).collect();
    let dev_interface = arguments.iter().any(|a| a == "--dev-interface");
    anyhow::ensure!(
        arguments
            .iter()
            .filter(|a| a.as_str() == "--dev-interface")
            .count()
            <= 1,
        "duplicate --dev-interface"
    );
    arguments.retain(|a| a != "--dev-interface");
    #[cfg(not(feature = "developer-tools"))]
    anyhow::ensure!(
        !dev_interface,
        "--dev-interface requires the developer-tools build feature"
    );
    let usage = "usage: mundaris_app [--solar-system | --real-solar-system | --reference-frames | --celestial-model | --gravity-orbits]";
    anyhow::ensure!(
        arguments.len() <= 1,
        "conflicting or duplicate arguments; {usage}"
    );
    let (reference_frames, celestial_model, gravity_orbits, solar_scale) =
        match arguments.first().map(String::as_str) {
            None | Some("--solar-system") => (false, false, false, Some(false)),
            Some("--real-solar-system") => (false, false, false, Some(true)),
            Some("--reference-frames") => (true, false, false, None),
            Some("--celestial-model") => (false, true, false, None),
            Some("--gravity-orbits") => (false, false, true, None),
            Some(argument) => anyhow::bail!("unknown argument: {argument}; {usage}"),
        };
    let mut app = MundarisApp::new(
        reference_frames,
        celestial_model,
        gravity_orbits,
        solar_scale,
    )?;
    #[cfg(feature = "developer-tools")]
    if dev_interface {
        anyhow::ensure!(
            app.gravity_demo.is_some(),
            "development interface does not support reference-frame or celestial-model demos"
        );
        let preset = if gravity_orbits {
            "gravity-orbits"
        } else if solar_scale == Some(true) {
            "real-solar-system"
        } else {
            "solar-system"
        };
        let proxy = event_loop.create_proxy();
        let service =
            mundaris_app::developer_service::DeveloperService::start(preset, move || {
                let _ = proxy.send_event(AppEvent::DeveloperWake);
            })?;
        if let Some(demo) = &mut app.gravity_demo {
            demo.developer_set_session(&service.descriptor.session_id);
        }
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
