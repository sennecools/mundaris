//! Native graph authoring for the shared test solar system.
//!
//! The editor mutates a serializable graph draft. Evaluated previews and bundle
//! exports run on one bounded background job at a time; only a queued app command
//! can publish a compiled graph into world authority.
use mundaris_terrain_fields::graph::{
    CompiledGraph, Graph, LayoutPoint, MAX_GRAPH_BYTES, Node, VariationLocks, catalog, templates,
    vary,
};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

const PREVIEW_WIDTH: usize = 96;
const PREVIEW_HEIGHT: usize = 48;
const EXPORT_RESOLUTION: u32 = 32;
const UNDO_LIMIT: usize = 48;
const GRAPH_NODE_WIDTH: f32 = 152.0;
const GRAPH_NODE_HEIGHT: f32 = 42.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewMap {
    Height,
    Humidity,
    Temperature,
    Weights,
    Support,
    SelectedNode,
}
impl PreviewMap {
    const ALL: [Self; 6] = [
        Self::Height,
        Self::Humidity,
        Self::Temperature,
        Self::Weights,
        Self::Support,
        Self::SelectedNode,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Height => "Height",
            Self::Humidity => "Humidity",
            Self::Temperature => "Temp",
            Self::Weights => "Weights",
            Self::Support => "Support",
            Self::SelectedNode => "Node",
        }
    }
}

#[derive(Clone)]
struct PreviewMapData {
    map: PreviewMap,
    pixels: Vec<egui::Color32>,
    minimum: f64,
    maximum: f64,
}
struct PreviewData {
    generation: u64,
    selected_node: String,
    selected_node_scalar: bool,
    graph_identity: String,
    geometry_identity: String,
    climate_identity: String,
    material_identity: String,
    palette_identity: String,
    maps: Vec<PreviewMapData>,
}
enum JobRequest {
    Preview {
        generation: u64,
        graph: Graph,
        selected_node: String,
    },
    Export {
        generation: u64,
        graph: Graph,
        root: PathBuf,
        resolution: u32,
    },
}
enum JobOutput {
    Preview(Result<PreviewData, String>),
    Export(Result<(PathBuf, String), String>),
}
struct JobResult {
    generation: u64,
    output: JobOutput,
}

pub(crate) enum PanelAction {
    Apply {
        body: mundaris_world::BodyId,
        graph: Box<Graph>,
    },
    Restore {
        body: mundaris_world::BodyId,
    },
}

#[derive(Clone)]
struct PublishedStatus {
    body_name: String,
    graph_identity: String,
    terrain_revision: u64,
    restored: bool,
}

pub(crate) struct TerrainAuthoringPanel {
    open: bool,
    graph: Graph,
    selected_node: String,
    selected_map: PreviewMap,
    selected_template: String,
    preview_generation: u64,
    preview_dirty: bool,
    pending_job: Option<JobRequest>,
    worker: Option<JoinHandle<()>>,
    result_rx: Receiver<JobResult>,
    result_tx: SyncSender<JobResult>,
    last_good: Option<PreviewData>,
    textures: Vec<(PreviewMap, egui::TextureHandle)>,
    error: Option<String>,
    status: Option<String>,
    undo: Vec<Graph>,
    locks: VariationLocks,
    variation_seed: u64,
    export_resolution: u32,
    graph_path: String,
    output_path: String,
    selected_body_key: Option<(String, u64)>,
    published: Option<PublishedStatus>,
    window_has_focus: bool,
}

