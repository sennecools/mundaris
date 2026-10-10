//! Species browser model (docs/STUDIO_UI.md amendment 2026-10-10e): the
//! species files in `content/flora/species`, their edits, and a line-up of
//! grown variants made on a worker thread. Toolkit-free; the Studio shell
//! owns one and draws its [`SpeciesView`].
//!
//! Parameters come from the flora crate's descriptor tables
//! (`astrum_core::params`), so new genome parameters appear without UI code.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use astrum_core::params::{ParamDesc, ParamValue};
use astrum_flora::metrics::Bands;
use astrum_flora::raster::{Camera, Canvas, draw};
use astrum_flora::{Genome, Kit, PlantMetrics, SpeciesFile, grow_plant, variant_seed};
use glam::{Vec2, Vec3};

use super::view::{ParamEdit, ParamItem};

/// Thumbnail size in pixels.
pub const THUMB_W: usize = 168;
pub const THUMB_H: usize = 224;
/// Variants in a line-up.
pub const DEFAULT_VARIANTS: u32 = 6;
pub const MAX_VARIANTS: u32 = 12;

/// The species content directory of this checkout.
pub fn species_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora/species")
}

/// One species file and its edit.
#[derive(Debug, Clone)]
pub struct SpeciesEntry {
    pub path: PathBuf,
    /// Leading `//` comment block, kept on save.
    header: String,
    pub saved: SpeciesFile,
    pub edit: SpeciesFile,
}

impl SpeciesEntry {
    pub fn dirty(&self) -> bool {
        self.edit != self.saved
    }
}

/// One descriptor table inside a species file (the genome; the niche once the
/// flora crate has it). The browser lists the parts in order; a
/// [`ParamEdit`] index counts across them.
trait Part: Sync {
    fn len(&self) -> usize;
    fn item(&self, index: usize, edit: &SpeciesFile, saved: &SpeciesFile) -> ParamItem;
    fn value(&self, index: usize, file: &SpeciesFile) -> ParamValue;
    /// Checks `value` against the descriptor and stores it.
    fn set(&self, index: usize, file: &mut SpeciesFile, value: ParamValue) -> Result<(), String>;
}

struct Table<T: 'static> {
    get: fn(&SpeciesFile) -> &T,
    get_mut: fn(&mut SpeciesFile) -> &mut T,
    descs: fn() -> &'static [ParamDesc<T>],
}

impl<T: 'static> Part for Table<T> {
    fn len(&self) -> usize {
        (self.descs)().len()
    }

    fn item(&self, index: usize, edit: &SpeciesFile, saved: &SpeciesFile) -> ParamItem {
        let desc = &(self.descs)()[index];
        let value = (desc.get)((self.get)(edit));
        ParamItem {
            overridden: value != (desc.get)((self.get)(saved)),
            ..ParamItem::from_desc(desc, (self.get)(edit))
        }
    }

    fn value(&self, index: usize, file: &SpeciesFile) -> ParamValue {
        let desc = &(self.descs)()[index];
        (desc.get)((self.get)(file))
    }

    fn set(&self, index: usize, file: &mut SpeciesFile, value: ParamValue) -> Result<(), String> {
        let desc = &(self.descs)()[index];
        let value = desc
            .kind
            .check(value)
            .map_err(|error| format!("{}: {error}", desc.key))?;
        (desc.set)((self.get_mut)(file), value);
        Ok(())
    }
}

fn genome(file: &SpeciesFile) -> &Genome {
    &file.genome
}

fn genome_mut(file: &mut SpeciesFile) -> &mut Genome {
    &mut file.genome
}

fn genome_descs() -> &'static [ParamDesc<Genome>] {
    <Genome as astrum_core::params::Params>::descriptors()
}

static GENOME: Table<Genome> = Table {
    get: genome,
    get_mut: genome_mut,
    descs: genome_descs,
};

/// The species file's descriptor tables, in panel order.
fn parts() -> [&'static dyn Part; 1] {
    [&GENOME]
}

/// The part holding parameter `index` and the index inside it.
fn locate(index: usize) -> Option<(&'static dyn Part, usize)> {
    let mut rest = index;
    for part in parts() {
        if rest < part.len() {
            return Some((part, rest));
        }
        rest -= part.len();
    }
    None
}

/// A raster thumbnail: sRGB bytes, three per pixel, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    pub rgb: Arc<[u8]>,
}

