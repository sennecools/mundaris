#![forbid(unsafe_code)]

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use mundaris_renderer::Renderer;
use tracing::info;
use tracing_subscriber::EnvFilter;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

struct MundarisApp {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    startup_error: Option<anyhow::Error>,
}

impl MundarisApp {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            startup_error: None,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.startup_error = Some(error);
        event_loop.exit();
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
                self.fail(event_loop, error.into());
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

        let response = renderer.on_window_event(&event);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                if let Err(error) = renderer.render() {
                    self.fail(
                        event_loop,
                        anyhow::Error::new(error).context("rendering a frame"),
                    );
                    return;
                }
            }
            _ => {}
        }

        if response.repaint {
            window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + Duration::from_millis(16),
        ));
        if let Some(window) = &self.window {
            window.request_redraw();
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

    let event_loop = EventLoop::new().context("creating the native event loop")?;
    let mut app = MundarisApp::new();
    event_loop
        .run_app(&mut app)
        .context("running the native application event loop")?;

    if let Some(error) = app.startup_error {
        return Err(error);
    }

    info!("Mundaris shut down cleanly");
    Ok(())
}
