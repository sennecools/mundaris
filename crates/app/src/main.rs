// `deny` rather than `forbid`: Slint-generated UI code allows `unsafe_code`
// locally. Hand-written code in this binary still may not use `unsafe`.
#![deny(unsafe_code)]

//! Mundaris Studio: the native entry point. Slint owns the window and event
//! loop; the engine renders the scene into a texture shown in the viewport and
//! shares one wgpu device with the UI (docs/STUDIO_UI.md §3).

use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result, anyhow};
use mundaris_app::{
    GravityOrbitsDemo,
    studio::view::{Overlay, Shortcut, StudioAction},
    viewport::{FlightKey, PointerButton, ViewportEvent},
};
use mundaris_renderer::{GpuContext, Renderer, TerrainViewMode};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tracing::info;
use tracing_subscriber::EnvFilter;

slint::include_modules!();

mod profiler_ui;
mod ui_sync;

/// Frame-owner state shared by the UI callbacks on the event-loop thread.
struct Engine {
    demo: GravityOrbitsDemo,
    renderer: Renderer,
    #[cfg(feature = "developer-tools")]
    developer: Option<mundaris_app::developer_service::DeveloperService>,
    error: Option<anyhow::Error>,
    scene_size: [u32; 2],
    models: Option<ui_sync::UiModels>,
    /// Engine frame of each profiler strip bar, for click selection.
    bar_frames: Vec<u64>,
    profiler_models: profiler_ui::ProfilerModels,
}

thread_local! {
    static ENGINE: RefCell<Option<Rc<RefCell<Engine>>>> = const { RefCell::new(None) };
}

fn with_engine(f: impl FnOnce(&mut Engine)) {
    let engine = ENGINE.with(|slot| slot.borrow().clone());
    if let Some(engine) = engine
        && let Ok(mut engine) = engine.try_borrow_mut()
    {
        f(&mut engine);
    }
}

impl Engine {
    /// Human input takes control back from an automation lease.
    fn human_input(&mut self, reason: &str) {
        #[cfg(feature = "developer-tools")]
        if let Some(service) = &mut self.developer {
            service.interrupt(&mut self.demo, &mut self.renderer, reason);
        }
        #[cfg(not(feature = "developer-tools"))]
        let _ = reason;
    }

    #[cfg(feature = "developer-tools")]
    fn developer_turn(&mut self) {
        if let Some(service) = &mut self.developer {
            service.turn(&mut self.demo, &mut self.renderer, true);
            if service.shutdown_requested {
                let _ = slint::quit_event_loop();
            }
        }
    }

    /// One engine frame: update, render the scene texture, publish the view.
    fn frame(&mut self, ui: &StudioWindow) {
        if self.error.is_some() {
            return;
        }
        #[cfg(feature = "developer-tools")]
        {
            self.developer_turn();
            // Developer builds sample GPU timestamps on request; keep sampling
            // while the profiler is visible so its GPU readouts stay live.
            if ui.get_bottom_open() {
                self.renderer.request_developer_gpu_timing();
            }
        }
        let scale = ui.window().scale_factor();
        let width = (ui.get_viewport_width() * scale).round().max(0.0) as u32;
        let height = (ui.get_viewport_height() * scale).round().max(0.0) as u32;
        if let Err(error) = self.demo.render(&mut self.renderer, width, height, scale) {
            self.error = Some(error.context("rendering a frame"));
            let _ = slint::quit_event_loop();
            return;
        }
        #[cfg(feature = "developer-tools")]
        if let Some(service) = &mut self.developer {
            service.observe(&self.demo, &self.renderer, width > 0 && height > 0);
        }
        if let (Some(texture), Some(size)) =
            (self.renderer.scene_texture(), self.renderer.scene_size())
            && size != self.scene_size
        {
            match slint::Image::try_from(texture.clone()) {
                Ok(image) => {
                    ui.set_scene(image);
                    self.scene_size = size;
                }
                Err(error) => {
                    self.error = Some(anyhow!("importing the scene texture: {error:?}"));
                    let _ = slint::quit_event_loop();
                    return;
                }
            }
        }
        let _span = mundaris_app::engine_profile::span("Studio panel sync");
        let models = self
            .models
            .get_or_insert_with(|| ui_sync::UiModels::bind(ui));
        let panels_due = models.panels_due();
        {
            let _span = mundaris_app::engine_profile::span("Panel models");
            models.apply(ui, self.demo.studio_view(), panels_due);
        }
        if !panels_due {
            return;
        }
        if ui.get_bottom_open() {
            let (data, bar_frames) = {
                let _span = mundaris_app::engine_profile::span("Profiler panel build");
                profiler_ui::build(&mut self.demo, &self.profiler_models)
            };
            let _span = mundaris_app::engine_profile::span("Profiler panel set");
            ui.set_profiler(data);
            self.bar_frames = bar_frames;
        }
    }
}

