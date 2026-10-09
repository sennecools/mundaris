//! Immutable, radial body-shape definitions used by the redesigned surface path.

use glam::{DQuat, DVec3, EulerRot};
use astrum_math::Direction3;

use super::TerrainError;

const MAX_REFERENCE_RADIUS_M: f64 = 1.0e8;
const MIN_AXIS_FRACTION: f64 = 1.0e-3;
const MAX_AXIS_FRACTION: f64 = 1.0e3;
const MAX_IRREGULAR_AMPLITUDE: f64 = 0.35;
const ENVELOPE_RELATIVE_ALLOWANCE: f64 = 256.0 * f64::EPSILON;

/// A deterministic positive radial surface, independent of terrain relief.
///
/// The shape is star-shaped about the body origin: it has exactly one radius for
/// every direction. This deliberately excludes undercuts and enclosed cavities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeDefinition {
    kind: ShapeKind,
    configuration_identity: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ShapeKind {
    Sphere,
    Ellipsoid {
        axes_fractions: [f64; 3],
    },
    Irregular {
        axes_fractions: [f64; 3],
        asymmetry: f64,
        lobes: f64,
        rotation: DQuat,
        seed: u64,
    },
}

impl ShapeDefinition {
    /// Constructs the unit-sphere shape, where the reference radius is the
    /// physical radius in every direction.
    /// Whether this is the reference sphere (zero shape height everywhere).
    pub fn is_sphere(&self) -> bool {
        matches!(self.kind, ShapeKind::Sphere)
    }

    pub fn sphere() -> Self {
        Self {
            kind: ShapeKind::Sphere,
            configuration_identity: hash_words([0x5348_4150_4500_0001]),
        }
    }

    /// Constructs an axis-aligned ellipsoid. Each value scales the reference
    /// radius along the corresponding body axis.
    pub fn ellipsoid(axes_fractions: [f64; 3]) -> Result<Self, TerrainError> {
        validate_axes(axes_fractions)?;
        let axes_fractions = axes_fractions.map(canonical_zero);
        Ok(Self {
            kind: ShapeKind::Ellipsoid { axes_fractions },
            configuration_identity: hash_floats(0x5348_4150_4500_0002, axes_fractions),
        })
    }

    /// Constructs a seeded, smoothly irregular ellipsoid.
    ///
    /// The two amplitudes are independently bounded in `[0, 0.35]`; their sum
    /// remains below one, so every direction keeps a strictly positive radius.
    /// The dipole term introduces broad asymmetry and the quartic term creates
    /// six broad lobes. Seeded orientation is stable for a given definition.
    pub fn irregular(
        axes_fractions: [f64; 3],
        asymmetry: f64,
        lobes: f64,
        seed: u64,
    ) -> Result<Self, TerrainError> {
        validate_axes(axes_fractions)?;
        if !asymmetry.is_finite()
            || !lobes.is_finite()
            || !(0.0..=MAX_IRREGULAR_AMPLITUDE).contains(&asymmetry)
            || !(0.0..=MAX_IRREGULAR_AMPLITUDE).contains(&lobes)
            || asymmetry + lobes >= 1.0
        {
            return Err(TerrainError::InvalidConfig);
        }

        let axes_fractions = axes_fractions.map(canonical_zero);
        let asymmetry = canonical_zero(asymmetry);
        let lobes = canonical_zero(lobes);
        let rotation = seeded_rotation(seed);
        Ok(Self {
            kind: ShapeKind::Irregular {
                axes_fractions,
                asymmetry,
                lobes,
                rotation,
                seed,
            },
            configuration_identity: hash_words([
                0x5348_4150_4500_0003,
                seed,
                axes_fractions[0].to_bits(),
                axes_fractions[1].to_bits(),
                axes_fractions[2].to_bits(),
                asymmetry.to_bits(),
                lobes.to_bits(),
            ]),
        })
    }

    /// Stable identity for this shape algorithm version and its exact
    /// canonical configuration. It does not use formatted floating-point text.
    pub fn configuration_identity(self) -> u64 {
        self.configuration_identity
    }

