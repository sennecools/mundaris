//! Astrum Studio window (docs/STUDIO_UI.md): winit owns the window and event
//! loop, egui draws the Studio chrome, and the engine renders the scene into an
//! offscreen texture that the viewport shows. One wgpu device is shared.
//!
//! Frame order: panels (from the previous view) → viewport input → engine frame
//! sized to the viewport → overlays → tessellate → surface acquire (the vsync
//! wait under FIFO) → encode and submit → present.

mod panels;
mod profiler_panel;
mod surface;
mod theme;
mod viewport;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use egui::{Frame, Key, KeyboardShortcut, Margin, Modifiers};
use serde::{Deserialize, Serialize};
use astrum_app::{
    GravityOrbitsDemo,
    engine_profile::span,
    studio::view::{StudioAction, StudioView},
};
use astrum_renderer::{GpuContext, Renderer};
use tracing::info;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

use crate::profiler_ui::{self, ProfilerData, ProfilerModels};
use profiler_panel::ProfilerInput;
use surface::WindowSurface;
use viewport::{Interaction, ViewportInput};

/// Refresh period for panel text (inspector, status bar, HUD, profiler).
const PANEL_PERIOD: Duration = Duration::from_millis(100);

/// Wakeups delivered to the event loop from other threads.
#[derive(Debug, Clone, Copy)]
pub enum AppEvent {
    #[cfg(feature = "developer-tools")]
    DeveloperWake,
}

/// Frame owner: the demo, its renderer and panel caches.
pub struct Engine {
    pub demo: GravityOrbitsDemo,
    pub renderer: Renderer,
    #[cfg(feature = "developer-tools")]
    pub developer: Option<astrum_app::developer_service::DeveloperService>,
    error: Option<anyhow::Error>,
    quit: bool,
    profiler_models: ProfilerModels,
    profiler_data: ProfilerData,
    /// Engine frame of each profiler strip bar, for click selection.
    bar_frames: Vec<u64>,
    /// Throttled copy of the view for readable panel text.
    panel_view: StudioView,
    last_panels: Option<Instant>,
}

impl Engine {
    pub fn new(demo: GravityOrbitsDemo, renderer: Renderer) -> Self {
        Self {
            demo,
            renderer,
            #[cfg(feature = "developer-tools")]
            developer: None,
            error: None,
            quit: false,
            profiler_models: ProfilerModels::default(),
            profiler_data: ProfilerData::default(),
            bar_frames: Vec::new(),
            panel_view: StudioView::default(),
            last_panels: None,
        }
    }

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
                self.quit = true;
            }
        }
    }

    fn action(&mut self, action: StudioAction) {
        self.human_input("human_input_ui");
        self.demo.studio_action(action);
    }

    fn interaction(&mut self, interaction: Interaction) {
        match interaction {
            Interaction::Event(event) => self.demo.viewport_event(event),
            Interaction::Shortcut(shortcut) => self.demo.viewport_shortcut(shortcut),
            Interaction::Click(pos, double) => self.demo.viewport_click(pos, double),
            Interaction::FlyTo(pos) => self.demo.viewport_fly_to(pos),
            Interaction::LabelClick(index, double) => self.demo.viewport_label_click(index, double),
            Interaction::Human(reason) => self.human_input(reason),
        }
    }

    fn profiler_input(&mut self, input: ProfilerInput) {
        let profiler = self.demo.profiler();
        match input {
            ProfilerInput::Action(action) => self.action(action),
            ProfilerInput::SelectFrame(index) => {
                if let Some(&frame) = self.bar_frames.get(index) {
                    profiler.selected_frame = Some(frame);
                    profiler.selected_span = None;
                }
            }
            ProfilerInput::FollowLatest => {
                profiler.selected_frame = None;
                profiler.selected_span = None;
            }
            ProfilerInput::SelectSpan(index) => profiler.selected_span = Some(index),
        }
    }

    /// One engine frame: update, render the scene texture, refresh panel caches.
    fn frame(&mut self, width: u32, height: u32, scale: f32, profiler_open: bool) {
        if self.error.is_some() {
            return;
        }
        #[cfg(feature = "developer-tools")]
        {
            self.developer_turn();
            // Developer builds sample GPU timestamps on request; keep sampling
            // while the profiler is visible so its GPU readouts stay live.
            if profiler_open {
                self.renderer.request_developer_gpu_timing();
            }
        }
        if let Err(error) = self.demo.render(&mut self.renderer, width, height, scale) {
            self.error = Some(error.context("rendering a frame"));
            return;
        }
        #[cfg(feature = "developer-tools")]
        if let Some(service) = &mut self.developer {
            service.observe(&self.demo, &self.renderer, width > 0 && height > 0);
        }
        let now = Instant::now();
        if self
            .last_panels
            .is_some_and(|last| now.duration_since(last) < PANEL_PERIOD)
        {
            return;
        }
        self.last_panels = Some(now);
        let _span = span("Studio panel sync");
        self.panel_view.clone_from(self.demo.studio_view());
        if profiler_open {
            let _span = span("Profiler panel build");
            let (data, bar_frames) = profiler_ui::build(&mut self.demo, &self.profiler_models);
            self.profiler_data = data;
            self.bar_frames = bar_frames;
        }
    }
}

