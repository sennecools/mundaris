//! Reference-renderer adapter for authoritative world surface queries.
//!
//! This translates world-owned samples into the renderer's domain-free vertex
//! attributes. It does not evaluate shape, terrain, or material fields itself.

use glam::{DQuat, DVec3};
use astrum_math::{Direction3, surface::SurfaceLocation};
use astrum_world::terrain::{
    GeologicalControls, MoonTerrainGenerator, SurfaceDetailDiagnostics, SurfaceGenerator,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReferenceSample {
    pub(crate) shape_radius_m: f64,
    pub(crate) shape_gradient_m_per_unit_direction: DVec3,
    pub(crate) terrain_height_m: f64,
    pub(crate) terrain_gradient_m_per_unit_direction: DVec3,
    pub(crate) radius_m: f64,
    pub(crate) normal_body: DVec3,
    pub(crate) material_weights: [f64; 4],
    pub(crate) work: [u32; 2],
    pub(crate) accepted_features: Option<u32>,
}

pub(crate) trait SurfaceQuery {
    fn radius_m(&self) -> f64;
    fn radial_envelope_m(&self) -> [f64; 2];
    fn absolute_height_bound_m(&self) -> f64;
    fn evaluate_point(&self, location: SurfaceLocation) -> anyhow::Result<ReferenceSample>;

    /// Direction-local geological description when the source surface exposes
    /// one. Legacy terrain families intentionally return `None`.
    fn geological_controls(
        &self,
        _location: SurfaceLocation,
    ) -> anyhow::Result<Option<GeologicalControls>> {
        Ok(None)
    }

    fn view_target_radius_m(
        &self,
        location: SurfaceLocation,
        include_terrain_relief: bool,
    ) -> anyhow::Result<f64> {
        let _ = include_terrain_relief;
        Ok(self.evaluate_point(location)?.radius_m)
    }

    #[allow(dead_code)] // Used by the sibling family example, not the historical CLI.
    fn detail_diagnostics(
        &self,
        _location: SurfaceLocation,
    ) -> anyhow::Result<Option<SurfaceDetailDiagnostics>> {
        Ok(None)
    }

    #[allow(dead_code)]
    fn body_to_chart_direction(&self, body_direction: DVec3) -> DVec3 {
        body_direction
    }

    fn chart_to_body_direction(&self, chart_direction: DVec3) -> DVec3 {
        chart_direction
    }

    fn evaluate_batch(
        &self,
        locations: &[SurfaceLocation],
        output: &mut [ReferenceSample],
    ) -> anyhow::Result<()> {
        if locations.len() != output.len() {
            anyhow::bail!("surface query batch length mismatch");
        }
        for (location, sample) in locations.iter().copied().zip(output) {
            *sample = self.evaluate_point(location)?;
        }
        Ok(())
    }
}

impl SurfaceQuery for MoonTerrainGenerator {
    fn radius_m(&self) -> f64 {
        MoonTerrainGenerator::radius_m(self)
    }

    fn radial_envelope_m(&self) -> [f64; 2] {
        let bound = self.conservative_absolute_height_bound_m();
        [self.radius_m() - bound, self.radius_m() + bound]
    }

    fn absolute_height_bound_m(&self) -> f64 {
        self.conservative_absolute_height_bound_m()
    }

    fn view_target_radius_m(
        &self,
        location: SurfaceLocation,
        include_terrain_relief: bool,
    ) -> anyhow::Result<f64> {
        if include_terrain_relief {
            Ok(MoonTerrainGenerator::evaluate_point(self, location)?
                .terrain()
                .height_m()
                + self.radius_m())
        } else {
            Ok(self.radius_m())
        }
    }

    fn evaluate_point(&self, location: SurfaceLocation) -> anyhow::Result<ReferenceSample> {
        let sample = MoonTerrainGenerator::evaluate_point(self, location)?;
        let terrain = sample.terrain();
        let material = sample.material();
        Ok(ReferenceSample {
            shape_radius_m: self.radius_m(),
            shape_gradient_m_per_unit_direction: DVec3::ZERO,
            terrain_height_m: terrain.height_m(),
            terrain_gradient_m_per_unit_direction: terrain.tangent_gradient_m_per_unit_direction(),
            radius_m: self.radius_m() + terrain.height_m(),
            normal_body: terrain.normal_body(location, self.radius_m())?.unit(),
            material_weights: [
                material.regolith_weight(),
                material.rock_weight(),
                material.basalt_weight(),
                0.0,
            ],
            work: [0; 2],
            accepted_features: None,
        })
    }

    fn evaluate_batch(
        &self,
        locations: &[SurfaceLocation],
        output: &mut [ReferenceSample],
    ) -> anyhow::Result<()> {
        if locations.len() != output.len() {
            anyhow::bail!("surface query batch length mismatch");
        }
        if locations.is_empty() {
            return Ok(());
        }
        let first = MoonTerrainGenerator::evaluate_point(self, locations[0])?;
        let mut samples = vec![first; locations.len()];
        MoonTerrainGenerator::evaluate_batch(self, locations, &mut samples)?;
        for ((sample, location), target) in samples.into_iter().zip(locations).zip(output) {
            let terrain = sample.terrain();
            let material = sample.material();
            *target = ReferenceSample {
                shape_radius_m: self.radius_m(),
                shape_gradient_m_per_unit_direction: DVec3::ZERO,
                terrain_height_m: terrain.height_m(),
                terrain_gradient_m_per_unit_direction: terrain
                    .tangent_gradient_m_per_unit_direction(),
                radius_m: self.radius_m() + terrain.height_m(),
                normal_body: terrain.normal_body(*location, self.radius_m())?.unit(),
                material_weights: [
                    material.regolith_weight(),
                    material.rock_weight(),
                    material.basalt_weight(),
                    0.0,
                ],
                work: [0; 2],
                accepted_features: None,
            };
        }
        Ok(())
    }
}

impl SurfaceQuery for SurfaceGenerator {
    fn detail_diagnostics(
        &self,
        location: SurfaceLocation,
    ) -> anyhow::Result<Option<SurfaceDetailDiagnostics>> {
        Ok(SurfaceGenerator::detail_diagnostics(self, location)?)
    }
    fn radius_m(&self) -> f64 {
        SurfaceGenerator::radius_m(self)
    }

    fn radial_envelope_m(&self) -> [f64; 2] {
        SurfaceGenerator::conservative_radius_envelope_m(self)
    }

    fn absolute_height_bound_m(&self) -> f64 {
        SurfaceGenerator::conservative_absolute_height_bound_m(self)
    }

    fn evaluate_point(&self, location: SurfaceLocation) -> anyhow::Result<ReferenceSample> {
        let sample = SurfaceGenerator::evaluate_point(self, location)?;
        Ok(ReferenceSample {
            shape_radius_m: sample.shape().radius_m(),
            shape_gradient_m_per_unit_direction: sample.shape().gradient_m(),
            terrain_height_m: sample.terrain().height_m(),
            terrain_gradient_m_per_unit_direction: sample
                .terrain()
                .tangent_gradient_m_per_unit_direction(),
            radius_m: sample.radius_m(),
            normal_body: sample.normal(),
            material_weights: sample.material_weights(),
            work: [
                sample.work().cells_visited,
                sample.work().candidate_features,
            ],
            accepted_features: sample.work().accepted_features,
        })
    }

    fn geological_controls(
        &self,
        location: SurfaceLocation,
    ) -> anyhow::Result<Option<GeologicalControls>> {
        Ok(SurfaceGenerator::geological_controls(self, location)?)
    }

    fn evaluate_batch(
        &self,
        locations: &[SurfaceLocation],
        output: &mut [ReferenceSample],
    ) -> anyhow::Result<()> {
        if locations.len() != output.len() {
            anyhow::bail!("surface query batch length mismatch");
        }
        if locations.is_empty() {
            return Ok(());
        }
        let first = SurfaceGenerator::evaluate_point(self, locations[0])?;
        let mut samples = vec![first; locations.len()];
        SurfaceGenerator::evaluate_batch(self, locations, &mut samples)?;
        for (sample, target) in samples.into_iter().zip(output) {
            *target = ReferenceSample {
                shape_radius_m: sample.shape().radius_m(),
                shape_gradient_m_per_unit_direction: sample.shape().gradient_m(),
                terrain_height_m: sample.terrain().height_m(),
                terrain_gradient_m_per_unit_direction: sample
                    .terrain()
                    .tangent_gradient_m_per_unit_direction(),
                radius_m: sample.radius_m(),
                normal_body: sample.normal(),
                material_weights: sample.material_weights(),
                work: [
                    sample.work().cells_visited,
                    sample.work().candidate_features,
                ],
                accepted_features: sample.work().accepted_features,
            };
        }
        Ok(())
    }
}

/// Rotates reference-chart queries into a selected body-fixed landmark region.
/// The world oracle remains authoritative; all result vectors are rotated back
/// into chart coordinates for temporary mesh construction and lighting.
#[derive(Clone, Copy)]
#[allow(dead_code)] // Constructed by the sibling family example, not the legacy CLI.
pub(crate) struct RotatedSurfaceQuery<'a> {
    source: &'a SurfaceGenerator,
    chart_to_body: DQuat,
}