impl TerrainAuthoringPanel {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let template_catalog = templates();
        let first = template_catalog
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item.get("graph"))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("terrain graph templates are unavailable"))?;
        let graph: Graph = serde_json::from_value(first)?;
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        Ok(Self {
            open: std::env::var("MUNDARIS_TERRAIN_AUTHORING").is_ok_and(|value| value == "1"),
            graph,
            selected_node: "height".into(),
            selected_map: PreviewMap::Height,
            selected_template: "rocky-feature".into(),
            preview_generation: 1,
            preview_dirty: true,
            pending_job: None,
            worker: None,
            result_rx,
            result_tx,
            last_good: None,
            textures: Vec::new(),
            error: None,
            status: None,
            undo: Vec::new(),
            locks: VariationLocks {
                geometry: false,
                climate: false,
                palette: false,
            },
            variation_seed: 1,
            export_resolution: EXPORT_RESOLUTION,
            graph_path: String::new(),
            output_path: std::env::var("MUNDARIS_DEV_OUTPUT")
                .unwrap_or_else(|_| "D:/Mundaris/target/terrain-authoring-native/exports".into()),
            selected_body_key: None,
            published: None,
            window_has_focus: false,
        })
    }

    pub(crate) fn set_published(
        &mut self,
        body_name: String,
        graph_identity: String,
        terrain_revision: u64,
        restored: bool,
    ) {
        self.published = Some(PublishedStatus {
            body_name,
            graph_identity,
            terrain_revision,
            restored,
        });
    }
    #[cfg(test)]
    pub(crate) fn published_status(&self) -> Option<(&str, &str, u64, bool)> {
        self.published.as_ref().map(|published| {
            (
                published.body_name.as_str(),
                published.graph_identity.as_str(),
                published.terrain_revision,
                published.restored,
            )
        })
    }
    pub(crate) fn set_status(&mut self, status: String) {
        self.status = Some(status);
    }
    pub(crate) fn camera_input_blocked(&self) -> bool {
        self.open || self.window_has_focus
    }

    pub(crate) fn draw(
        &mut self,
        context: &egui::Context,
        body: mundaris_world::BodyId,
        body_name: &str,
        reference_radius_m: f64,
        terrain_revision: u64,
        can_restore_selected_body: bool,
    ) -> Option<PanelAction> {
        self.poll_worker(context);
        let body_key = (body_name.to_owned(), reference_radius_m.to_bits());
        if self.selected_body_key.as_ref() != Some(&body_key) {
            self.selected_body_key = Some(body_key);
            if reference_radius_m.is_finite()
                && reference_radius_m > 0.0
                && self.graph.reference_radius_m.to_bits() != reference_radius_m.to_bits()
            {
                self.push_undo();
                self.graph.reference_radius_m = reference_radius_m;
                self.mark_graph_changed();
            }
            self.status = Some(format!(
                "Graph reference radius adapted to {body_name}: {reference_radius_m:.3} m"
            ));
        }
        if self.last_good.as_ref().is_some_and(|preview| {
            preview.generation == self.preview_generation
                && preview.selected_node != self.selected_node
        }) {
            self.mark_graph_changed();
        }

        let mut action = None;
        egui::Area::new(egui::Id::new("terrain-authoring-toggle"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 8.0))
            .show(context, |ui| {
                if ui
                    .button(if self.open {
                        "Terrain authoring ✓"
                    } else {
                        "Terrain authoring"
                    })
                    .clicked()
                {
                    self.open = !self.open;
                }
            });
        if self.open {
            let mut open = self.open;
            let response = egui::Window::new("Terrain authoring · shared test system")
                .id(egui::Id::new("terrain-authoring-window"))
                .default_pos(egui::pos2(660.0, 52.0))
                .default_size(egui::vec2(560.0, 640.0))
                .min_width(420.0)
                .min_height(300.0)
                .max_width(600.0)
                .max_size(egui::vec2(600.0, 680.0))
                .resizable(true)
                .open(&mut open)
                .show(context, |ui| {
                    ui.set_max_width(560.0);
                    egui::ScrollArea::vertical()
                        .max_height(600.0)
                        .show(ui, |ui| {
                        let graph_before_frame = self.graph.clone();
                        let mut undo_action = false;
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(format!("Selected body: {body_name}"));
                            ui.label(format!("reference radius {:.3} m", reference_radius_m));
                            ui.label(format!("terrain revision {terrain_revision}"));
                        });
                        ui.small("Graph previews use the selected body's physical radius and field units.");
                        ui.separator();

                        let mut graph_changed = false;
                        let templates_value = templates();
                        let template_items = templates_value.as_array().cloned().unwrap_or_default();
                        egui::ComboBox::from_label("Graph template")
                            .selected_text(&self.selected_template)
                            .show_ui(ui, |ui| {
                                for item in &template_items {
                                    let id = item["id"].as_str().unwrap_or_default();
                                    let name = item["name"].as_str().unwrap_or(id);
                                    if ui.selectable_label(self.selected_template == id, name).clicked()
                                        && let Ok(graph) = serde_json::from_value::<Graph>(item["graph"].clone())
                                    {
                                        self.push_undo();
                                        self.selected_template = id.to_owned();
                                        self.graph = graph;
                                        self.graph.reference_radius_m = reference_radius_m;
                                        self.layout_missing_nodes();
                                        self.selected_node = self.graph.nodes.first().map_or_else(String::new, |n| n.id.clone());
                                        graph_changed = true;
                                    }
                                }
                            });
                        self.draw_preview_section(ui);
                        ui.horizontal(|ui| {
                            ui.label("recipe_id");
                            graph_changed |= ui.text_edit_singleline(&mut self.graph.recipe_id).changed();
                            ui.label(format!("reference radius: {:.3} m", reference_radius_m));
                        });
                        if egui::ScrollArea::both()
                            .max_width(ui.available_width())
                            .max_height(240.0)
                            .show(ui, |ui| self.draw_graph_canvas(ui))
                            .inner
                        {
                            graph_changed = true;
                        }
                        ui.collapsing("Seeds and deterministic variation", |ui| {
                            ui.horizontal(|ui| {
                                graph_changed |= ui.add(egui::DragValue::new(&mut self.graph.seeds.geometry).prefix("geometry ")).changed();
                                graph_changed |= ui.add(egui::DragValue::new(&mut self.graph.seeds.climate).prefix("climate ")).changed();
                                graph_changed |= ui.add(egui::DragValue::new(&mut self.graph.seeds.material).prefix("material ")).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.locks.geometry, "Lock geometry");
                                ui.checkbox(&mut self.locks.climate, "Lock climate");
                                ui.checkbox(&mut self.locks.palette, "Lock palette");
                            });
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut self.variation_seed).prefix("variation seed "));
                                if ui.button("Generate constrained variation").clicked() {
                                    self.push_undo();
                                    match vary(&self.graph, self.variation_seed, &self.locks) {
                                        Ok(graph) => { self.graph = graph; graph_changed = true; self.status = Some("Deterministic variation generated with the selected locks".into()); }
                                        Err(error) => { self.error = Some(error.to_string()); }
                                    }
                                }
                            });
                        });
                        ui.horizontal_wrapped(|ui| {
                            let catalog = catalog();
                            let kinds = catalog["node_kinds"].as_array().cloned().unwrap_or_default();
                            egui::ComboBox::from_label("Add node")
                                .selected_text("Choose node kind")
                                .show_ui(ui, |ui| {
                                    for kind in &kinds {
                                        let name = kind["kind"].as_str().unwrap_or_default();
                                        let label = kind["label"].as_str().unwrap_or(name);
                                        if ui.button(label).clicked()
                                            && let Some(node) = node_from_catalog(kind, &self.graph)
                                        {
                                            self.push_undo();
                                            self.selected_node = node.id.clone();
                                            self.graph.nodes.push(node);
                                            self.layout_missing_nodes();
                                            graph_changed = true;
                                        }
                                    }
                                });
                            if ui.button("Undo").clicked()
                                && let Some(previous) = self.undo.pop()
                            {
                                self.graph = previous;
                                self.layout_missing_nodes();
                                graph_changed = true;
                                undo_action = true;
                                self.status = Some("Graph draft restored from undo history".into());
                            }
                            if ui.button("Delete selected node").clicked()
                                && self.graph.nodes.len() > 1
                                && !self.selected_node.is_empty()
                            {
                                self.push_undo();
                                self.graph.nodes.retain(|node| node.id != self.selected_node);
                                for node in &mut self.graph.nodes { node.inputs.retain(|_, source| source != &self.selected_node); }
                                self.graph.layout.remove(&self.selected_node);
                                self.selected_node = self.graph.nodes.first().map_or_else(String::new, |node| node.id.clone());
                                graph_changed = true;
                            }
                        });
                        ui.separator();
                        if self.draw_selected_node(ui) { graph_changed = true; }
                        if self.draw_outputs(ui) { graph_changed = true; }
                        ui.separator();
                        ui.heading("Graph persistence and bundle export");
                        ui.horizontal(|ui| { ui.label("Graph file"); ui.text_edit_singleline(&mut self.graph_path); });
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Save graph").clicked() { self.save_graph(); }
                            if ui.button("Load graph").clicked() { self.load_graph(reference_radius_m); graph_changed = true; }
                            ui.add(egui::DragValue::new(&mut self.export_resolution).range(1..=256).prefix("bundle face resolution "));
                        });
                        ui.horizontal(|ui| { ui.label("External export directory"); ui.text_edit_singleline(&mut self.output_path); });
                        if ui.button("Export lossless graph bundle").clicked() {
                            match validate_external_output(Path::new(&self.output_path)) {
                                Ok(root) => {
                                    let generation = self.preview_generation;
                                    self.queue_job(JobRequest::Export { generation, graph: self.graph.clone(), root, resolution: self.export_resolution });
                                    self.status = Some("Bounded graph bundle export queued off the render thread".into());
                                }
                                Err(error) => self.error = Some(error),
                            }
                        }

                        ui.separator();
                        let preview_matches = self.last_good.as_ref().is_some_and(|preview| preview.generation == self.preview_generation) && self.error.is_none();
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(preview_matches && self.error.is_none(), egui::Button::new("Apply graph to selected body")).clicked() {
                                action = Some(PanelAction::Apply { body, graph: Box::new(self.graph.clone()) });
                            }
                            if ui.add_enabled(can_restore_selected_body, egui::Button::new("Restore original surface")).clicked() {
                                action = Some(PanelAction::Restore { body });
                            }
                        });
                        if let Some(published) = &self.published {
                            let response = ui.small(format!(
                                "{} · graph {} · revision {} · {}",
                                published.body_name,
                                short_identity(&published.graph_identity),
                                published.terrain_revision,
                                if published.restored { "original restored" } else { "graph published" }
                            ));
                            response.on_hover_text(format!("Full graph identity: {}", published.graph_identity));
                        }
                        if graph_changed {
                            if !undo_action && !self.undo.last().is_some_and(|previous| *previous == graph_before_frame) {
                                self.push_undo_snapshot(graph_before_frame);
                            }
                            self.mark_graph_changed();
                        }
                    });
                });
            self.window_has_focus = response
                .as_ref()
                .is_some_and(|inner| inner.response.contains_pointer());
            self.open = open;
        } else {
            self.window_has_focus = false;
        }
        if self.preview_dirty {
            let graph = self.graph.clone();
            let generation = self.preview_generation;
            self.queue_job(JobRequest::Preview {
                generation,
                graph,
                selected_node: self.selected_node.clone(),
            });
            self.preview_dirty = false;
        }
        self.poll_worker(context);
        action
    }

    fn push_undo(&mut self) {
        self.undo.push(self.graph.clone());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }
    fn push_undo_snapshot(&mut self, graph: Graph) {
        self.undo.push(graph);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }
    fn mark_graph_changed(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1).max(1);
        self.preview_dirty = true;
        self.error = None;
    }
    fn layout_missing_nodes(&mut self) {
        let ids = self
            .graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        self.graph.layout.retain(|id, point| {
            ids.contains(id.as_str())
                && point.x.is_finite()
                && point.y.is_finite()
                && (0.0..=10_000.0).contains(&point.x)
                && (0.0..=10_000.0).contains(&point.y)
        });
        for (index, node) in self.graph.nodes.iter().enumerate() {
            self.graph
                .layout
                .entry(node.id.clone())
                .or_insert(LayoutPoint {
                    x: (index / 5) as f64 * 205.0,
                    y: (index % 5) as f64 * 74.0,
                });
        }
    }

    fn draw_graph_canvas(&mut self, ui: &mut egui::Ui) -> bool {
        self.layout_missing_nodes();
        let max_x = self
            .graph
            .layout
            .values()
            .map(|p| p.x)
            .fold(0.0_f64, f64::max);
        let max_y = self
            .graph
            .layout
            .values()
            .map(|p| p.y)
            .fold(0.0_f64, f64::max);
        let size = egui::vec2(
            (max_x as f32 + GRAPH_NODE_WIDTH + 28.0).max(460.0),
            (max_y as f32 + GRAPH_NODE_HEIGHT + 28.0).max(225.0),
        );
        ui.label(
            "Graph canvas · drag nodes · reconnect inputs below · wires show authored dependencies",
        );
        let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(22, 28, 36));
        let point = |id: &str| {
            let layout = self
                .graph
                .layout
                .get(id)
                .cloned()
                .unwrap_or(LayoutPoint { x: 0.0, y: 0.0 });
            rect.min + egui::vec2(layout.x as f32 + 10.0, layout.y as f32 + 10.0)
        };
        for node in &self.graph.nodes {
            let target = point(&node.id);
            for source_id in node.inputs.values() {
                let source = point(source_id);
                let a = source + egui::vec2(GRAPH_NODE_WIDTH, GRAPH_NODE_HEIGHT * 0.5);
                let b = target + egui::vec2(0.0, GRAPH_NODE_HEIGHT * 0.5);
                painter.line_segment(
                    [a, b],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(75, 178, 220)),
                );
                painter.circle_filled(b, 3.5, egui::Color32::from_rgb(75, 178, 220));
            }
        }
        let mut changed = false;
        let mut dragged = None;
        for node in &self.graph.nodes {
            let pos = point(&node.id);
            let node_rect =
                egui::Rect::from_min_size(pos, egui::vec2(GRAPH_NODE_WIDTH, GRAPH_NODE_HEIGHT));
            let selected = node.id == self.selected_node;
            painter.rect_filled(
                node_rect,
                4.0,
                if selected {
                    egui::Color32::from_rgb(56, 86, 116)
                } else {
                    egui::Color32::from_rgb(42, 50, 62)
                },
            );
            painter.rect_stroke(
                node_rect,
                4.0,
                egui::Stroke::new(
                    1.0,
                    if selected {
                        egui::Color32::LIGHT_BLUE
                    } else {
                        egui::Color32::GRAY
                    },
                ),
                egui::StrokeKind::Inside,
            );
            painter.text(
                node_rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}\n{}", node.label, node.kind),
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
            let hit = ui.interact(
                node_rect,
                egui::Id::new(("terrain-graph-node", &node.id)),
                egui::Sense::click_and_drag(),
            );
            if hit.clicked() {
                self.selected_node = node.id.clone();
            }
            if hit.dragged() {
                dragged = Some((node.id.clone(), hit.drag_delta()));
            }
        }
        if let Some((id, delta)) = dragged {
            if let Some(layout) = self.graph.layout.get_mut(&id) {
                layout.x = (layout.x + delta.x as f64).max(0.0);
                layout.y = (layout.y + delta.y as f64).max(0.0);
            }
            changed = true;
        }
        changed
    }

    fn draw_selected_node(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(index) = self
            .graph
            .nodes
            .iter()
            .position(|node| node.id == self.selected_node)
        else {
            return false;
        };
        ui.heading("Selected node · parameters and typed connections");
        let mut changed = false;
        let node_ids = self
            .graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        let node = &mut self.graph.nodes[index];
        ui.horizontal(|ui| {
            ui.label(format!("Node id: {}", node.id));
            ui.label("Label");
            changed |= ui.text_edit_singleline(&mut node.label).changed();
        });
        let input_names = match node.kind.as_str() {
            "remap" => vec!["input"],
            "add" => vec!["a", "b"],
            "blend" => vec!["a", "b", "mask"],
            "material" => vec!["height", "humidity", "temperature"],
            _ => Vec::new(),
        };
        for input in input_names {
            ui.horizontal(|ui| {
                ui.label(format!("{input} dependency"));
                let current = node.inputs.get(input).cloned().unwrap_or_default();
                egui::ComboBox::from_id_salt(("terrain-edge", &node.id, input))
                    .selected_text(if current.is_empty() {
                        "Unconnected"
                    } else {
                        &current
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(current.is_empty(), "Unconnected")
                            .clicked()
                        {
                            node.inputs.remove(input);
                            changed = true;
                        }
                        for id in &node_ids {
                            if ui.selectable_label(current == *id, id).clicked() && current != *id {
                                node.inputs.insert(input.into(), id.clone());
                                changed = true;
                            }
                        }
                    });
            });
        }
        let keys = node.params.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let Some(value) = node.params.get_mut(&key) else {
                continue;
            };
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{}{}",
                    key,
                    parameter_unit(&key, &node.kind, value)
                ));
                match value {
                    Value::Number(number) if number.is_i64() || number.is_u64() => {
                        if let Some(number) = number.as_u64() {
                            let mut v = number;
                            if ui.add(egui::DragValue::new(&mut v).speed(1.0)).changed() {
                                *value = json!(v);
                                changed = true;
                            }
                        } else if let Some(number) = number.as_i64() {
                            let mut v = number;
                            if ui.add(egui::DragValue::new(&mut v).speed(1.0)).changed() {
                                *value = json!(v);
                                changed = true;
                            }
                        }
                    }
                    Value::Number(number) => {
                        if let Some(number) = number.as_f64() {
                            let mut v = number;
                            if ui
                                .add(egui::DragValue::new(&mut v).speed(1.0).max_decimals(6))
                                .changed()
                                && v.is_finite()
                            {
                                *value = json!(v);
                                changed = true;
                            }
                        }
                    }
                    Value::String(text) if key == "unit" || key == "seed_namespace" => {
                        let choices: &[&str] = if key == "unit" {
                            &["m", "1", "K"]
                        } else {
                            &["geometry", "climate", "material"]
                        };
                        egui::ComboBox::from_id_salt(("terrain-param", &node.id, &key))
                            .selected_text(text.as_str())
                            .show_ui(ui, |ui| {
                                for choice in choices {
                                    if ui.selectable_label(text == choice, *choice).clicked() {
                                        *text = (*choice).into();
                                        changed = true;
                                    }
                                }
                            });
                    }
                    Value::String(text) => {
                        changed |= ui.text_edit_singleline(text).changed();
                    }
                    Value::Array(values) if key == "palette" && values.len() == 3 => {
                        for (i, color) in values.iter_mut().enumerate() {
                            if let Some(rgb) = json_rgb(color) {
                                let mut rgb = rgb.map(|c| c as f32);
                                if ui.color_edit_button_rgb(&mut rgb).changed() {
                                    *color = json!(rgb.map(|c| c.clamp(0.0, 1.0) as f64));
                                    changed = true;
                                }
                                ui.label(format!("C{} linear", i + 1));
                            }
                        }
                    }
                    Value::Array(values) => {
                        for component in values.iter_mut() {
                            if let Some(number) = component.as_f64() {
                                let mut v = number;
                                if ui
                                    .add(egui::DragValue::new(&mut v).speed(0.01).max_decimals(5))
                                    .changed()
                                    && v.is_finite()
                                {
                                    *component = json!(v);
                                    changed = true;
                                }
                            } else if let Some(text) = component.as_str() {
                                let mut text = text.to_owned();
                                if ui.text_edit_singleline(&mut text).changed() {
                                    *component = json!(text);
                                    changed = true;
                                }
                            }
                        }
                    }
                    Value::Bool(value) => {
                        changed |= ui.checkbox(value, "enabled").changed();
                    }
                    _ => {
                        ui.label(value.to_string());
                    }
                }
            });
        }
        if let Some(preview) = &self.last_good
            && let Some(sample) = preview
                .maps
                .iter()
                .find(|map| map.map == PreviewMap::SelectedNode)
        {
            ui.small(format!("Selected scalar node preview range {:.5} … {:.5}; graph units are checked when compiling", sample.minimum, sample.maximum));
        }
        changed
    }

    fn draw_preview_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("Rust field previews");
        ui.horizontal_wrapped(|ui| {
            for map in PreviewMap::ALL {
                if ui
                    .selectable_label(self.selected_map == map, map.label())
                    .clicked()
                {
                    self.selected_map = map;
                }
            }
        });
        if let Some(preview) = self.last_good.as_ref() {
            let identity_line = ui.small(format!(
                "Generation {} · graph {} · geometry {}",
                preview.generation,
                short_identity(&preview.graph_identity),
                short_identity(&preview.geometry_identity)
            ));
            identity_line.on_hover_text(format!(
                "Full graph identity: {}\nFull geometry identity: {}",
                preview.graph_identity, preview.geometry_identity
            ));
            let field_line = ui.small(format!(
                "climate {} · material {} · palette {}",
                short_identity(&preview.climate_identity),
                short_identity(&preview.material_identity),
                short_identity(&preview.palette_identity)
            ));
            field_line.on_hover_text(format!(
                "Full climate identity: {}\nFull material identity: {}\nFull palette identity: {}",
                preview.climate_identity, preview.material_identity, preview.palette_identity
            ));
            ui.small(format!("Selected node: {}", preview.selected_node));
            ui.horizontal_wrapped(|ui| {
                for (map, texture) in &self.textures {
                    if *map == self.selected_map
                        && (*map != PreviewMap::SelectedNode || preview.selected_node_scalar)
                    {
                        let size = egui::vec2(144.0, 72.0);
                        ui.vertical(|ui| {
                            ui.label(map.label());
                            ui.image((texture.id(), size));
                            if let Some(data) = preview.maps.iter().find(|data| data.map == *map) {
                                ui.small(format!("{:.4} … {:.4}", data.minimum, data.maximum));
                            }
                        });
                    }
                }
            });
        } else {
            ui.label("Waiting for first valid graph preview…");
        }
        if self.preview_dirty {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!(
                    "Preview generation {} pending; last-good map remains visible",
                    self.preview_generation
                ),
            );
        }
        if self.selected_map == PreviewMap::SelectedNode
            && self
                .last_good
                .as_ref()
                .is_some_and(|preview| !preview.selected_node_scalar)
        {
            ui.label(
                "This node has a material output; select a scalar node or view material weights.",
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("Graph / job error: {error}"),
            );
        }
        if let Some(status) = &self.status {
            ui.label(status);
        }
    }

    fn draw_outputs(&mut self, ui: &mut egui::Ui) -> bool {
        ui.collapsing("Typed graph outputs", |ui| {
            let ids = self
                .graph
                .nodes
                .iter()
                .map(|node| node.id.clone())
                .collect::<Vec<_>>();
            let mut changed = false;
            for (label, current) in [
                ("Height output · m", &mut self.graph.outputs.height),
                ("Humidity output · 1", &mut self.graph.outputs.humidity),
                (
                    "Temperature output · K",
                    &mut self.graph.outputs.temperature,
                ),
                (
                    "Material output · weights",
                    &mut self.graph.outputs.material,
                ),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    egui::ComboBox::from_label(label)
                        .selected_text(current.as_str())
                        .show_ui(ui, |ui| {
                            for id in &ids {
                                if ui.selectable_label(*current == *id, id).clicked()
                                    && *current != *id
                                {
                                    *current = id.clone();
                                    changed = true;
                                }
                            }
                        });
                });
            }
            ui.horizontal(|ui| {
                ui.label("Feature support · 1");
                let current = self.graph.outputs.support.clone();
                egui::ComboBox::from_label("Feature support output")
                    .selected_text(current.as_deref().unwrap_or("None"))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "None").clicked()
                            && current.is_some()
                        {
                            self.graph.outputs.support = None;
                            changed = true;
                        }
                        for id in &ids {
                            if ui
                                .selectable_label(current.as_deref() == Some(id), id)
                                .clicked()
                                && current.as_deref() != Some(id)
                            {
                                self.graph.outputs.support = Some(id.clone());
                                changed = true;
                            }
                        }
                    });
            });
            changed
        })
        .body_returned
        .unwrap_or(false)
    }

    fn save_graph(&mut self) {
        match explicit_graph_path(&self.graph_path).and_then(|path| {
            let bytes =
                serde_json::to_vec_pretty(&self.graph).map_err(|error| error.to_string())?;
            if bytes.len() > 256 * 1024 {
                return Err("graph file exceeds the 256 KiB limit".into());
            }
            std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
            Ok(path)
        }) {
            Ok(path) => {
                self.status = Some(format!("Graph saved: {}", path.display()));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn load_graph(&mut self, radius_m: f64) {
        let result = explicit_graph_path(&self.graph_path).and_then(|path| {
            let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
            if metadata.len() > MAX_GRAPH_BYTES as u64 {
                return Err("graph file exceeds the 256 KiB limit".into());
            }
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            std::fs::File::open(&path)
                .map_err(|error| error.to_string())?
                .take(MAX_GRAPH_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() > MAX_GRAPH_BYTES {
                return Err("graph file exceeds the 256 KiB limit".into());
            }
            let mut graph: Graph =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            graph.reference_radius_m = radius_m;
            Ok((path, graph))
        });
        match result {
            Ok((path, graph)) => {
                self.push_undo();
                self.graph = graph;
                self.layout_missing_nodes();
                self.selected_template = self.graph.recipe_id.clone();
                self.status = Some(format!(
                    "Graph loaded and adapted to selected body: {}",
                    path.display()
                ));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn queue_job(&mut self, job: JobRequest) {
        if self.worker.is_some() {
            self.pending_job = Some(job);
        } else {
            self.start_job(job);
        }
    }
    fn start_job(&mut self, job: JobRequest) {
        let tx = self.result_tx.clone();
        self.worker = Some(thread::spawn(move || {
            let generation = match &job {
                JobRequest::Preview { generation, .. } | JobRequest::Export { generation, .. } => {
                    *generation
                }
            };
            let output = match job {
                JobRequest::Preview {
                    generation,
                    graph,
                    selected_node,
                } => JobOutput::Preview(
                    preview_graph(generation, &graph, &selected_node)
                        .map_err(|error| error.to_string()),
                ),
                JobRequest::Export {
                    graph,
                    root,
                    resolution,
                    ..
                } => JobOutput::Export(
                    mundaris_terrain_fields::graph_bundle::publish(&graph, resolution, &root)
                        .map(|(path, identity, _)| (path, identity))
                        .map_err(|error| error.to_string()),
                ),
            };
            let _ = tx.send(JobResult { generation, output });
        }));
    }
    fn poll_worker(&mut self, context: &egui::Context) {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(worker) = self.worker.take()
                && worker.join().is_err()
            {
                self.error = Some("Terrain authoring background job panicked".into());
            }
            while let Ok(result) = self.result_rx.try_recv() {
                match result.output {
                    JobOutput::Preview(Ok(preview)) => {
                        if result.generation != self.preview_generation {
                            continue;
                        }
                        self.textures = preview
                            .maps
                            .iter()
                            .map(|map| {
                                (
                                    map.map,
                                    context.load_texture(
                                        format!("terrain-authoring-{:?}", map.map),
                                        egui::ColorImage::new(
                                            [PREVIEW_WIDTH, PREVIEW_HEIGHT],
                                            map.pixels.clone(),
                                        ),
                                        egui::TextureOptions::NEAREST,
                                    ),
                                )
                            })
                            .collect();
                        self.last_good = Some(preview);
                        self.preview_dirty = false;
                        self.error = None;
                    }
                    JobOutput::Preview(Err(error)) => {
                        if result.generation == self.preview_generation {
                            self.error = Some(error);
                            self.preview_dirty = false;
                        }
                    }
                    JobOutput::Export(Ok((path, identity))) => {
                        self.status = Some(format!(
                            "Verified lossless graph bundle {identity}: {}",
                            path.display()
                        ));
                        self.error = None;
                    }
                    JobOutput::Export(Err(error)) => {
                        self.error = Some(format!("bundle export failed: {error}"))
                    }
                }
            }
        }
        if self.worker.is_none()
            && let Some(job) = self.pending_job.take()
        {
            self.start_job(job);
        }
    }
}

fn short_identity(identity: &str) -> &str {
    identity.get(..12).unwrap_or(identity)
}

fn preview_graph(
    generation: u64,
    graph: &Graph,
    selected_node: &str,
) -> Result<PreviewData, anyhow::Error> {
    let compiled = CompiledGraph::compile(graph.clone())?;
    let mut height = Vec::with_capacity(PREVIEW_WIDTH * PREVIEW_HEIGHT);
    let mut humidity = Vec::with_capacity(height.capacity());
    let mut temperature = Vec::with_capacity(height.capacity());
    let mut weights = Vec::with_capacity(height.capacity());
    let mut support = Vec::with_capacity(height.capacity());
    let mut selected = Vec::with_capacity(height.capacity());
    let mut selected_node_scalar = true;
    for y in 0..PREVIEW_HEIGHT {
        let latitude = std::f64::consts::FRAC_PI_2
            - (y as f64 + 0.5) / PREVIEW_HEIGHT as f64 * std::f64::consts::PI;
        for x in 0..PREVIEW_WIDTH {
            let longitude = (x as f64 + 0.5) / PREVIEW_WIDTH as f64 * std::f64::consts::TAU
                - std::f64::consts::PI;
            let direction = [
                latitude.cos() * longitude.cos(),
                latitude.sin(),
                latitude.cos() * longitude.sin(),
            ];
            let sample = compiled.evaluate(direction)?;
            height.push(sample.height_m);
            humidity.push(sample.humidity);
            temperature.push(sample.temperature_k);
            weights.push(sample.weights);
            support.push(sample.support);
            if let Some(value) = sample.node_values.get(selected_node).copied() {
                selected.push(value);
            } else {
                selected_node_scalar = false;
                selected.push(0.0);
            }
        }
    }
    let identities = compiled.identities().clone();
    Ok(PreviewData {
        generation,
        selected_node: selected_node.to_owned(),
        selected_node_scalar,
        graph_identity: identities.graph,
        geometry_identity: identities.geometry,
        climate_identity: identities.climate,
        material_identity: identities.material,
        palette_identity: identities.palette,
        maps: vec![
            scalar_map(PreviewMap::Height, height, false),
            scalar_map(PreviewMap::Humidity, humidity, true),
            scalar_map(PreviewMap::Temperature, temperature, false),
            weight_map(weights),
            scalar_map(PreviewMap::Support, support, true),
            scalar_map(PreviewMap::SelectedNode, selected, false),
        ],
    })
}
fn scalar_map(map: PreviewMap, values: Vec<f64>, fixed_range: bool) -> PreviewMapData {
    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (maximum - minimum).abs().max(f64::MIN_POSITIVE);
    let pixels = values
        .into_iter()
        .map(|value| {
            let t = if fixed_range {
                value.clamp(0.0, 1.0)
            } else {
                ((value - minimum) / span).clamp(0.0, 1.0)
            };
            let channel = (t * 255.0).round() as u8;
            egui::Color32::from_gray(channel)
        })
        .collect();
    PreviewMapData {
        map,
        pixels,
        minimum,
        maximum,
    }
}
fn weight_map(values: Vec<[f64; 3]>) -> PreviewMapData {
    let pixels = values
        .iter()
        .map(|weights| {
            egui::Color32::from_rgb(
                (weights[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (weights[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (weights[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            )
        })
        .collect();
    PreviewMapData {
        map: PreviewMap::Weights,
        pixels,
        minimum: 0.0,
        maximum: 1.0,
    }
}
fn node_from_catalog(spec: &Value, graph: &Graph) -> Option<Node> {
    if graph.nodes.len() >= 128 {
        return None;
    }
    let kind = spec["kind"].as_str()?;
    let mut id = kind.replace('_', "-");
    let mut suffix = 1;
    while graph.nodes.iter().any(|node| node.id == id) {
        id = format!("{}-{suffix}", kind.replace('_', "-"));
        suffix += 1;
    }
    let params = spec["params"]
        .as_array()?
        .iter()
        .filter_map(|parameter| {
            Some((
                parameter["name"].as_str()?.to_owned(),
                parameter["default"].clone(),
            ))
        })
        .collect();
    let inputs = spec["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|input| {
            input["name"]
                .as_str()
                .map(|name| (name.to_owned(), String::new()))
        })
        .collect();
    Some(Node {
        id: id.clone(),
        kind: kind.into(),
        label: spec["label"].as_str().unwrap_or(kind).into(),
        params,
        inputs,
    })
}
fn parameter_unit(key: &str, kind: &str, value: &Value) -> &'static str {
    match key {
        "radius_m" | "depth_m" | "rim_m" | "wavelength_m" | "height_transition_m" => " [m]",
        "temperature_transition_k" => " [K]",
        "unit" => " [output unit]",
        "octaves" => " [count]",
        "persistence" | "humidity_bias" | "palette" => " [unitless / linear]",
        _ if kind == "noise" && value.as_str() == Some("K") => " [K]",
        _ => "",
    }
}
fn json_rgb(value: &Value) -> Option<[f64; 3]> {
    let colors = value.as_array()?;
    Some([
        colors.first()?.as_f64()?,
        colors.get(1)?.as_f64()?,
        colors.get(2)?.as_f64()?,
    ])
}
fn explicit_graph_path(raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw.trim());
    if raw.trim().is_empty() || !path.is_absolute() {
        return Err("Enter an explicit absolute graph file path".into());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Graph file path cannot contain parent traversal".into());
    }
    Ok(path)
}
pub(crate) fn validate_external_output(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err("Export path must be an explicit absolute external directory".into());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Export path cannot contain parent traversal".into());
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let checkout = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot identify source checkout root".to_owned())?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| "export path has no existing parent".to_owned())?
            .to_os_string();
        missing.push(name);
        ancestor = ancestor
            .parent()
            .ok_or_else(|| "export path has no existing parent".to_owned())?;
    }
    let mut resolved = ancestor.canonicalize().map_err(|error| error.to_string())?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    if resolved.starts_with(&checkout) {
        return Err("Export directory must remain outside the engine checkout".into());
    }
    Ok(resolved)
}
