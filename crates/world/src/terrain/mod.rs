//! Immutable terrain intent, independent of frames and disposable render patches.

mod query;
pub use query::*;
mod generator;
pub use generator::*;

/// Explicit authoring salt, not a runtime body handle or display name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainIdentity(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainSeed(pub u64);

/// Algorithm compatibility, independent of application/cache versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerrainGeneratorVersion {
    V1,
}
impl TerrainGeneratorVersion {
    pub fn from_code(code: u32) -> Result<Self, TerrainError> {
        match code {
            1 => Ok(Self::V1),
            _ => Err(TerrainError::UnsupportedVersion(code)),
        }
    }
    pub fn code(self) -> u32 {
        match self {
            Self::V1 => 1,
        }
    }
}

/// Per-body publication coherence. Never a generation salt.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainRevision(u64);
impl TerrainRevision {
    pub fn value(self) -> u64 {
        self.0
    }
    pub(crate) fn next(self) -> Result<Self, TerrainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(TerrainError::RevisionOverflow)
    }
}

/// Fixed additive output order; finer terms cannot modify earlier bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainBand {
    Macro,
    Range,
    Regional,
    Local,
    Fine,
}
impl TerrainBand {
    pub const ALL: [Self; 5] = [
        Self::Macro,
        Self::Range,
        Self::Regional,
        Self::Local,
        Self::Fine,
    ];
}

/// Characteristic noise scale, not a single Fourier wavelength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TerrainScale {
    Metres { longest_wavelength_m: f64 },
    Angular { lowest_cycles_per_body: f64 },
}

/// Finite octaves with fixed frequency doubling and amplitude halving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainBandConfig {
    amplitude_m: f64,
    scale: TerrainScale,
    octaves: u8,
}
impl TerrainBandConfig {
    pub fn new(amplitude_m: f64, scale: TerrainScale, octaves: u8) -> Result<Self, TerrainError> {
        if !amplitude_m.is_finite() || amplitude_m < 0.0 || !(1..=4).contains(&octaves) {
            return Err(TerrainError::InvalidConfig);
        }
        let factor = (1u32 << (octaves - 1)) as f64;
        let valid = match scale {
            TerrainScale::Metres {
                longest_wavelength_m: x,
            } => x.is_finite() && x / factor >= 8.0,
            TerrainScale::Angular {
                lowest_cycles_per_body: x,
            } => x.is_finite() && x > 0.0 && (x * factor).is_finite(),
        };
        if !valid {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            amplitude_m: canonical_zero(amplitude_m),
            scale,
            octaves,
        })
    }
    pub fn amplitude_m(self) -> f64 {
        self.amplitude_m
    }
    pub fn scale(self) -> TerrainScale {
        self.scale
    }
    pub fn octaves(self) -> u8 {
        self.octaves
    }
    pub fn absolute_height_bound_m(self) -> f64 {
        let mut sum = 0.0;
        let mut amplitude = self.amplitude_m;
        for _ in 0..self.octaves {
            if amplitude != 0.0 {
                sum = (sum + amplitude).next_up();
                amplitude = (amplitude * 0.5).next_up();
            }
        }
        sum
    }
}