#[allow(dead_code)] // Shared adapter helpers used by the sibling family example.
impl<'a> RotatedSurfaceQuery<'a> {
    pub(crate) fn new(source: &'a SurfaceGenerator, chart_to_body: DQuat) -> Self {
        Self {
            source,
            chart_to_body,
        }
    }

    pub(crate) fn body_center_direction(self) -> DVec3 {
        self.chart_to_body * DVec3::Z
    }

    pub(crate) fn chart_to_body_axes(self) -> [DVec3; 3] {
        [
            self.chart_to_body * DVec3::X,
            self.chart_to_body * DVec3::Y,
            self.chart_to_body * DVec3::Z,
        ]
    }

    fn to_body_location(self, location: SurfaceLocation) -> anyhow::Result<SurfaceLocation> {
        Ok(SurfaceLocation::new(Direction3::try_new(
            self.chart_to_body * location.direction().unit(),
        )?))
    }

    fn to_chart_sample(self, sample: astrum_world::terrain::SurfaceSample) -> ReferenceSample {
        let body_to_chart = self.chart_to_body.conjugate();
        ReferenceSample {
            shape_radius_m: sample.shape().radius_m(),
            shape_gradient_m_per_unit_direction: body_to_chart * sample.shape().gradient_m(),
            terrain_height_m: sample.terrain().height_m(),
            terrain_gradient_m_per_unit_direction: body_to_chart
                * sample.terrain().tangent_gradient_m_per_unit_direction(),
            radius_m: sample.radius_m(),
            normal_body: body_to_chart * sample.normal(),
            material_weights: sample.material_weights(),
            work: [
                sample.work().cells_visited,
                sample.work().candidate_features,
            ],
            accepted_features: sample.work().accepted_features,
        }
    }
}

