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
        export: bool,
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
    /// Attribution control: stop atlas producer requests while drawing the resident cut.
    ResidentCoverHold {
        enabled: bool,
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
}
fn one() -> f64 {
    1.0
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