/// Small immutable V1 controls; output remaps must preserve band envelopes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainControls {
    continent_bias: f64,
    continent_contrast: f64,
    mountain_coverage: f64,
    mountain_strength: f64,
    warp_fraction: f64,
    ridge_softness: f64,
}
impl TerrainControls {
    pub fn new(
        continent_bias: f64,
        continent_contrast: f64,
        mountain_coverage: f64,
        mountain_strength: f64,
        warp_fraction: f64,
        ridge_softness: f64,
    ) -> Result<Self, TerrainError> {
        if ![
            continent_bias,
            continent_contrast,
            mountain_coverage,
            mountain_strength,
            warp_fraction,
            ridge_softness,
        ]
        .iter()
        .all(|x| x.is_finite())
            || !(-1.0..=1.0).contains(&continent_bias)
            || continent_contrast <= 0.0
            || !(0.0..=1.0).contains(&mountain_coverage)
            || !(0.0..=1.0).contains(&mountain_strength)
            || !(0.0..=1.0).contains(&warp_fraction)
            || ridge_softness <= 0.0
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            continent_bias: canonical_zero(continent_bias),
            continent_contrast,
            mountain_coverage: canonical_zero(mountain_coverage),
            mountain_strength: canonical_zero(mountain_strength),
            warp_fraction: canonical_zero(warp_fraction),
            ridge_softness,
        })
    }
    pub fn continent_bias(self) -> f64 {
        self.continent_bias
    }
    pub fn continent_contrast(self) -> f64 {
        self.continent_contrast
    }
    pub fn mountain_coverage(self) -> f64 {
        self.mountain_coverage
    }
    pub fn mountain_strength(self) -> f64 {
        self.mountain_strength
    }
    pub fn warp_fraction(self) -> f64 {
        self.warp_fraction
    }
    pub fn ridge_softness(self) -> f64 {
        self.ridge_softness
    }
}
fn canonical_zero(x: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainConfig {
    bands: [TerrainBandConfig; 5],
    controls: TerrainControls,
    absolute_height_bound_m: f64,
}
impl TerrainConfig {
    pub fn new(
        bands: [TerrainBandConfig; 5],
        controls: TerrainControls,
    ) -> Result<Self, TerrainError> {
        if bands
            .iter()
            .skip(1)
            .any(|b| matches!(b.scale, TerrainScale::Angular { .. }))
        {
            return Err(TerrainError::InvalidConfig);
        }
        let sum = bands.iter().fold(0.0, |sum, b| {
            let bound = b.absolute_height_bound_m();
            if bound == 0.0 {
                sum
            } else {
                (sum + bound).next_up()
            }
        });
        // Outward allowance for finite sums/transforms. Flat remains exact.
        let bound = if sum == 0.0 {
            0.0
        } else {
            (sum + sum * (128.0 * f64::EPSILON)).next_up()
        };
        if !bound.is_finite() {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            bands,
            controls,
            absolute_height_bound_m: bound,
        })
    }
    pub fn band(&self, band: TerrainBand) -> TerrainBandConfig {
        self.bands[band as usize]
    }
    pub fn controls(&self) -> TerrainControls {
        self.controls
    }
    pub fn absolute_height_bound_m(&self) -> f64 {
        self.absolute_height_bound_m
    }
}

/// Cache equality compares the complete definition and reference-radius bits,
/// never an unchecked hash or runtime revision alone.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainDefinition {
    identity: TerrainIdentity,
    seed: TerrainSeed,
    version: TerrainGeneratorVersion,
    config: TerrainConfig,
}
impl TerrainDefinition {
    pub fn new(
        identity: TerrainIdentity,
        seed: TerrainSeed,
        version: TerrainGeneratorVersion,
        config: TerrainConfig,
    ) -> Self {
        Self {
            identity,
            seed,
            version,
            config,
        }
    }
    pub fn identity(&self) -> TerrainIdentity {
        self.identity
    }
    pub fn seed(&self) -> TerrainSeed {
        self.seed
    }
    pub fn version(&self) -> TerrainGeneratorVersion {
        self.version
    }
    pub fn config(&self) -> &TerrainConfig {
        &self.config
    }
    /// Combined radial-graph validation before an authoritative edit commits.
    pub fn validate_radius(&self, radius_m: f64) -> Result<(), TerrainError> {
        self.validate_radius_envelope(radius_m)?;
        TerrainGenerator::validate_definition(self, radius_m)
    }
    pub(super) fn validate_radius_envelope(&self, radius_m: f64) -> Result<(), TerrainError> {
        let envelope = self.config.absolute_height_bound_m();
        if !radius_m.is_finite()
            || radius_m <= 0.0
            || radius_m > 1e8
            || envelope > 0.1 * radius_m
            || radius_m - envelope <= (64.0 * f64::EPSILON * radius_m).max(1e-3)
        {
            return Err(TerrainError::InvalidRadius);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TerrainError {
    #[error("unsupported terrain generator version: {0}")]
    UnsupportedVersion(u32),
    #[error("invalid terrain band or control configuration")]
    InvalidConfig,
    #[error("reference radius and terrain envelope are incompatible")]
    InvalidRadius,
    #[error("terrain footprint must be finite and nonnegative")]
    InvalidFootprint,
    #[error("terrain revision overflow")]
    RevisionOverflow,
    #[error("terrain batch input/output lengths differ")]
    LengthMismatch,
    #[error("nonfinite terrain query result")]
    NonFiniteResult,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revision_overflow_is_an_error_not_a_generation_identity_change() {
        assert_eq!(
            TerrainRevision(u64::MAX).next(),
            Err(TerrainError::RevisionOverflow)
        );
    }
}
