//! Native geometry adapters retain the selected world authority unchanged.
use super::*;

/// Resolution guide only; this is separate from certified reconstruction error.
const REPRESENTATION_SAMPLE_SPACING_PIXELS: f64 = 1.0;

/// Exact world definition used by a disposable native terrain representation.
#[derive(Debug, Clone, PartialEq)]
pub enum NativeTerrainDefinition {
    Legacy(TerrainDefinition),
    Surface(SurfaceDefinition),
}

impl From<TerrainDefinition> for NativeTerrainDefinition {
    fn from(definition: TerrainDefinition) -> Self {
        Self::Legacy(definition)
    }
}

impl From<SurfaceDefinition> for NativeTerrainDefinition {
    fn from(definition: SurfaceDefinition) -> Self {
        Self::Surface(definition)
    }
}

impl NativeTerrainDefinition {
    pub fn identity(&self) -> TerrainIdentity {
        match self {
            Self::Legacy(definition) => definition.identity(),
            Self::Surface(definition) => definition.identity(),
        }
    }

    pub fn seed(&self) -> TerrainSeed {
        match self {
            Self::Legacy(definition) => definition.seed(),
            Self::Surface(definition) => definition.seed(),
        }
    }
    pub fn legacy(&self) -> Option<&TerrainDefinition> {
        match self {
            Self::Legacy(definition) => Some(definition),
            Self::Surface(_) => None,
        }
    }

    /// Combined shape and geological radial displacement from the reference sphere.
    pub fn absolute_height_bound_m(&self, radius_m: f64) -> Result<f64> {
        match self {
            Self::Legacy(definition) => Ok(definition.config().absolute_height_bound_m()),
            Self::Surface(definition) => {
                let generator = SurfaceGenerator::new(definition, radius_m)?;
                Ok(surface_height_bound(&generator))
            }
        }
    }

    pub(super) fn working_heap_bound_bytes(&self) -> usize {
        match self {
            Self::Legacy(definition) => {
                size_of::<TerrainGenerator>()
                    + TerrainGenerator::working_heap_bound_bytes(definition.config())
            }
            Self::Surface(_) => {
                size_of::<SurfaceGenerator>() + SurfaceGenerator::working_heap_bound_bytes()
            }
        }
    }
}

pub(super) enum NativeTerrainGenerator {
    Legacy(Box<TerrainGenerator>),
    Surface(Box<SurfaceGenerator>),
}

fn surface_height_bound(generator: &SurfaceGenerator) -> f64 {
    let envelope = generator.conservative_radius_envelope_m();
    (envelope[0] - generator.radius_m())
        .abs()
        .max((envelope[1] - generator.radius_m()).abs())
        .next_up()
}

impl NativeTerrainGenerator {
    pub(super) fn new(definition: &NativeTerrainDefinition, radius_m: f64) -> Result<Self> {
        match definition {
            NativeTerrainDefinition::Legacy(definition) => Ok(Self::Legacy(Box::new(
                TerrainGenerator::new(definition, radius_m)?,
            ))),
            NativeTerrainDefinition::Surface(definition) => {
                let generator = SurfaceGenerator::new(definition, radius_m)?;
                anyhow::ensure!(
                    surface_height_bound(&generator) < 0.1 * radius_m,
                    "native surface radial envelope exceeds the renderer's ten-percent limit"
                );
                Ok(Self::Surface(Box::new(generator)))
            }
        }
    }

    pub(super) fn radius_m(&self) -> f64 {
        match self {
            Self::Legacy(generator) => generator.radius_m(),
            Self::Surface(generator) => generator.radius_m(),
        }
    }

    pub(super) fn resident_heap_bytes(&self) -> usize {
        match self {
            Self::Legacy(generator) => {
                size_of::<TerrainGenerator>() + generator.resident_heap_bytes()
            }
            Self::Surface(generator) => {
                size_of::<SurfaceGenerator>() + generator.resident_heap_bytes()
            }
        }
    }

