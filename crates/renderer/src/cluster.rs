//! Opt-in, bounded finite-region cluster prototype presentation.
//!
//! These settings never mutate surface authority or the planetary scheduler.
use glam::{DMat3, DVec3};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClusterMode {
    #[default]
    Reference,
    Culling,
    Lod,
}
impl ClusterMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Reference => "Resident reference",
            Self::Culling => "Cluster culling only",
            Self::Lod => "Cluster LOD",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClusterDebug {
    #[default]
    Lit,
    Clusters,
    Lod,
    Residency,
}
impl ClusterDebug {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lit => "Lit terrain",
            Self::Clusters => "Actual cluster colors",
            Self::Lod => "Drawn cluster detail",
            Self::Residency => "Residency / fallback",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClusterSettings {
    pub mode: ClusterMode,
    pub debug: ClusterDebug,
    pub triangle_edges: bool,
    pub cluster_edges: bool,
    pub freeze: bool,
}

/// Current rendering transform, distinct from an optionally frozen selection view.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RegionView {
    pub slot: usize,
    pub anchor_view_m: DVec3,
    pub body_to_view: DMat3,
    pub sun_body: DVec3,
    pub appearance: crate::ResidentMaterialAppearance,
    pub quality_fallback: bool,
    pub pending_finer: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ClusterReport {
    pub enabled: bool,
    pub active_mode: ClusterMode,
    pub freeze_active: bool,
    pub resident_regions: usize,
    pub pending_regions: usize,
    pub selected_clusters: usize,
    pub selected_fine_clusters: usize,
    pub selected_coarse_clusters: usize,
    pub selected_transition_clusters: usize,
    pub resident_clusters: usize,
    pub submitted_triangles: usize,
    pub draw_commands: usize,
    pub cpu_bytes: usize,
    pub gpu_bytes: u64,
    pub build_micros: u64,
    pub selection_cpu_micros: u64,
    pub all_build_micros: u64,
    pub queue_wait_micros: u64,
    pub max_queue_wait_micros: u64,
    pub completed_builds: u64,
    pub failed_builds: u64,
    pub evicted_before_selection: u64,
    pub upload_bytes: u64,
    pub gpu_selection_ms: Option<f64>,
    pub gpu_render_ms: Option<f64>,
    pub fallback_regions: usize,
    pub cache_hits: u64,
    pub rebuilds: u64,
    pub cancelled: u64,
    pub reason: Option<String>,
    pub counters_scope: &'static str,
}

pub const REGION_CAP: usize = 32;
pub const CPU_PRODUCT_CAP_BYTES: usize = 64 * 1024 * 1024;
pub const GPU_CAP_BYTES: u64 = 64 * 1024 * 1024;
// Conservative reservation for one compiler; allocator peak is not instrumented.
pub const BUILDER_SCRATCH_CAP_BYTES: usize = 72 * 1024 * 1024;
pub const BUILD_QUEUE_CAP: usize = 4;
pub const COMPLETION_CAP: usize = 4;
pub const PUBLICATION_CAP_PER_FRAME: usize = 1;
/// Maximum geometric pixel displacement of the finite resident input mesh.
/// This is separate from complete-world and lighting/appearance acceptance.
pub const FINITE_MESH_ERROR_PX: f64 = 0.15;
pub const FINITE_NORMAL_ERROR_RAD: f64 = 0.02;
pub const FINITE_MATERIAL_ERROR: f64 = 0.01;