/// The scene texture registered with egui; re-pointed when the renderer
/// recreates it (viewport resize).
struct SceneTexture {
    texture: wgpu::Texture,
    id: egui::TextureId,
}

/// Studio workspaces shown as tabs in the top bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Workspace {
    /// Outliner, viewport, inspector and log drawer.
    #[default]
    Editor,
    /// Viewport over the full-width profiler.
    Performance,
}

impl Workspace {
    pub const ALL: [Self; 2] = [Self::Editor, Self::Performance];
    pub const NAMES: [&'static str; 2] = ["Editor", "Performance"];
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Inspector tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InspectorTab {
    #[default]
    Body,
    Planet,
    Render,
}

impl InspectorTab {
    pub const ALL: [Self; 3] = [Self::Body, Self::Planet, Self::Render];
    pub const NAMES: [&'static str; 3] = ["Body", "Planet", "Render"];
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Panel layout remembered between runs (`LAYOUT_PATH`, relative to the
/// working directory's build output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct StudioLayout {
    workspace: Workspace,
    inspector_tab: InspectorTab,
    log_open: bool,
    outliner_width: f32,
    inspector_width: f32,
    log_height: f32,
    /// Profiler height in the Performance workspace; 0 picks 60 % of the window.
    performance_height: f32,
}

impl Default for StudioLayout {
    fn default() -> Self {
        Self {
            workspace: Workspace::Editor,
            inspector_tab: InspectorTab::Body,
            log_open: false,
            outliner_width: 240.0,
            inspector_width: 340.0,
            log_height: 170.0,
            performance_height: 0.0,
        }
    }
}

const LAYOUT_PATH: &str = "target/studio-layout.json";

impl StudioLayout {
    fn load() -> Self {
        std::fs::read_to_string(LAYOUT_PATH)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let path = std::path::Path::new(LAYOUT_PATH);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// UI-local presentation state.
#[derive(Default)]
struct UiState {
    viewport: ViewportInput,
    layout: StudioLayout,
    timeline: profiler_panel::TimelineState,
    scene: Option<SceneTexture>,
}

struct Presentation {
    window: Arc<Window>,
    surface: WindowSurface,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

pub struct StudioApp {
    gpu: GpuContext,
    engine: Engine,
    egui_ctx: egui::Context,
    presentation: Option<Presentation>,
    ui: UiState,
    occluded: bool,
}

impl StudioApp {
    pub fn new(gpu: GpuContext, engine: Engine) -> Self {
        let egui_ctx = egui::Context::default();
        theme::apply(&egui_ctx);
        Self {
            gpu,
            engine,
            egui_ctx,
            presentation: None,
            ui: UiState {
                layout: StudioLayout::load(),
                ..UiState::default()
            },
            occluded: false,
        }
    }

    /// The first engine or startup error, if the loop stopped because of one.
    pub fn finish(mut self) -> Result<()> {
        self.engine.error.take().map_or(Ok(()), Err)
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.engine.error.get_or_insert(error);
        event_loop.exit();
    }

    fn create_presentation(&self, event_loop: &ActiveEventLoop) -> Result<Presentation> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Astrum Studio")
                        .with_inner_size(LogicalSize::new(1600.0, 900.0)),
                )
                .context("creating the Studio window")?,
        );
        let surface = WindowSurface::new(&self.gpu, Arc::clone(&window))?;
        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(self.gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &self.gpu.device,
            surface.config.format,
            egui_wgpu::RendererOptions::default(),
        );
        Ok(Presentation {
            window,
            surface,
            egui_state,
            egui_renderer,
        })
    }

    fn redraw(&mut self) {
        let Some(presentation) = &mut self.presentation else {
            return;
        };
        let size = presentation.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let device = &self.gpu.device;
        presentation.surface.resize(device, size.width, size.height);
        let raw_input = presentation
            .egui_state
            .take_egui_input(presentation.window.as_ref());
        let mut ui_ms = 0.0;
        let engine = &mut self.engine;
        let ui = &mut self.ui;
        let egui_renderer = &mut presentation.egui_renderer;
        let full_output = self.egui_ctx.run_ui(raw_input, |root| {
            ui_ms += draw(root, engine, ui, egui_renderer, device);
        });
        presentation
            .egui_state
            .handle_platform_output(presentation.window.as_ref(), full_output.platform_output);

        let tail = Instant::now();
        let ui_span = span("UI draw");
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        for (id, deltas) in &full_output.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(device, &self.gpu.queue, *id, delta);
            }
        }
        drop(ui_span);
        ui_ms += tail.elapsed().as_secs_f64() * 1000.0;

        let frame = {
            let _span = span("Surface acquire");
            presentation.surface.acquire(device)
        };
        if let Some(frame) = frame {
            let encode = Instant::now();
            let ui_span = span("UI draw");
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [
                    presentation.surface.config.width,
                    presentation.surface.config.height,
                ],
                pixels_per_point: full_output.pixels_per_point,
            };
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Studio UI encoder"),
            });
            let extra = egui_renderer.update_buffers(
                device,
                &self.gpu.queue,
                &mut encoder,
                &paint_jobs,
                &screen,
            );
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Studio UI"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0035,
                                g: 0.0044,
                                b: 0.0065,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                egui_renderer.render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
            }
            self.gpu
                .queue
                .submit(extra.into_iter().chain([encoder.finish()]));
            drop(ui_span);
            ui_ms += encode.elapsed().as_secs_f64() * 1000.0;
            let _span = span("Present");
            presentation.window.pre_present_notify();
            self.gpu.queue.present(frame);
        }
        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }
        self.engine.demo.set_ui_render_ms(ui_ms);
    }
}