impl SurfaceQuery for RotatedSurfaceQuery<'_> {
    fn detail_diagnostics(
        &self,
        location: SurfaceLocation,
    ) -> anyhow::Result<Option<SurfaceDetailDiagnostics>> {
        let body_to_chart = self.chart_to_body.conjugate();
        Ok(self
            .source
            .detail_diagnostics(self.to_body_location(location)?)?
            .map(|d| SurfaceDetailDiagnostics {
                inherited_gradient_m: body_to_chart * d.inherited_gradient_m,
                regional_gradient_m: body_to_chart * d.regional_gradient_m,
                local_gradient_m: body_to_chart * d.local_gradient_m,
                fine_gradient_m: body_to_chart * d.fine_gradient_m,
                parent_morphology_gradient_per_unit_direction: body_to_chart
                    * d.parent_morphology_gradient_per_unit_direction,
                ..d
            }))
    }
    fn radius_m(&self) -> f64 {
        self.source.radius_m()
    }

    fn radial_envelope_m(&self) -> [f64; 2] {
        self.source.radial_envelope_m()
    }

    fn absolute_height_bound_m(&self) -> f64 {
        self.source.absolute_height_bound_m()
    }

    fn evaluate_point(&self, location: SurfaceLocation) -> anyhow::Result<ReferenceSample> {
        let body_location = self.to_body_location(location)?;
        Ok(self.to_chart_sample(self.source.evaluate_point(body_location)?))
    }

    fn geological_controls(
        &self,
        location: SurfaceLocation,
    ) -> anyhow::Result<Option<GeologicalControls>> {
        let body_location = self.to_body_location(location)?;
        Ok(self
            .source
            .geological_controls(body_location)?
            .map(|controls| GeologicalControls {
                structural_direction: self.chart_to_body.conjugate()
                    * controls.structural_direction,
                ..controls
            }))
    }

    fn view_target_radius_m(
        &self,
        location: SurfaceLocation,
        include_terrain_relief: bool,
    ) -> anyhow::Result<f64> {
        self.source
            .view_target_radius_m(self.to_body_location(location)?, include_terrain_relief)
    }

    fn body_to_chart_direction(&self, body_direction: DVec3) -> DVec3 {
        self.chart_to_body.conjugate() * body_direction
    }

    fn chart_to_body_direction(&self, chart_direction: DVec3) -> DVec3 {
        self.chart_to_body * chart_direction
    }
}
