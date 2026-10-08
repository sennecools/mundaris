//! Versioned development operations; all engine mutation remains app-owned.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
pub const REQUEST_CAPACITY: usize = 64;
pub const COMMANDS_PER_TURN: usize = 16;
pub const HISTORY_CAPACITY: usize = 1024;
pub const LEASE_SECONDS: u64 = 30;
pub const MAX_MESSAGE_BYTES: usize = 65_536;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDescriptor {
    pub protocol_version: u32,
    pub session_id: String,
    pub endpoint: String,
    pub pid: u32,
    pub preset: String,
    pub executable: String,
    pub output_directory: String,
    pub binary_sha256: String,
    pub build_manifest: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevRequest {
    pub protocol_version: u32,
    pub session_id: String,
    pub request_id: String,
    pub operation: DevOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DevOperation {
    Capabilities,
    Inspect,
    Diagnostics { scope: String },
    AcquireControl { owner: String },
    RenewControl { lease: String },
    ReleaseControl { lease: String },
    Action { lease: String, command: DevCommand },
    Receipt { command_id: String },
    Events { after: u64 },
    Capture { lease: String, name: String },
    Cancel { lease: String },
    Shutdown { lease: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DevCommand {
    /// Diagnostic-only profiler controls. Freeze retains the displayed data while
    /// the simulation continues. Export queues a bounded background write.
    Profiler {
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        freeze: Option<bool>,
        #[serde(default)]
        view: Option<String>,
        #[serde(default)]
        frame: Option<u64>,
        #[serde(default)]
        event: Option<u64>,
        #[serde(default)]
        export: bool,
        #[serde(default)]
        zoom: Option<f64>,
        #[serde(default)]
        pan: Option<f64>,
        #[serde(default)]
        fill_window: Option<bool>,
    },
    Select {
        body: String,
    },
    Focus {
        body: String,
        #[serde(default)]
        body_fixed: bool,
    },
    Overview,
    LookAt {
        body: String,
    },
    NavigationMode {
        mode: String,
    },
    Navigation {
        #[serde(default)]
        drag: [f64; 2],
        #[serde(default)]
        scroll_notches: f64,
        #[serde(default)]
        translation: [f64; 3],
        #[serde(default = "one")]
        speed_multiplier: f64,
        #[serde(default = "one")]
        boost_multiplier: f64,
        #[serde(default)]
        duration_s: f64,
    },
    Clearance {
        meters: f64,
    },
    /// Attribution control: stop preparation/publication while drawing the retained cover.
    ResidentCoverHold {
        enabled: bool,
    },
    ClusterRendering {
        mode: String,
        #[serde(default = "cluster_lit")]
        debug: String,
        #[serde(default)]
        triangle_edges: bool,
        #[serde(default)]
        cluster_edges: bool,
        #[serde(default)]
        freeze: bool,
    },
    /// Reproducible developer capture pose in the selected body's f64 fixed frame.
    /// The ordinary surface controller and complete-source clearance remain active.
    SurfacePose {
        body: String,
        position_body_m: [f64; 3],
        orientation_xyzw: [f64; 4],
    },
    Pause {
        paused: bool,
    },
    Rate {
        multiplier: f64,
    },
    Seek {
        seconds: f64,
    },
    SingleStep {
        #[serde(default = "yes")]
        forward: bool,
    },
    Reset,
    RenderMode {
        mode: String,
    },
    Layer {
        layer: String,
        enabled: bool,
    },
    SkySetting {
        setting: String,
        value: f64,
    },
    /// Opt-in single resident GPU tile comparison fixture. Omitted controls use
    /// the canonical fixed acceptance region; `enabled` defaults to false.
    GpuTile {
        #[serde(default)]
        enabled: bool,
        #[serde(default = "default_tile_family")]
        family: String,
        #[serde(default)]
        seed: u64,
        #[serde(default = "default_tile_radius")]
        radius_m: f64,
        #[serde(default = "default_tile_face")]
        face: String,
        #[serde(default = "default_tile_level")]
        level: u8,
        #[serde(default = "default_tile_x")]
        x: u32,
        #[serde(default = "default_tile_y")]
        y: u32,
        #[serde(default = "default_tile_cells")]
        cells: u32,
        #[serde(default)]
        revision: u64,
    },
    /// Change presentation/camera/light controls without rebuilding tile content.
    GpuTileView {
        #[serde(default)]
        mode: u8,
        #[serde(default)]
        camera_offset_m: [f64; 3],
        #[serde(default = "default_sun_direction")]
        sun_direction_body: [f64; 3],
        #[serde(default)]
        reference_cpu: bool,
    },
    /// Opt-in fixed parent/four-child hierarchy fixture. Child CPU generation is
    /// asynchronous; the parent remains drawable until the renderer publishes a
    /// coherent child set.
    GpuHierarchy {
        #[serde(default)]
        enabled: bool,
        #[serde(default = "yes")]
        refine: bool,
        #[serde(default = "default_hierarchy_morph_duration_ms")]
        morph_duration_ms: u64,
        #[serde(default)]
        child_delays_ms: [u64; 4],
        #[serde(default = "default_hierarchy_request_mask")]
        request_mask: u8,
        #[serde(default)]
        cancel_pending: bool,
        #[serde(default)]
        diagnostic_validate: bool,
    },
    /// Opt-in regional adaptive GPU terrain route. All budgets are prototype
    /// admission controls and remain observable through `resident_regional`.
    GpuRegional {
        #[serde(default)]
        enabled: bool,
        #[serde(default = "default_regional_max_depth")]
        max_depth: u8,
        #[serde(default = "default_regional_gpu_slots")]
        gpu_slots: usize,
        #[serde(default = "default_regional_cpu_tiles")]
        cpu_tiles: usize,
        #[serde(default = "default_regional_worker_count")]
        worker_count: usize,
        #[serde(default)]
        worker_delay_ms: u64,
        #[serde(default = "default_regional_upload_tiles_per_frame")]
        upload_tiles_per_frame: usize,
        #[serde(default = "default_regional_upload_bytes_per_frame")]
        upload_bytes_per_frame: u64,
        #[serde(default = "default_regional_publication_groups_per_frame")]
        publication_groups_per_frame: usize,
        #[serde(default = "default_regional_transition_limit")]
        transition_limit: usize,
        #[serde(default = "default_hierarchy_morph_duration_ms")]
        morph_duration_ms: u64,
        #[serde(default = "default_regional_split_error_px")]
        split_error_px: f64,
        #[serde(default = "default_regional_merge_error_px")]
        merge_error_px: f64,
    },
}
fn default_tile_family() -> String {
    "rocky_v5".into()
}
fn default_tile_radius() -> f64 {
    80_000.0
}
fn default_tile_face() -> String {
    "positive_z".into()
}
fn default_tile_level() -> u8 {
    9
}
fn default_tile_x() -> u32 {
    157
}
fn default_tile_y() -> u32 {
    39
}
fn default_tile_cells() -> u32 {
    64
}
fn default_sun_direction() -> [f64; 3] {
    [0.3, 0.8, 0.5]
}
fn default_hierarchy_morph_duration_ms() -> u64 {
    150
}
fn default_hierarchy_request_mask() -> u8 {
    0b1111
}
fn default_regional_max_depth() -> u8 {
    3
}
fn default_regional_gpu_slots() -> usize {
    128
}
fn default_regional_cpu_tiles() -> usize {
    256
}
fn default_regional_worker_count() -> usize {
    4
}
fn default_regional_upload_tiles_per_frame() -> usize {
    2
}
fn default_regional_upload_bytes_per_frame() -> u64 {
    1_048_576
}
fn default_regional_publication_groups_per_frame() -> usize {
    2
}
fn default_regional_transition_limit() -> usize {
    4
}
fn default_regional_split_error_px() -> f64 {
    2.0
}
fn default_regional_merge_error_px() -> f64 {
    1.0
}
fn one() -> f64 {
    1.0
}
fn cluster_lit() -> String {
    "lit".into()
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevResponse {
    pub protocol_version: u32,
    pub session_id: String,
    pub request_id: String,
    pub status: String,
    pub data: Value,
}

impl DevResponse {
    pub fn new(session: &str, request: &str, status: &str, data: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            session_id: session.into(),
            request_id: request.into(),
            status: status.into(),
            data,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevReceipt {
    pub command_id: String,
    pub sequence: u64,
    pub status: String,
    pub world_revision: u64,
    pub prepared_frame: Option<u64>,
    pub submitted_frame: Option<u64>,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyInventory {
    pub handle: String,
    pub semantic_identity: Option<String>,
    pub name: String,
    pub mass_kg: f64,
    pub reference_radius_m: f64,
    pub position_m: [f64; 3],
    pub velocity_m_s: [f64; 3],
    pub surface_available: bool,
    #[serde(default)]
    pub state_reference_frame: String,
    #[serde(default)]
    pub supported_operations: Vec<String>,
}