fn view_mode_names() -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        TerrainViewMode::ALL
            .iter()
            .map(|mode| match mode {
                TerrainViewMode::Lit => "Lit",
                TerrainViewMode::Height => "Height",
                TerrainViewMode::Normals => "Normals",
                TerrainViewMode::Grid => "Grid",
                TerrainViewMode::Level => "LOD level",
                TerrainViewMode::MorphFade => "Morph / fade",
            })
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// Maps a viewport key to a shortcut, a flight key, or nothing.
fn handle_key(engine: &mut Engine, text: &str, pressed: bool, shift: bool, repeat: bool) -> bool {
    use slint::platform::Key;
    engine.demo.viewport_event(ViewportEvent::Boost(shift));
    if let Some(key) = FlightKey::from_text(text) {
        engine
            .demo
            .viewport_event(ViewportEvent::Key { key, pressed });
        return true;
    }
    if !pressed || repeat {
        return false;
    }
    let is = |key: Key| text == SharedString::from(key).as_str();
    let shortcut = match text {
        "f" | "F" => Some(Shortcut::Focus),
        "i" | "I" => Some(Shortcut::SurfaceNavigation),
        "h" | "H" => Some(Shortcut::Horizon),
        _ if is(Key::Home) => Some(Shortcut::Overview),
        _ if is(Key::Escape) => Some(Shortcut::FreeFlight),
        _ if is(Key::Tab) && shift => Some(Shortcut::PreviousBody),
        _ if is(Key::Tab) || is(Key::Backtab) => Some(Shortcut::NextBody),
        _ => None,
    };
    if let Some(shortcut) = shortcut {
        engine.demo.viewport_shortcut(shortcut);
        return true;
    }
    false
}

