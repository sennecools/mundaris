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

struct MundarisApp {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    application_error: Option<anyhow::Error>,
    occluded: bool,
    redraw_schedule: redraw::RedrawSchedule,
}

impl MundarisApp {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            application_error: None,
            occluded: false,
            redraw_schedule: redraw::RedrawSchedule::default(),
        }
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
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
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
                if let Err(error) = renderer.render(bootstrap_ui) {
                    self.fail(
                        event_loop,
                        anyhow::Error::new(error).context("rendering a frame"),
                    );
                    return;
                }
            }
            _ => {}
        }

        if repaint && !self.occluded && window.is_minimized() != Some(true) {
            window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
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
    let mut app = MundarisApp::new();
    event_loop
        .run_app(&mut app)
        .context("running the native application event loop")?;

    if let Some(error) = app.application_error {
        return Err(error);
    }

    info!("Mundaris shut down cleanly");
    Ok(())
}