/// Runs `add` in a child of the panel that cannot grow it. egui panels
/// report overflowing content by shrinking their outer rect, which slid the
/// viewport over the inspector; content past the edge is clipped instead.
fn contained<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let rect = ui.max_rect();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let inner = add(&mut child);
    ui.advance_cursor_after_rect(rect);
    inner
}

/// Lays out the Studio for one frame and runs the engine frame inside the
/// viewport. Returns the milliseconds spent in UI work (excluding the engine).
fn draw(
    root: &mut egui::Ui,
    engine: &mut Engine,
    ui_state: &mut UiState,
    egui_renderer: &mut egui_wgpu::Renderer,
    device: &wgpu::Device,
) -> f64 {
    let started = Instant::now();
    let ui_span = span("UI draw");
    let mut actions = Vec::new();
    let mut profiler_inputs = Vec::new();
    let mut stop_automation = false;
    {
        let view = engine.demo.studio_view();
        let panel = &engine.panel_view;
        let layout = &mut ui_state.layout;
        // Planet undo and redo, unless a text field has the keyboard.
        if view.planet.is_some() && !root.ctx().egui_wants_keyboard_input() {
            let (undo, redo) = root.ctx().input_mut(|input| {
                // Redo first: Ctrl+Z also matches Ctrl+Shift+Z.
                let redo = input.consume_shortcut(&KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::Z,
                )) || input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
                let undo =
                    input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
                (undo, redo)
            });
            if undo {
                actions.push(StudioAction::PlanetUndo);
            }
            if redo {
                actions.push(StudioAction::PlanetRedo);
            }
        }
        let chrome = |margin: Margin| Frame::new().fill(theme::SURFACE_1).inner_margin(margin);
        egui::Panel::top("toolbar")
            .exact_size(40.0)
            .frame(chrome(Margin::symmetric(theme::SPACE_3 as i8, 0)))
            .show(root, |ui| {
                panels::toolbar(
                    ui,
                    view,
                    &panel.time_text,
                    &mut layout.workspace,
                    &mut actions,
                )
            });
        let editor = layout.workspace == Workspace::Editor;
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(chrome(Margin::symmetric(theme::SPACE_3 as i8, 0)))
            .show(root, |ui| {
                let log_open = editor.then_some(&mut layout.log_open);
                stop_automation = panels::status_bar(ui, view, panel, log_open)
            });
        match layout.workspace {
            Workspace::Editor => {
                let shown = egui::Panel::left("outliner")
                    .resizable(true)
                    .default_size(layout.outliner_width)
                    .size_range(180.0..=420.0)
                    .frame(chrome(Margin::same(theme::SPACE_3 as i8)))
                    .show(root, |ui| {
                        contained(ui, |ui| panels::outliner(ui, view, &mut actions))
                    });
                layout.outliner_width = shown.response.rect.width();
                let shown = egui::Panel::right("inspector")
                    .resizable(true)
                    .default_size(layout.inspector_width)
                    .size_range(280.0..=560.0)
                    .frame(chrome(Margin::same(theme::SPACE_3 as i8)))
                    .show(root, |ui| {
                        contained(ui, |ui| {
                            panels::inspector(
                                ui,
                                view,
                                panel,
                                &mut layout.inspector_tab,
                                &mut actions,
                            )
                        })
                    });
                layout.inspector_width = shown.response.rect.width();
                if layout.log_open {
                    let shown = egui::Panel::bottom("log")
                        .resizable(true)
                        .default_size(layout.log_height)
                        .size_range(90.0..=420.0)
                        .frame(chrome(Margin::same(theme::SPACE_2 as i8)))
                        .show(root, |ui| {
                            contained(ui, |ui| {
                                panels::section_header(ui, "LOG", "");
                                panels::log(ui, panel);
                            })
                        });
                    layout.log_height = shown.response.rect.height();
                }
            }
            Workspace::Performance => {
                let available = root.available_height();
                let default = if layout.performance_height > 0.0 {
                    layout.performance_height
                } else {
                    available * 0.6
                };
                let shown = egui::Panel::bottom("performance")
                    .resizable(true)
                    .default_size(default)
                    .size_range(240.0..=(available - 160.0).max(260.0))
                    .frame(chrome(Margin::same(theme::SPACE_2 as i8)))
                    .show(root, |ui| {
                        contained(ui, |ui| {
                            profiler_inputs = profiler_panel::show(
                                ui,
                                &engine.profiler_data,
                                &mut ui_state.timeline,
                            );
                        })
                    });
                layout.performance_height = shown.response.rect.height();
            }
        }
    }
    drop(ui_span);
    let mut ui_ms = started.elapsed().as_secs_f64() * 1000.0;
    for action in actions {
        engine.action(action);
    }
    for input in profiler_inputs {
        engine.profiler_input(input);
    }
    if stop_automation {
        engine.human_input("human_stop");
    }

    egui::CentralPanel::no_frame().show(root, |ui| {
        let rect = ui.max_rect();
        let interactions = ui_state
            .viewport
            .interact(ui, rect, engine.demo.studio_view());
        for interaction in interactions {
            engine.interaction(interaction);
        }
        let scale = ui.ctx().pixels_per_point();
        let width = (rect.width() * scale).round().max(0.0) as u32;
        let height = (rect.height() * scale).round().max(0.0) as u32;
        let profiler_open = ui_state.layout.workspace == Workspace::Performance;
        engine.frame(width, height, scale, profiler_open);

        let overlay = Instant::now();
        let _span = span("UI draw");
        let texture = engine.renderer.scene_texture();
        if let Some(texture) = texture
            && ui_state.scene.as_ref().map(|scene| &scene.texture) != Some(texture)
        {
            // The scene holds sRGB-encoded colour in `SCENE_TEXTURE_FORMAT`
            // (Rgba8Unorm); egui samples textures as gamma-space values, so the
            // plain view is the matching one (an sRGB view would decode twice).
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let id = match &ui_state.scene {
                Some(scene) => {
                    egui_renderer.update_egui_texture_from_wgpu_texture(
                        device,
                        &view,
                        wgpu::FilterMode::Nearest,
                        scene.id,
                    );
                    scene.id
                }
                None => {
                    egui_renderer.register_native_texture(device, &view, wgpu::FilterMode::Nearest)
                }
            };
            ui_state.scene = Some(SceneTexture {
                texture: texture.clone(),
                id,
            });
        }
        let scene = texture.and(ui_state.scene.as_ref().map(|scene| scene.id));
        viewport::paint(
            ui,
            rect,
            scene,
            engine.demo.studio_view(),
            &engine.panel_view.hud,
        );
        let mut overlay_actions = Vec::new();
        viewport::overlay_controls(
            ui.ctx(),
            rect,
            engine.demo.studio_view(),
            &mut overlay_actions,
        );
        for action in overlay_actions {
            engine.action(action);
        }
        ui_ms += overlay.elapsed().as_secs_f64() * 1000.0;
    });
    ui_ms
}