fn wire_callbacks(ui: &StudioWindow) {
    let action = |action: StudioAction| {
        with_engine(|engine| {
            engine.human_input("human_input_ui");
            engine.demo.studio_action(action);
        })
    };
    ui.on_toggle_pause(move || action(StudioAction::TogglePause));
    ui.on_set_rate(move |index| action(StudioAction::SetRate(index as usize)));
    ui.on_set_camera(move |index| action(StudioAction::SetCamera(index as usize)));
    ui.on_set_view(move |index| action(StudioAction::SetView(index as usize)));
    ui.on_toggle_terrain(move || action(StudioAction::ToggleTerrain));
    ui.on_toggle_labels(move || action(StudioAction::ToggleLabels));
    ui.on_toggle_overlay(move |index| {
        if let Some(overlay) = Overlay::ALL.get(index as usize) {
            action(StudioAction::ToggleOverlay(*overlay));
        }
    });
    ui.on_capture(move || action(StudioAction::Capture));
    ui.on_select_body(move |index| action(StudioAction::SelectBody(index as usize)));
    ui.on_focus_body(move |index| action(StudioAction::FocusBody(index as usize)));
    ui.on_set_speed_exponent(move |value| action(StudioAction::SetSpeedExponent(value)));
    ui.on_approach(move |index| action(StudioAction::Approach(index as usize)));
    ui.on_surface_navigation(move || action(StudioAction::SurfaceNavigation));
    ui.on_frame_selected(move || action(StudioAction::FrameSelected));
    ui.on_profiler_set_enabled(move |value| action(StudioAction::ProfilerEnabled(value)));
    ui.on_profiler_set_frozen(move |value| action(StudioAction::ProfilerFrozen(value)));
    ui.on_profiler_export(move || action(StudioAction::ProfilerExport));
    ui.on_profiler_capture(move || action(StudioAction::CaptureBadFrames));
    ui.on_profiler_select_frame(|index| {
        with_engine(|engine| {
            if let Some(&frame) = engine.bar_frames.get(index as usize) {
                let profiler = engine.demo.profiler();
                profiler.selected_frame = Some(frame);
                profiler.selected_span = None;
            }
        })
    });
    ui.on_profiler_follow_latest(|| {
        with_engine(|engine| {
            let profiler = engine.demo.profiler();
            profiler.selected_frame = None;
            profiler.selected_span = None;
        })
    });
    ui.on_profiler_select_span(|index| {
        with_engine(|engine| engine.demo.profiler().selected_span = Some(index as usize))
    });
    ui.on_profiler_set_full_frame(|value| {
        with_engine(|engine| {
            let profiler = engine.demo.profiler();
            profiler.full_frame = value;
            profiler.selected_span = None;
        })
    });
    ui.on_stop_automation(move || with_engine(|engine| engine.human_input("human_stop")));

    ui.on_viewport_pointer(|kind, button, x, y| {
        with_engine(|engine| {
            let pos = [x, y];
            let button = match button {
                1 => Some(PointerButton::Primary),
                2 => Some(PointerButton::Secondary),
                _ => None,
            };
            let event = match (kind, button) {
                (1, Some(button)) => {
                    engine.human_input("human_input_mouse_button");
                    ViewportEvent::PointerButton {
                        pos,
                        button,
                        pressed: true,
                    }
                }
                (2, Some(button)) => ViewportEvent::PointerButton {
                    pos,
                    button,
                    pressed: false,
                },
                (3, _) => ViewportEvent::PointerLeft,
                _ => ViewportEvent::PointerMoved(pos),
            };
            engine.demo.viewport_event(event);
        })
    });
    ui.on_viewport_scroll(|_dx, dy| {
        with_engine(|engine| {
            engine.human_input("human_input_mouse_wheel");
            // Slint reports wheel lines as 60 logical pixels.
            engine
                .demo
                .viewport_event(ViewportEvent::Wheel(f64::from(dy) / 60.0));
        })
    });
    ui.on_viewport_clicked(|x, y, double| {
        with_engine(|engine| engine.demo.viewport_click([x, y], double))
    });
    ui.on_viewport_label_clicked(|index, double| {
        with_engine(|engine| {
            engine.human_input("human_input_mouse_button");
            engine.demo.viewport_label_click(index as usize, double);
        })
    });
    ui.on_viewport_key(|text, pressed, shift, repeat| {
        let mut handled = false;
        with_engine(|engine| {
            if pressed {
                engine.human_input("human_input_keyboard_pressed");
            }
            handled = handle_key(engine, text.as_str(), pressed, shift, repeat);
        });
        handled
    });
    ui.on_viewport_focus(|focused| {
        with_engine(|engine| {
            engine
                .demo
                .viewport_event(ViewportEvent::FocusChanged(focused))
        })
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

    let gpu = GpuContext::new().context("initializing the GPU")?;
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Manual {
            instance: gpu.instance.clone(),
            adapter: gpu.adapter.clone(),
            device: gpu.device.clone(),
            queue: gpu.queue.clone(),
        })
        .select()
        .context("selecting the Slint wgpu backend")?;

    let ui = StudioWindow::new().context("creating the Studio window")?;
    ui.set_view_modes(view_mode_names());

    #[allow(unused_mut)]
    let mut renderer = Renderer::new(&gpu);
    let mut demo = GravityOrbitsDemo::shared_test_system()?;
    #[cfg(feature = "developer-tools")]
    let developer = if dev_interface {
        renderer.set_developer_observation_mode(true);
        let service =
            mundaris_app::developer_service::DeveloperService::start("test-solar-system", || {
                let _ = slint::invoke_from_event_loop(|| {
                    with_engine(|engine| engine.developer_turn());
                });
            })?;
        demo.developer_set_session(&service.descriptor.session_id);
        Some(service)
    } else {
        None
    };
    demo.reset_wall_capture();
    let engine = Rc::new(RefCell::new(Engine {
        demo,
        renderer,
        #[cfg(feature = "developer-tools")]
        developer,
        error: None,
        scene_size: [0, 0],
        models: None,
        bar_frames: Vec::new(),
        profiler_models: Default::default(),
    }));
    ENGINE.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&engine)));
    // The Studio records CPU spans by default so the profiler timeline is live.
    engine
        .borrow_mut()
        .demo
        .studio_action(StudioAction::ProfilerEnabled(true));
    wire_callbacks(&ui);

    // Each engine frame runs after Slint finishes drawing the previous one, so
    // simulation and scene rendering are paced by presentation (vsync).
    // (Running it inside BeforeRendering measured slower: ~90 vs ~112 FPS.)
    let ui_weak = ui.as_weak();
    let mut render_started: Option<std::time::Instant> = None;
    let mut render_span: Option<mundaris_app::engine_profile::ProfileSpan> = None;
    ui.window()
        .set_rendering_notifier(move |state, _api| match state {
            slint::RenderingState::BeforeRendering => {
                render_started = Some(std::time::Instant::now());
                // Recorded on the main lane so the timeline shows UI cost next to
                // engine work. Slint acquires the surface before this notification
                // and presents after AfterRendering, so the span is UI drawing and
                // submission only (measured ~95% CPU-busy), not a vsync wait.
                render_span = Some(mundaris_app::engine_profile::span("Slint UI draw"));
            }
            slint::RenderingState::AfterRendering => {
                drop(render_span.take());
                if let Some(started) = render_started.take() {
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    with_engine(|engine| engine.demo.set_ui_render_ms(ms));
                }
                let ui_weak = ui_weak.clone();
                // A posted event runs promptly; zero-delay timers are serviced
                // coarsely on Windows and measurably missed vsync slots.
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        with_engine(|engine| engine.frame(&ui));
                        ui.window().request_redraw();
                    }
                });
            }
            _ => {}
        })
        .map_err(|error| anyhow!("installing the frame notifier: {error:?}"))?;
    info!("Mundaris Studio window initialized");
    ui.run().context("running the Studio event loop")?;

    ENGINE.with(|slot| slot.borrow_mut().take());
    let error = engine.borrow_mut().error.take();
    if let Some(error) = error {
        return Err(error);
    }
    info!("Mundaris shut down cleanly");
    Ok(())
}