    /// Versioned algorithm name for diagnostics and persisted sample records.
    pub fn algorithm_name(self) -> &'static str {
        match self.kind {
            ShapeKind::Sphere => "SphereV1",
            ShapeKind::Ellipsoid { .. } => "EllipsoidV1",
            ShapeKind::Irregular { .. } => "IrregularV1",
        }
    }

    /// Semiaxis fractions relative to the supplied reference radius.
    pub fn axes_fractions(self) -> [f64; 3] {
        match self.kind {
            ShapeKind::Sphere => [1.0; 3],
            ShapeKind::Ellipsoid { axes_fractions }
            | ShapeKind::Irregular { axes_fractions, .. } => axes_fractions,
        }
    }

    /// Irregular harmonic amplitudes, `[asymmetry, lobes]`, or zero for
    /// analytic sphere and ellipsoid definitions.
    pub fn irregular_amplitudes(self) -> [f64; 2] {
        match self.kind {
            ShapeKind::Irregular {
                asymmetry, lobes, ..
            } => [asymmetry, lobes],
            _ => [0.0; 2],
        }
    }

    /// Seed used to orient the irregular harmonics, if this shape is seeded.
    pub fn seed(self) -> Option<u64> {
        match self.kind {
            ShapeKind::Irregular { seed, .. } => Some(seed),
            _ => None,
        }
    }

    /// Evaluates the radial distance, tangent derivative and outward normal.
    pub fn query(
        self,
        direction: Direction3,
        reference_radius_m: f64,
    ) -> Result<ShapeSample, TerrainError> {
        validate_reference_radius(reference_radius_m)?;
        let unit = direction.unit();
        if matches!(self.kind, ShapeKind::Sphere) {
            return Ok(ShapeSample {
                direction: unit,
                radius_m: reference_radius_m,
                gradient_m: DVec3::ZERO,
                normal: unit,
            });
        }
        let (axes, modulation, modulation_gradient) = match self.kind {
            ShapeKind::Sphere => ([1.0; 3], 1.0, DVec3::ZERO),
            ShapeKind::Ellipsoid { axes_fractions } => (axes_fractions, 1.0, DVec3::ZERO),
            ShapeKind::Irregular {
                axes_fractions,
                asymmetry,
                lobes,
                rotation,
                ..
            } => {
                let rotated = rotation * unit;
                let dipole = rotated.x;
                let quartic_sum = rotated.x.powi(4) + rotated.y.powi(4) + rotated.z.powi(4);
                let lobe = 3.0 * quartic_sum - 2.0;
                let rotated_gradient = DVec3::new(
                    asymmetry + 12.0 * lobes * rotated.x.powi(3),
                    12.0 * lobes * rotated.y.powi(3),
                    12.0 * lobes * rotated.z.powi(3),
                );
                let modulation = 1.0 + asymmetry * dipole + lobes * lobe;
                let modulation_gradient = rotation.conjugate() * rotated_gradient;
                (
                    axes_fractions,
                    modulation,
                    tangent_projection(modulation_gradient, unit),
                )
            }
        };

        let inverse_axes_squared = DVec3::new(
            1.0 / (axes[0] * axes[0]),
            1.0 / (axes[1] * axes[1]),
            1.0 / (axes[2] * axes[2]),
        );
        let inverse_radius_squared = unit.dot(unit * inverse_axes_squared);
        let ellipsoid_radius = inverse_radius_squared.sqrt().recip();
        let ellipsoid_gradient = tangent_projection(
            -(unit * inverse_axes_squared) / inverse_radius_squared.powf(1.5),
            unit,
        );

        let radius_m = reference_radius_m * ellipsoid_radius * modulation;
        let gradient_m = reference_radius_m
            * (ellipsoid_gradient * modulation + ellipsoid_radius * modulation_gradient);
        if !radius_m.is_finite() || radius_m <= 0.0 || !gradient_m.is_finite() {
            return Err(TerrainError::NonFiniteResult);
        }

        let normal = (unit - gradient_m / radius_m).normalize();
        if !normal.is_finite() {
            return Err(TerrainError::NonFiniteResult);
        }
        Ok(ShapeSample {
            direction: unit,
            radius_m,
            gradient_m,
            normal,
        })
    }

    /// Conservative minimum and maximum radii over all directions.
    ///
    /// For an ellipsoid, radial extrema are its shortest and longest semiaxes.
    /// Irregular modulation lies in `[1-asymmetry-lobes, 1+asymmetry+lobes]`,
    /// since both harmonic terms are bounded by one. Multiplying these global
    /// bounds is conservative even if the extrema cannot occur in the same
    /// direction. This is a radial envelope only; it does not bound terrain
    /// relief, overhangs, or any non-star-shaped geometry.
    pub fn conservative_radius_envelope_m(
        self,
        reference_radius_m: f64,
    ) -> Result<[f64; 2], TerrainError> {
        validate_reference_radius(reference_radius_m)?;
        let (axes, modulation_bound) = match self.kind {
            ShapeKind::Sphere => ([1.0; 3], 0.0),
            ShapeKind::Ellipsoid { axes_fractions } => (axes_fractions, 0.0),
            ShapeKind::Irregular {
                axes_fractions,
                asymmetry,
                lobes,
                ..
            } => (axes_fractions, asymmetry + lobes),
        };
        let shortest = axes.into_iter().fold(f64::INFINITY, f64::min);
        let longest = axes.into_iter().fold(0.0, f64::max);
        let envelope = [
            reference_radius_m * shortest * (1.0 - modulation_bound),
            reference_radius_m * longest * (1.0 + modulation_bound),
        ];
        let conservatively_rounded = [
            (envelope[0] * (1.0 - ENVELOPE_RELATIVE_ALLOWANCE)).next_down(),
            (envelope[1] * (1.0 + ENVELOPE_RELATIVE_ALLOWANCE)).next_up(),
        ];
        if conservatively_rounded.iter().all(|value| value.is_finite())
            && conservatively_rounded[0] > 0.0
        {
            Ok(conservatively_rounded)
        } else {
            Err(TerrainError::NonFiniteResult)
        }
    }
}