impl ApplicationHandler<AppEvent> for StudioApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.presentation.is_some() {
            return;
        }
        match self.create_presentation(event_loop) {
            Ok(presentation) => {
                self.presentation = Some(presentation);
                info!("Astrum Studio window initialized");
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(presentation) = &mut self.presentation else {
            return;
        };
        let _ = presentation
            .egui_state
            .on_window_event(presentation.window.as_ref(), &event);
        match event {
            WindowEvent::CloseRequested => {
                self.ui.layout.save();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                presentation
                    .surface
                    .resize(&self.gpu.device, size.width, size.height)
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded {
                    // Frames resume after a pause; do not count the gap as elapsed time.
                    self.engine.demo.reset_wall_capture();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                if let Some(error) = self.engine.error.take() {
                    self.fail(event_loop, error);
                } else if self.engine.quit {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        #[cfg(feature = "developer-tools")]
        match event {
            AppEvent::DeveloperWake => {
                self.engine.developer_turn();
                if self.engine.quit {
                    event_loop.exit();
                } else if let Some(presentation) = &self.presentation {
                    presentation.window.request_redraw();
                }
            }
        }
        #[cfg(not(feature = "developer-tools"))]
        {
            let _ = event_loop;
            match event {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Continuous rendering; FIFO presentation blocks in surface acquire, so
        // frames follow the display rate. Occluded or minimized windows idle.
        if let Some(presentation) = &self.presentation {
            let size = presentation.window.inner_size();
            if !self.occluded && size.width > 0 && size.height > 0 {
                presentation.window.request_redraw();
            }
        }
    }
}

/// Builds the event loop's error for a failed startup outside the loop.
pub fn loop_error(error: winit::error::EventLoopError) -> anyhow::Error {
    anyhow!("running the Studio event loop: {error}")
}
