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

struct MundarisApp {
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
    fn new(reference_frames: bool, celestial_model: bool, gravity_orbits: bool) -> Result<Self> {
        Ok(Self {
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
            gravity_demo: if gravity_orbits {
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

impl ApplicationHandler for MundarisApp {
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

        let repaint = renderer.on_window_event(&event);
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

    let event_loop = EventLoop::new().context("creating the native event loop")?;
    let mut reference_frames = false;
    let mut celestial_model = false;
    let mut gravity_orbits = false;
    for argument in std::env::args().skip(1) {
        if argument == "--reference-frames"
            && !reference_frames
            && !celestial_model
            && !gravity_orbits
        {
            reference_frames = true;
        } else if argument == "--celestial-model"
            && !reference_frames
            && !celestial_model
            && !gravity_orbits
        {
            celestial_model = true;
        } else if argument == "--gravity-orbits"
            && !reference_frames
            && !celestial_model
            && !gravity_orbits
        {
            gravity_orbits = true;
        } else {
            anyhow::bail!(
                "unknown, conflicting or duplicate argument: {argument}; usage: mundaris_app [--reference-frames | --celestial-model | --gravity-orbits]"
            );
        }
    }
    let mut app = MundarisApp::new(reference_frames, celestial_model, gravity_orbits)?;
    event_loop
        .run_app(&mut app)
        .context("running the native application event loop")?;

    if let Some(error) = app.application_error {
        return Err(error);
    }

    info!("Mundaris shut down cleanly");
    Ok(())
}