/// A complete body-local shape result. The gradient is the derivative of
/// radius in metres per unit change tangent to the unit direction sphere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeSample {
    direction: DVec3,
    radius_m: f64,
    gradient_m: DVec3,
    normal: DVec3,
}

impl ShapeSample {
    pub fn radius_m(self) -> f64 {
        self.radius_m
    }

    /// Body-tangent derivative of radius, in metres per unit direction.
    pub fn gradient_m(self) -> DVec3 {
        self.gradient_m
    }

    pub fn normal(self) -> DVec3 {
        self.normal
    }

    pub fn position(self, direction: Direction3) -> DVec3 {
        direction.unit() * self.radius_m
    }

    pub fn direction(self) -> DVec3 {
        self.direction
    }
}

fn validate_axes(axes: [f64; 3]) -> Result<(), TerrainError> {
    if axes
        .iter()
        .all(|axis| axis.is_finite() && (MIN_AXIS_FRACTION..=MAX_AXIS_FRACTION).contains(axis))
    {
        Ok(())
    } else {
        Err(TerrainError::InvalidConfig)
    }
}

fn validate_reference_radius(radius_m: f64) -> Result<(), TerrainError> {
    if radius_m.is_finite() && (0.0..=MAX_REFERENCE_RADIUS_M).contains(&radius_m) && radius_m > 0.0
    {
        Ok(())
    } else {
        Err(TerrainError::InvalidRadius)
    }
}

fn tangent_projection(value: DVec3, direction: DVec3) -> DVec3 {
    value - direction * value.dot(direction)
}

fn seeded_rotation(seed: u64) -> DQuat {
    let mut state = seed;
    let angles: [f64; 3] = std::array::from_fn(|_| {
        state = splitmix64(state);
        let unit = (state >> 11) as f64 * (1.0 / ((1u64 << 53) as f64));
        unit * std::f64::consts::TAU
    });
    DQuat::from_euler(EulerRot::XYZ, angles[0], angles[1], angles[2])
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn hash_floats(tag: u64, values: [f64; 3]) -> u64 {
    hash_words([
        tag,
        values[0].to_bits(),
        values[1].to_bits(),
        values[2].to_bits(),
    ])
}

fn hash_words(words: impl IntoIterator<Item = u64>) -> u64 {
    words.into_iter().fold(0x9e37_79b9_7f4a_7c15, |hash, word| {
        splitmix64(hash ^ splitmix64(word))
    })
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
