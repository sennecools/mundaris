//! Species genome (genesis design §4.3) and the species content file.
//!
//! A genome holds the *parameters of growth rules*, never a shape. The grower
//! (`grow.rs`) turns it into a branch skeleton by simulating bud competition
//! for space; the structural rules (space seeking, pipe-model thickness,
//! shedding of starved branches) cannot be switched off from here. "Alien"
//! lives in the discrete archetype choices and off-Earth constants
//! (phyllotaxis, pipe exponent, negative gravitropism, three buds per node).
//!
//! Content: `content/flora/<species>.ron`, one [`SpeciesFile`] each.

use serde::{Deserialize, Serialize};

use crate::params::{ParamDesc, ParamKind, ParamValue, Params};

pub const SCHEMA: u32 = 1;

/// One species: growth genome, climate niche (placement), and look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeciesFile {
    pub schema: u32,
    pub name: String,
    pub genome: Genome,
    /// Alien body plan; when set it replaces the tree/shrub grower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::bodyplan::PlanGenome>,
    pub niche: crate::niche::Niche,
    /// Hand-set colours replacing the planet palette (art direction only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_override: Option<Look>,
    /// Resolved colours: the planet palette (`palette::apply`) or the
    /// override. Not stored.
    #[serde(skip)]
    pub look: Look,
    /// Opaque crown areas (m², top and side) of the grown LOD0 at scale 1
    /// (`scatter::crown_areas`), set by `measure_crowns`. Not stored.
    #[serde(skip)]
    pub crown: Option<[f64; 2]>,
    /// Foliage build, from the planet file (`palette::apply`). Not stored.
    #[serde(skip)]
    pub style: FoliageStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrowthForm {
    /// Single trunk, clear bole, crown above (space-competition grower).
    Tree,
    /// Several stems from the base, crown down to the ground.
    Shrub,
    /// Few thick upright stems, sparse branching (succulent/cactus-like).
    Columnar,
    /// Low dome of short dense shoots.
    Cushion,
    /// Blades rising from one point (grass, sedge).
    Tuft,
    /// Organs in a whorl at the ground, optional central stalk.
    Rosette,
    /// Stalk and cap.
    FungalCap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrownShape {
    Ellipsoid,
    /// Widest at the bottom (shed snow, conifers).
    Cone,
    /// Wide flat top (savanna, acacia-like).
    Umbrella,
    Cylinder,
    /// Dome, widest at the base of the crown.
    Hemisphere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisMode {
    /// The apical bud keeps leading the axis.
    Monopodial,
    /// The apex stops after each cycle; the strongest lateral takes over.
    Sympodial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossSection {
    Round,
    Ribbed,
    Triangular,
    Flattened,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceStyle {
    Smooth,
    Ringed,
    Jointed,
    Scaled,
    Spiralled,
}

/// Leaf/organ archetype; the shape comes from the kit (`kit.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrganKind {
    /// No organs (bare branches).
    Leafless,
    Blade,
    Needle,
    Frond,
    Fan,
    Bulb,
    Tube,
    Membrane,
    Filament,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrganOrientation {
    /// Face the light (up and out).
    TowardLight,
    /// Stand upright.
    Vertical,
    /// Hang down.
    Pendant,
    /// Follow the branch.
    Along,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FruitKind {
    Barren,
    Berry,
    Pod,
    SporeBody,
    Bloom,
}

/// Growth genome (about 35 parameters, design §4.3). Lengths in metres,
/// angles in degrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Genome {
    // Architecture
    pub form: GrowthForm,
    /// Target mature height.
    pub height_m: f64,
    /// Stems from the base (1 for a tree).
    pub stems: u32,
    /// Fraction of the height below the crown (clear bole), 0..0.9.
    pub crown_base: f64,
    /// Crown width / crown height.
    pub crown_aspect: f64,
    pub crown_shape: CrownShape,
    /// Borchert–Honda λ: share of resource kept by the main axis, 0.3..0.8.
    pub apical_dominance: f64,
    pub axis_mode: AxisMode,
    /// Resource per unit of free space (Palubicki α), 0.5..5.
    pub vigor: f64,
    /// Growth cycles simulated.
    pub cycles: u32,
    // Branching
    /// Lateral buds per node, 1..3.
    pub buds_per_node: u32,
    /// Branch angle from the parent axis at order 1.
    pub branch_angle_deg: f64,
    /// Change of the branch angle per order.
    pub branch_angle_per_order_deg: f64,
    /// Divergence between successive nodes (golden angle 137.5 on Earth).
    pub phyllotaxis_deg: f64,
    pub internode_m: f64,
    /// Internode length factor per order.
    pub internode_per_order: f64,
    pub max_order: u32,
    // Tropism
    /// Weight of the free-space direction (Palubicki ξ).
    pub space_seeking: f64,
    /// Weight of the up direction; negative droops.
    pub gravitropism: f64,
    /// Change of gravitropism per branch order (orthotropic leader,
    /// plagiotropic laterals).
    pub gravitropism_per_order: f64,
    /// Weight of the current direction (stiffness of the shoot path).
    pub straightness: f64,
    /// Lean along +x (prevailing wind).
    pub wind_bias: f64,
    /// Random bend per internode, 0..1.
    pub wobble: f64,
    // Biomechanics
    /// Pipe model exponent n in r_parent^n = Σ r_child^n.
    pub pipe_exponent: f64,
    /// Radius of a terminal internode.
    pub tip_radius_m: f64,
    /// Base flare: extra radius factor at the root, 0..1.
    pub flare: f64,
    /// Wind stiffness, 0 (whippy) .. 1 (rigid).
    pub stiffness: f64,
    /// Shed branches whose free space per internode falls below this, 0..1.
    pub shed_threshold: f64,
    // Segments
    pub cross_section: CrossSection,
    pub surface: SurfaceStyle,
    // Organs
    pub organ: OrganKind,
    pub organ_size_m: f64,
    /// Organs per bearing node.
    pub organ_cluster: u32,
    pub organ_orientation: OrganOrientation,
    /// Organs on nodes within this many internodes of a tip.
    pub organ_reach: u32,
    // Reproduction
    pub fruit: FruitKind,
    pub fruit_size_m: f64,
    /// Chance per tip, 0..1.
    pub fruit_chance: f64,
}

/// How foliage is built (art direction 2026-10-10: stylised by default; the
/// realistic leaves stay selectable for comparison).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FoliageStyle {
    /// A few big soft clumps with rounded normals and a gradient inside the
    /// crown; chunky, lower-poly branches.
    #[default]
    Stylised,
    /// Individual kit organs (leaves, needles) on the full branch tree.
    Realistic,
}

/// Species colours (linear RGB), from the planet palette (`palette.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Look {
    pub bark: [f32; 3],
    pub organ: [f32; 3],
    /// Organ colour at the tips (gradient from `organ`).
    pub organ_tip: [f32; 3],
    pub accent: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GenomeError {
    #[error("schema {0} unsupported (expected {SCHEMA})")]
    Schema(u32),
    #[error("parse: {0}")]
    Parse(String),
    #[error("`{0}` out of range")]
    Range(&'static str),
}

impl SpeciesFile {
    pub fn from_ron(text: &str) -> Result<Self, GenomeError> {
        let file: SpeciesFile =
            ron::from_str(text).map_err(|e| GenomeError::Parse(e.to_string()))?;
        if file.schema != SCHEMA {
            return Err(GenomeError::Schema(file.schema));
        }
        file.genome.validate()?;
        file.niche.validate().map_err(GenomeError::Range)?;
        if let Some(p) = &file.plan {
            p.validate().map_err(GenomeError::Range)?;
        }
        Ok(file)
    }

    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .expect("species serialises")
    }
}

impl Genome {
    /// Checks every parameter against its descriptor range.
    pub fn validate(&self) -> Result<(), GenomeError> {
        for d in Self::descriptors() {
            if d.kind.check((d.get)(self)).is_err() {
                return Err(GenomeError::Range(d.key));
            }
        }
        Ok(())
    }
}

const FORMS: &[&str] = &[
    "Tree",
    "Shrub",
    "Columnar",
    "Cushion",
    "Tuft",
    "Rosette",
    "FungalCap",
];
const CROWNS: &[&str] = &["Ellipsoid", "Cone", "Umbrella", "Cylinder", "Hemisphere"];
const AXES: &[&str] = &["Monopodial", "Sympodial"];
const SECTIONS: &[&str] = &["Round", "Ribbed", "Triangular", "Flattened"];
const SURFACES: &[&str] = &["Smooth", "Ringed", "Jointed", "Scaled", "Spiralled"];
const ORGANS: &[&str] = &[
    "Leafless", "Blade", "Needle", "Frond", "Fan", "Bulb", "Tube", "Membrane", "Filament",
];
const ORIENTS: &[&str] = &["TowardLight", "Vertical", "Pendant", "Along"];
const FRUITS: &[&str] = &["Barren", "Berry", "Pod", "SporeBody", "Bloom"];

macro_rules! choice_enum {
    ($t:ty, [$($v:ident),*]) => {
        impl $t {
            pub const ALL: &'static [$t] = &[$(<$t>::$v),*];
            pub fn index(self) -> usize { Self::ALL.iter().position(|v| *v == self).unwrap() }
        }
    };
}
choice_enum!(
    GrowthForm,
    [Tree, Shrub, Columnar, Cushion, Tuft, Rosette, FungalCap]
);
choice_enum!(
    CrownShape,
    [Ellipsoid, Cone, Umbrella, Cylinder, Hemisphere]
);
choice_enum!(AxisMode, [Monopodial, Sympodial]);
choice_enum!(CrossSection, [Round, Ribbed, Triangular, Flattened]);
choice_enum!(SurfaceStyle, [Smooth, Ringed, Jointed, Scaled, Spiralled]);
choice_enum!(
    OrganKind,
    [
        Leafless, Blade, Needle, Frond, Fan, Bulb, Tube, Membrane, Filament
    ]
);
choice_enum!(OrganOrientation, [TowardLight, Vertical, Pendant, Along]);
choice_enum!(FruitKind, [Barren, Berry, Pod, SporeBody, Bloom]);

macro_rules! genome_params {
    ($($group:literal: $key:ident: $kind:ident ($($a:tt)*) $unit:literal $help:literal;)*) => {
        &[$(genome_params!(@one $group, $key, $kind($($a)*), $unit, $help)),*]
    };
    (@one $group:literal, $key:ident, float($min:expr, $max:expr), $unit:literal, $help:literal) => {
        ParamDesc { key: stringify!($key), label: "", group: $group, unit: $unit, help: $help,
            kind: ParamKind::Float { min: $min, max: $max, log: false },
            get: |g| ParamValue::Float(g.$key),
            set: |g, v| if let ParamValue::Float(v) = v { g.$key = v } }
    };
    (@one $group:literal, $key:ident, logf($min:expr, $max:expr), $unit:literal, $help:literal) => {
        ParamDesc { key: stringify!($key), label: "", group: $group, unit: $unit, help: $help,
            kind: ParamKind::Float { min: $min, max: $max, log: true },
            get: |g| ParamValue::Float(g.$key),
            set: |g, v| if let ParamValue::Float(v) = v { g.$key = v } }
    };
    (@one $group:literal, $key:ident, int($min:expr, $max:expr), $unit:literal, $help:literal) => {
        ParamDesc { key: stringify!($key), label: "", group: $group, unit: $unit, help: $help,
            kind: ParamKind::Int { min: $min, max: $max },
            get: |g| ParamValue::Int(g.$key as i64),
            set: |g, v| if let ParamValue::Int(v) = v { g.$key = v as u32 } }
    };
    (@one $group:literal, $key:ident, choice($opts:expr, $t:ty), $unit:literal, $help:literal) => {
        ParamDesc { key: stringify!($key), label: "", group: $group, unit: $unit, help: $help,
            kind: ParamKind::Choice { options: $opts },
            get: |g| ParamValue::Choice(g.$key.index()),
            set: |g, v| if let ParamValue::Choice(i) = v { g.$key = <$t>::ALL[i] } }
    };
}

const GENOME_PARAMS: &[ParamDesc<Genome>] = genome_params! {
    "Architecture": form: choice(FORMS, GrowthForm) "" "Growth form archetype";
    "Architecture": height_m: logf(0.05, 120.0) "m" "Target mature height";
    "Architecture": stems: int(1, 12) "" "Stems from the base";
    "Architecture": crown_base: float(0.0, 0.9) "" "Fraction of the height below the crown";
    "Architecture": crown_aspect: float(0.1, 4.0) "" "Crown width / crown height";
    "Architecture": crown_shape: choice(CROWNS, CrownShape) "" "Envelope of free space";
    "Architecture": apical_dominance: float(0.3, 0.85) "" "Resource kept by the main axis";
    "Architecture": axis_mode: choice(AXES, AxisMode) "" "Apex keeps leading or hands over";
    "Architecture": vigor: float(0.5, 6.0) "" "Resource per unit of free space";
    "Architecture": cycles: int(1, 40) "" "Growth cycles";
    "Branching": buds_per_node: int(1, 3) "" "Lateral buds per node";
    "Branching": branch_angle_deg: float(5.0, 120.0) "deg" "Branch angle at order 1";
    "Branching": branch_angle_per_order_deg: float(-30.0, 30.0) "deg" "Branch angle change per order";
    "Branching": phyllotaxis_deg: float(0.0, 360.0) "deg" "Divergence between nodes";
    "Branching": internode_m: logf(0.005, 3.0) "m" "Internode length";
    "Branching": internode_per_order: float(0.3, 1.5) "" "Internode factor per order";
    "Branching": max_order: int(0, 6) "" "Deepest branch order";
    "Tropism": space_seeking: float(0.0, 3.0) "" "Pull toward free space";
    "Tropism": gravitropism: float(-1.5, 1.5) "" "Pull up (negative droops)";
    "Tropism": gravitropism_per_order: float(-1.5, 1.5) "" "Gravitropism change per order";
    "Tropism": straightness: float(0.0, 3.0) "" "Keep the current direction";
    "Tropism": wind_bias: float(-1.0, 1.0) "" "Lean along the prevailing wind";
    "Tropism": wobble: float(0.0, 1.0) "" "Random bend per internode";
    "Biomechanics": pipe_exponent: float(1.5, 3.5) "" "Pipe model exponent";
    "Biomechanics": tip_radius_m: logf(0.0005, 0.5) "m" "Terminal radius";
    "Biomechanics": flare: float(0.0, 1.5) "" "Extra radius at the root";
    "Biomechanics": stiffness: float(0.0, 1.0) "" "Wind stiffness";
    "Biomechanics": shed_threshold: float(0.0, 1.0) "" "Shed starved branches below";
    "Segments": cross_section: choice(SECTIONS, CrossSection) "" "Segment cross-section";
    "Segments": surface: choice(SURFACES, SurfaceStyle) "" "Segment surface style";
    "Organs": organ: choice(ORGANS, OrganKind) "" "Organ archetype (kit shape)";
    "Organs": organ_size_m: logf(0.005, 4.0) "m" "Organ length";
    "Organs": organ_cluster: int(1, 12) "" "Organs per bearing node";
    "Organs": organ_orientation: choice(ORIENTS, OrganOrientation) "" "Organ orientation";
    "Organs": organ_reach: int(1, 8) "" "Organs within this many internodes of a tip";
    "Reproduction": fruit: choice(FRUITS, FruitKind) "" "Fruit / spore-body archetype";
    "Reproduction": fruit_size_m: logf(0.005, 2.0) "m" "Fruit size";
    "Reproduction": fruit_chance: float(0.0, 1.0) "" "Chance per tip";
};

impl Params for Genome {
    fn descriptors() -> &'static [ParamDesc<Self>] {
        GENOME_PARAMS
    }
}

impl Default for Look {
    /// Neutral greens, used until a planet palette is applied.
    fn default() -> Self {
        Look { bark: [0.1, 0.07, 0.05], organ: [0.035, 0.1, 0.03], organ_tip: [0.12, 0.19, 0.035], accent: [0.5, 0.05, 0.3] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_cover_unique_keys() {
        let d = Genome::descriptors();
        for (i, a) in d.iter().enumerate() {
            for b in &d[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
        assert!(d.len() >= 30);
        let _ = (
            FORMS, CROWNS, AXES, SECTIONS, SURFACES, ORGANS, ORIENTS, FRUITS,
        );
        assert_eq!(FORMS.len(), GrowthForm::ALL.len());
        assert_eq!(ORGANS.len(), OrganKind::ALL.len());
        assert_eq!(FRUITS.len(), FruitKind::ALL.len());
    }
}
