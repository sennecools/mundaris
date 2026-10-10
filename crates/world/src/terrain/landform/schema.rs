//! Authored landform data (`content/landforms/*.ron`, pipeline App. C.3 and
//! M2 Shape design §2).
//!
//! A *landform set* lists landforms with a weight rule (an [`super::expr`]
//! expression over Tier A fields), a recipe file and an amplitude range. A
//! *recipe* is a named node graph; inputs are either a node name (`"ridge"`)
//! or an inline node (`Field(Flow)`, `Multiply(a: .., b: ..)`). The files are
//! parsed by the caller (RON in the app); [`super::LandformSet::compile`]
//! validates them into bytecode and [`super::Program`]s.
use serde::{Deserialize, Serialize};

/// Landform set file (`content/landforms/<set>.ron`, `LandformSet(..)`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename = "LandformSet")]
pub struct LandformSetFile {
    pub schema: u32,
    pub name: String,
    /// Shortest wavelength Tier A resolves (about two world-map texels).
    /// Recipe relief stacks must start at or below it, so they never
    /// double-count the eroded macro relief.
    pub relief_band_edge_m: f64,
    /// Divide the weights by their sum (they then sum to 1).
    pub normalize: bool,
    /// Landform that takes the remaining weight where the rules sum to less
    /// than [`super::expr::FALLBACK_FLOOR`].
    pub fallback: String,
    pub landforms: Vec<LandformEntry>,
}

/// One landform of a set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandformEntry {
    pub name: String,
    /// Weight rule, e.g. `"smoothstep(0.5, 0.8, uplift)"`.
    pub weight: String,
    /// Recipe path relative to the set file's directory.
    pub recipe: String,
    /// Inclusive `(min, max)` amplitude in metres, sampled per body; the
    /// recipe output is in units of this amplitude.
    pub amplitude_m: (f64, f64),
}

/// Recipe file (`content/landforms/recipes/<name>.ron`, `Recipe(..)`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename = "Recipe")]
pub struct RecipeFile {
    pub schema: u32,
    /// Name of the node whose value is the landform height (unit amplitude).
    pub output: String,
    pub graph: Vec<(String, Node)>,
}

/// A node input: a reference to a named node or an inline node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Input {
    Ref(String),
    Node(Box<Node>),
}

/// Tier A field readable by a recipe ([`super::FieldSource`]). Values are
/// clamped to [`RecipeField::range`] before use, so bounds stay sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecipeField {
    /// Orogenic uplift, 0..1.
    Uplift,
    /// Deposited sediment, 0..1.
    Sediment,
    /// Macro drainage proxy `flow / flow_max`, 0..1.
    Flow,
    /// Rock hardness, 0..1.
    Hardness,
    /// Moisture, 0..1.
    Moisture,
    /// Surface temperature, °C.
    Temperature,
    /// Volcanic mask, 0..1.
    Volcanic,
    /// Macro elevation above sea level, metres.
    Elevation,
    /// Signed plate-boundary distance `δ`, metres (Tier A clamps it to
    /// ±300 km).
    BoundaryCoord,
}

impl RecipeField {
    pub const ALL: [Self; 9] = [
        Self::Uplift,
        Self::Sediment,
        Self::Flow,
        Self::Hardness,
        Self::Moisture,
        Self::Temperature,
        Self::Volcanic,
        Self::Elevation,
        Self::BoundaryCoord,
    ];

    /// Stable id (structure hash, generated code).
    pub fn id(self) -> u32 {
        self as u32
    }

    /// Value range the evaluator clamps to and the bounds assume.
    pub fn range(self) -> (f64, f64) {
        match self {
            Self::Temperature => (-250.0, 500.0),
            Self::Elevation => (-30_000.0, 30_000.0),
            Self::BoundaryCoord => (-300_000.0, 300_000.0),
            _ => (0.0, 1.0),
        }
    }
}

/// Optional anisotropy of a noise stack via a 4th noise dimension
/// `q_w = f·kappa·δ` (design §3): the first `octaves` octaves stretch by
/// `stretch` along iso-`δ` lines (parallel to the plate boundary).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anisotropy {
    /// Power of two, 1..=16.
    pub stretch: u32,
    /// Power of two, 1/16..=16 (keeps `f·kappa` dyadic for the exact GPU split).
    pub kappa: f64,
    pub octaves: u32,
    /// `δ` is clamped to ±`clamp_km` before entering the noise.
    pub clamp_km: f64,
}

/// Recipe node. Noise stacks use lacunarity 2 on the dyadic ladder their
/// snapped base wavelength picks; octave amplitudes `gain^k` are normalised
/// to sum to 1, so a stack's output is in units of the landform amplitude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Node {
    /// Zero-mean fBm of gradient noise.
    Fbm {
        base_wavelength_m: f64,
        min_wavelength_m: f64,
        gain: f64,
        salt: u32,
        #[serde(default)]
        anisotropy: Option<Anisotropy>,
    },
    /// `max(1 − |n|, 0)^sharpness` per octave minus its measured mean.
    RidgedFbm {
        base_wavelength_m: f64,
        min_wavelength_m: f64,
        gain: f64,
        sharpness: f64,
        salt: u32,
        #[serde(default)]
        anisotropy: Option<Anisotropy>,
    },
    /// `|n|` per octave minus its measured mean.
    Billow {
        base_wavelength_m: f64,
        min_wavelength_m: f64,
        gain: f64,
        salt: u32,
        #[serde(default)]
        anisotropy: Option<Anisotropy>,
    },
    /// Domain warp of a noise stack by a 3-component fBm displacement.
    Warp {
        input: Input,
        strength_m: f64,
        base_wavelength_m: f64,
        octaves: u32,
        salt: u32,
    },
    /// Derivative damping of a noise stack: octave `k` is scaled by
    /// `1 / (1 + damp·s²)`, `s` the slope of the coarser octaves at full
    /// amplitude (pipeline §9.6).
    SlopeDamped {
        input: Input,
        damp: f64,
    },
    /// Gradient-aligned gullies over a noise stack (pipeline §9.6).
    ErosionFilter {
        input: Input,
        strength: f64,
        base_wavelength_m: f64,
        octaves: u32,
        salt: u32,
    },
    Field(RecipeField),
    Const(f64),
    Add {
        a: Input,
        b: Input,
    },
    Multiply {
        a: Input,
        b: Input,
    },
    /// `a + (b − a)·t`.
    Mix {
        a: Input,
        b: Input,
        t: Input,
    },
    Min {
        a: Input,
        b: Input,
    },
    Max {
        a: Input,
        b: Input,
    },
    /// Polynomial smooth minimum with blend width `k`.
    SmoothMin {
        a: Input,
        b: Input,
        k: f64,
    },
    Clamp {
        input: Input,
        min: f64,
        max: f64,
    },
    /// Monotone cubic (Fritsch–Carlson) through `(x, y)` points with strictly
    /// increasing x; flat outside.
    Curve {
        input: Input,
        points: Vec<(f64, f64)>,
    },
    Scale {
        input: Input,
        factor: f64,
    },
}