    pub(super) fn query_workspace_bytes(&self) -> usize {
        match self {
            Self::Legacy(generator) => generator.query_workspace_bytes(),
            Self::Surface(generator) => generator.query_workspace_bytes(),
        }
    }

    pub(super) fn evaluate_geometry_batch(
        &self,
        locations: &[SurfaceLocation],
        footprint: TerrainFootprint,
        output: &mut [SurfaceGeometrySample],
    ) -> Result<()> {
        anyhow::ensure!(
            locations.len() == output.len(),
            "surface batch length mismatch"
        );
        match self {
            Self::Legacy(generator) => {
                let mut samples = [TerrainSample::default(); MAX_GENERATION_BATCH];
                anyhow::ensure!(
                    locations.len() <= samples.len(),
                    "surface batch is too large"
                );
                generator.evaluate_batch(locations, footprint, &mut samples[..locations.len()])?;
                for ((location, sample), destination) in locations.iter().zip(samples).zip(output) {
                    *destination = SurfaceGeometrySample {
                        position_body_m: location.direction().unit()
                            * (generator.radius_m() + sample.height_m()),
                        normal_body: sample.normal_body(*location, generator.radius_m())?.unit(),
                    };
                }
            }
            Self::Surface(generator) => {
                // Positions retain the complete authority. Shading represents
                // its radial slope at the mesh footprint instead of aliasing
                // metre-scale analytic gradients into distant vertices.
                for (location, destination) in locations.iter().zip(output) {
                    let sample = generator.evaluate_point(*location)?;
                    *destination = SurfaceGeometrySample {
                        position_body_m: sample.position(*location),
                        normal_body: footprint_normal(generator, *location, footprint, sample)?,
                    };
                }
            }
        }
        Ok(())
    }

    /// A resolution guide for an uncertified compositional representation.
    /// It never replaces the complete error bound or changes world queries.
    pub(super) fn representation_clearance_m(&self, observer: glam::DVec3) -> Result<Option<f64>> {
        match self {
            Self::Legacy(_) => Ok(None),
            Self::Surface(generator) => {
                let Ok(direction) = Direction3::try_new(observer) else {
                    return Ok(Some(0.0));
                };
                let location = SurfaceLocation::new(direction);
                Ok(Some(
                    (observer.length() - generator.evaluate_point(location)?.radius_m()).max(0.0),
                ))
            }
        }
    }

    pub(super) fn representation_demand_pixels(
        &self,
        input: SurfaceRefinementInput,
        clearance_m: Option<f64>,
        split_pixels: f64,
    ) -> f64 {
        let Some(clearance) = clearance_m else {
            return input.certified_error_pixels;
        };
        // Cube-chart normalization has operator norm <= 1. The physical grid
        // spacing guide is therefore independent of face orientation. Use the
        // complete camera clearance to keep a loose global extent from driving
        // this heuristic to the near plane. Certified projection still uses
        // that conservative extent and is reported separately.
        let spacing = self.radius_m() * 2.0 / (16.0 * (1u64 << input.address.level()) as f64);
        // Use the nearest point of the patch ball rather than its centre:
        // perspective magnification can be greater near a patch's screen edge.
        let depth = (-input.center_view.z - input.ball_radius_m)
            .max(clearance)
            .max(input.projection.near_m());
        // Map the explicit spacing target to the selector's hysteresis scale.
        // The configured certified-error tolerance remains unchanged.
        spacing * input.projection.focal_pixels() / depth / REPRESENTATION_SAMPLE_SPACING_PIXELS
            * split_pixels
    }

    pub(super) fn has_local_profile_bounds(&self) -> bool {
        matches!(self, Self::Legacy(generator) if !generator.crater_features().is_empty())
    }

    pub(super) fn profile_difference_bound_m(
        &self,
        first: TerrainFootprint,
        second: TerrainFootprint,
    ) -> Result<f64> {
        match self {
            Self::Legacy(generator) => Ok(generator.profile_difference_bound_m(first, second)?),
            Self::Surface(_) => Ok(0.0),
        }
    }