/// One grown variant of the line-up.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub index: u32,
    pub seed: u64,
    pub thumbnail: Thumbnail,
    pub metrics: PlantMetrics,
    pub grow_ms: f64,
    /// Red flags against `Bands::HERO`.
    pub failures: Vec<String>,
}

/// A grown line-up of one species edit.
#[derive(Debug, Clone, PartialEq)]
pub struct LineUp {
    /// Request counter; textures re-upload when it changes.
    pub generation: u64,
    pub species: String,
    pub variants: Vec<Variant>,
    /// Wall time of the whole line-up (grow and raster, in parallel).
    pub total_ms: f64,
}

struct Request {
    generation: u64,
    species: SpeciesFile,
    variants: u32,
}

/// Background grower: one thread, latest request wins.
struct Worker {
    requests: mpsc::Sender<Request>,
    results: mpsc::Receiver<LineUp>,
}

impl Worker {
    fn spawn() -> Option<Self> {
        let (requests, inbox) = mpsc::channel::<Request>();
        let (outbox, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("studio-species-grower".into())
            .spawn(move || {
                let kit = Kit::builtin();
                while let Ok(mut request) = inbox.recv() {
                    while let Ok(newer) = inbox.try_recv() {
                        request = newer;
                    }
                    if outbox.send(grow_lineup(&request, &kit)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self { requests, results })
    }
}

/// Grows `request.variants` individuals in parallel and rasters LOD 0 of
/// each at one shared scale (as the flora contact sheets do).
fn grow_lineup(request: &Request, kit: &Kit) -> LineUp {
    let started = Instant::now();
    let species = &request.species;
    let grown: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..request.variants)
            .map(|index| {
                scope.spawn(move || {
                    let seed = variant_seed(species, index);
                    let t = Instant::now();
                    let plant = grow_plant(species, kit, seed);
                    (index, seed, plant, t.elapsed().as_secs_f64() * 1e3)
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });
    let (mut height, mut width) = (0.1_f32, 0.1_f32);
    for (_, _, plant, _) in &grown {
        let (lo, hi) = plant.lods[0].bounds();
        height = height.max(hi.z - lo.z.min(0.0));
        width = width.max((hi.x - lo.x).max(hi.y - lo.y));
    }
    let ground = THUMB_H as f32 - 10.0;
    let px_per_m = ((ground - 12.0) / height).min((THUMB_W as f32 - 10.0) / width);
    let variants = grown
        .into_iter()
        .map(|(index, seed, plant, grow_ms)| {
            let mut canvas = Canvas::new(THUMB_W, THUMB_H, Vec3::new(0.62, 0.68, 0.74));
            canvas.fill_rect(
                0,
                ground as usize,
                THUMB_W,
                THUMB_H,
                Vec3::new(0.16, 0.13, 0.1),
            );
            let camera = Camera {
                azimuth: 0.5,
                elevation: 0.12,
                target: Vec3::ZERO,
                anchor_px: Vec2::new(THUMB_W as f32 * 0.5, ground),
                px_per_m,
                clip: [0, 0, THUMB_W, THUMB_H],
            };
            draw(&mut canvas, &plant.lods[0], &camera);
            Variant {
                index,
                seed,
                thumbnail: Thumbnail {
                    width: THUMB_W,
                    height: THUMB_H,
                    rgb: canvas.to_srgb8().into(),
                },
                failures: plant.metrics.failures(&Bands::HERO),
                metrics: plant.metrics,
                grow_ms,
            }
        })
        .collect();
    LineUp {
        generation: request.generation,
        species: species.name.clone(),
        variants,
        total_ms: started.elapsed().as_secs_f64() * 1e3,
    }
}

/// Species browser actions (besides parameter edits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeciesAction {
    Select(usize),
    Save,
    Revert,
    /// Number of variants in the line-up.
    Variants(u32),
}

/// What the species panels draw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpeciesView {
    /// File stem and unsaved flag per species.
    pub species: Vec<(String, bool)>,
    pub selected: usize,
    pub file: String,
    pub dirty: bool,
    /// Every editable parameter of the selected species (genome first).
    pub params: Vec<ParamItem>,
    pub variants: u32,
    /// A line-up for the latest edit is being grown.
    pub growing: bool,
    pub lineup: Option<Arc<LineUp>>,
    /// Load, save or validation problem.
    pub error: Option<String>,
}

pub struct SpeciesEditor {
    dir: PathBuf,
    entries: Vec<SpeciesEntry>,
    selected: usize,
    variants: u32,
    worker: Option<Worker>,
    /// Latest requested and latest received line-up.
    requested: u64,
    lineup: Option<Arc<LineUp>>,
    error: Option<String>,
    started: bool,
}

impl SpeciesEditor {
    /// Reads every species in `dir`. Nothing grows until [`Self::activate`].
    pub fn load(dir: &Path) -> Self {
        let mut editor = Self {
            dir: dir.to_path_buf(),
            entries: Vec::new(),
            selected: 0,
            variants: DEFAULT_VARIANTS,
            worker: None,
            requested: 0,
            lineup: None,
            error: None,
            started: false,
        };
        editor.reload();
        editor
    }

    fn reload(&mut self) {
        self.entries.clear();
        let mut paths: Vec<PathBuf> = match std::fs::read_dir(&self.dir) {
            Ok(dir) => dir
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension().is_some_and(|ext| ext == "ron")
                        && path.file_name().is_some_and(|name| name != "kit.ron")
                })
                .collect(),
            Err(error) => {
                self.error = Some(format!("{}: {error}", self.dir.display()));
                return;
            }
        };
        paths.sort();
        let mut problems = Vec::new();
        for path in paths {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    problems.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            match SpeciesFile::from_ron(&text) {
                Ok(file) => self.entries.push(SpeciesEntry {
                    header: leading_comments(&text),
                    path,
                    saved: file.clone(),
                    edit: file,
                }),
                Err(error) => problems.push(format!("{}: {error}", file_name(&path))),
            }
        }
        self.error = (!problems.is_empty()).then(|| problems.join("; "));
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
    }

    pub fn entries(&self) -> &[SpeciesEntry] {
        &self.entries
    }

    /// Starts growing (first time the browser is shown) and collects results.
    pub fn activate(&mut self) {
        if !self.started {
            self.started = true;
            self.worker = Worker::spawn();
            if self.worker.is_none() {
                self.error = Some("could not start the species grower thread".into());
            }
            self.request();
        }
        self.poll();
    }

    /// Takes finished line-ups; keeps only the newest.
    pub fn poll(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        while let Ok(lineup) = worker.results.try_recv() {
            self.lineup = Some(Arc::new(lineup));
        }
    }

    fn request(&mut self) {
        let (Some(worker), Some(entry)) = (&self.worker, self.entries.get(self.selected)) else {
            return;
        };
        self.requested += 1;
        let _ = worker.requests.send(Request {
            generation: self.requested,
            species: entry.edit.clone(),
            variants: self.variants,
        });
    }

    /// Blocks until the line-up of the latest request arrives (tests and
    /// headless tools).
    pub fn wait(&mut self) -> Option<Arc<LineUp>> {
        let worker = self.worker.as_ref()?;
        while self
            .lineup
            .as_ref()
            .is_none_or(|lineup| lineup.generation < self.requested)
        {
            self.lineup = Some(Arc::new(worker.results.recv().ok()?));
        }
        self.lineup.clone()
    }

    pub fn action(&mut self, action: SpeciesAction) {
        match action {
            SpeciesAction::Select(index) if index < self.entries.len() => {
                if index != self.selected {
                    self.selected = index;
                    self.request();
                }
            }
            SpeciesAction::Select(_) => {}
            SpeciesAction::Variants(count) => {
                let count = count.clamp(1, MAX_VARIANTS);
                if count != self.variants {
                    self.variants = count;
                    self.request();
                }
            }
            SpeciesAction::Revert => {
                if let Some(entry) = self.entries.get_mut(self.selected) {
                    entry.edit = entry.saved.clone();
                    self.error = None;
                    self.request();
                }
            }
            SpeciesAction::Save => {
                if let Err(error) = self.save() {
                    self.error = Some(error);
                }
            }
        }
    }

    fn save(&mut self) -> Result<(), String> {
        let entry = self
            .entries
            .get_mut(self.selected)
            .ok_or("no species selected")?;
        entry
            .edit
            .genome
            .validate()
            .map_err(|error| error.to_string())?;
        let text = format!("{}{}\n", entry.header, entry.edit.to_ron());
        // Never write a file that would not load again.
        SpeciesFile::from_ron(&text).map_err(|error| error.to_string())?;
        std::fs::write(&entry.path, text)
            .map_err(|error| format!("{}: {error}", entry.path.display()))?;
        entry.saved = entry.edit.clone();
        self.error = None;
        Ok(())
    }

    /// Applies a parameter edit to the selected species and regrows.
    pub fn edit(&mut self, edit: ParamEdit) {
        let Some(entry) = self.entries.get_mut(self.selected) else {
            return;
        };
        let (index, value) = match edit {
            ParamEdit::Set(index, value) => (index, Some(value)),
            ParamEdit::Reset(index) => (index, None),
        };
        let Some((part, index)) = locate(index) else {
            return;
        };
        let value = value.unwrap_or_else(|| part.value(index, &entry.saved));
        if part.value(index, &entry.edit) == value {
            return;
        }
        match part.set(index, &mut entry.edit, value) {
            Ok(()) => {
                self.error = None;
                self.request();
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn view(&self) -> SpeciesView {
        let entry = self.entries.get(self.selected);
        let params = entry.map_or_else(Vec::new, |entry| {
            parts()
                .into_iter()
                .flat_map(|part| {
                    (0..part.len()).map(move |index| part.item(index, &entry.edit, &entry.saved))
                })
                .collect()
        });
        SpeciesView {
            species: self
                .entries
                .iter()
                .map(|entry| (file_stem(&entry.path), entry.dirty()))
                .collect(),
            selected: self.selected,
            file: entry.map_or_else(String::new, |entry| {
                format!("content/flora/species/{}", file_name(&entry.path))
            }),
            dirty: entry.is_some_and(SpeciesEntry::dirty),
            params,
            variants: self.variants,
            growing: self.started
                && self
                    .lineup
                    .as_ref()
                    .is_none_or(|lineup| lineup.generation < self.requested),
            lineup: self.lineup.clone(),
            error: self.error.clone(),
        }
    }
}

fn leading_comments(text: &str) -> String {
    let mut header = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("//") {
            header.push_str(line);
            header.push('\n');
        } else {
            break;
        }
    }
    header
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("astrum-species-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for entry in std::fs::read_dir(species_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "ron") {
                std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
            }
        }
        dir
    }

    #[test]
    fn loads_edits_saves_and_keeps_the_header() {
        let dir = scratch("save");
        let mut editor = SpeciesEditor::load(&dir);
        assert!(editor.error.is_none(), "{:?}", editor.error);
        assert!(!editor.entries().is_empty());
        let view = editor.view();
        assert_eq!(
            view.params.len(),
            parts().iter().map(|part| part.len()).sum::<usize>()
        );
        let height = view
            .params
            .iter()
            .position(|item| item.key == "height_m")
            .unwrap();
        let before = editor.entries()[0].edit.genome.height_m;
        editor.edit(ParamEdit::Set(height, ParamValue::Float(before * 0.5)));
        assert!(editor.view().dirty);
        assert!(editor.view().params[height].overridden);
        // Out of range is refused and reported.
        editor.edit(ParamEdit::Set(height, ParamValue::Float(1.0e6)));
        assert!(editor.view().error.is_some());
        editor.action(SpeciesAction::Save);
        assert!(!editor.view().dirty, "{:?}", editor.view().error);
        let path = &editor.entries()[0].path;
        let text = std::fs::read_to_string(path).unwrap();
        let original =
            std::fs::read_to_string(species_dir().join(path.file_name().unwrap())).unwrap();
        assert_eq!(leading_comments(&text), leading_comments(&original));
        let reloaded = SpeciesFile::from_ron(&text).unwrap();
        assert_eq!(reloaded.genome.height_m, before * 0.5);
        editor.edit(ParamEdit::Reset(height));
        assert!(!editor.view().dirty);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn line_up_grows_on_a_worker_and_follows_the_latest_edit() {
        let dir = scratch("grow");
        let mut editor = SpeciesEditor::load(&dir);
        editor.action(SpeciesAction::Variants(3));
        editor.activate();
        let first = editor.wait().unwrap();
        assert_eq!(first.variants.len(), 3);
        let thumb = &first.variants[0].thumbnail;
        assert_eq!(thumb.rgb.len(), thumb.width * thumb.height * 3);
        // Plant pixels differ from the backdrop.
        assert!(thumb.rgb.chunks(3).any(|px| px[0] < 100));
        let cycles = editor
            .view()
            .params
            .iter()
            .position(|item| item.key == "cycles")
            .unwrap();
        editor.edit(ParamEdit::Set(cycles, ParamValue::Int(4)));
        editor.edit(ParamEdit::Set(cycles, ParamValue::Int(5)));
        let latest = editor.wait().unwrap();
        assert!(latest.generation > first.generation);
        assert!(!editor.view().growing);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