    pub(super) fn profile_difference_bound_for_region_m(
        &self,
        cap: DirectionalCap,
        first: TerrainFootprint,
        second: TerrainFootprint,
    ) -> Result<f64> {
        match self {
            Self::Legacy(generator) => {
                Ok(generator.profile_difference_bound_for_region_m(cap, first, second)?)
            }
            Self::Surface(_) => Ok(0.0),
        }
    }

    pub(super) fn represented_height_bound_m(
        &self,
        cap: DirectionalCap,
        footprint: TerrainFootprint,
    ) -> Result<f64> {
        match self {
            Self::Legacy(generator) => Ok(generator
                .bounds_for_region(cap, footprint)?
                .represented_height_bound_m()),
            Self::Surface(generator) => Ok(surface_height_bound(generator)),
        }
    }

    pub(super) fn surface_certificate(
        &self,
        address: CubePatchAddress,
        metadata: PatchMetadata,
    ) -> Result<(SurfaceExtent, SurfaceErrorContributions)> {
        match self {
            Self::Legacy(generator) => terrain_surface_certificate(generator, address, metadata),
            Self::Surface(generator) => {
                let radius = generator.radius_m();
                let envelope = generator.conservative_radius_envelope_m();
                let bound = surface_height_bound(generator);
                let diameter = (5.0_f64.sqrt().next_up() * 2.0
                    / (16.0 * (1u64 << address.level()) as f64))
                    .next_up();
                // ||h(n)n|| <= H. The difference from a convex interpolation
                // of vertex residuals is <= 2H, independently of unsampled
                // features. This is an amplitude certificate, not a claim of
                // decreasing reconstruction error or sampled-extrema proof.
                Ok((
                    SurfaceExtent {
                        min_height_m: (envelope[0] - radius).next_down(),
                        max_height_m: (envelope[1] - radius).next_up(),
                        guaranteed_opaque_radius_m: 0.0,
                    },
                    SurfaceErrorContributions {
                        sphere_m: (radius * metadata.error_unit()).next_up().max(
                            SurfaceErrorContributions::interpolation_bound_m(
                                (3.0 * radius).next_up(),
                                diameter,
                            )?,
                        ),
                        filtered_interpolation_m: (2.0 * bound).next_up(),
                        unresolved_m: 0.0,
                        boundary_constraint_m: 0.0,
                        morph_remaining_m: 0.0,
                        numeric_m: (512.0 * f64::EPSILON * envelope[1]).next_up(),
                    },
                ))
            }
        }
    }
}

fn footprint_normal(
    generator: &SurfaceGenerator,
    location: SurfaceLocation,
    footprint: TerrainFootprint,
    sample: SurfaceSample,
) -> Result<glam::DVec3> {
    let width = footprint.metres();
    // Below half a metre the complete analytic derivative is resolved. Blend
    // continuously into finite secants to avoid cancellation at deep LODs.
    let blend = (width / 0.5).clamp(0.0, 1.0);
    if blend == 0.0 {
        return Ok(sample.normal());
    }
    let n = location.direction().unit();
    let step = (width.max(0.5) / generator.radius_m()).min(0.25);
    // Differentiate the radially extended height in all three body axes. This
    // has no tangent-basis switch or pole seam, and projects back to the sphere.
    let secant = |axis: glam::DVec3| -> Result<f64> {
        let query = |sign: f64| -> Result<f64> {
            let direction = Direction3::try_new(n + axis * (sign * step))?;
            Ok(generator
                .evaluate_point(SurfaceLocation::new(direction))?
                .radius_m())
        };
        Ok((query(1.0)? - query(-1.0)?) / (2.0 * step))
    };
    let gradient = glam::DVec3::new(
        secant(glam::DVec3::X)?,
        secant(glam::DVec3::Y)?,
        secant(glam::DVec3::Z)?,
    );
    let gradient = gradient - n * n.dot(gradient);
    let filtered = (n - gradient / sample.radius_m()).normalize();
    Ok((sample.normal() * (1.0 - blend) + filtered * blend).normalize())
}
