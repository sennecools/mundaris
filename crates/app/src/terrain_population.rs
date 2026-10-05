//! Observer-local terrain admission for a populated system. Inactive bodies retain
//! disposable raw cache entries, never queued generation or pinned draw ownership.
use anyhow::Result;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::{BodyId, CoherentCelestialView};
use std::time::Duration;

/// Population-level candidate admission and ownership handoff timing.
#[cfg(feature = "surface-profile")]
#[derive(Debug, Default, Clone, Copy)]
pub struct TerrainPopulationProfile {
    pub candidate_admission: Duration,
    pub ownership_handoff: Duration,
    pub total: Duration,
}

use crate::{planet_surface::*, planet_terrain::*};

pub struct TerrainPopulation {
    #[cfg(feature = "surface-profile")]
    pub profile: TerrainPopulationProfile,
    pub cache: TerrainPatchCache,
    pub cover: AdaptiveTerrainCover,
    pub work: TerrainWorkReport,
    active_body: Option<BodyId>,
    far_probe: CelestialStaging,
}
impl TerrainPopulation {
    pub fn new() -> Result<Self> {
        Self::with_worker_count(0)
    }
    /// Native default retains application CPU headroom with four bounded workers.
    /// Serial operation-count fixtures use `new`; controls permit measured A/B.
    pub fn interactive() -> Result<Self> {
        let count = match std::env::var("MUNDARIS_TERRAIN_WORKERS") {
            Ok(value) => value.parse::<usize>()?,
            Err(std::env::VarError::NotPresent) => 4,
            Err(error) => return Err(error.into()),
        };
        Self::with_worker_count(count)
    }
    /// Explicit count makes serial/1/2/4 measurements use the same admission path.
    pub fn with_worker_count(worker_count: usize) -> Result<Self> {
        Ok(Self {
            #[cfg(feature = "surface-profile")]
            profile: TerrainPopulationProfile::default(),
            cache: TerrainPatchCache::new_with_workers(
                TERRAIN_CPU_CAP_BYTES,
                MAX_TERRAIN_PATCHES,
                worker_count,
            )?,
            cover: AdaptiveTerrainCover::default(),
            work: TerrainWorkReport::default(),
            active_body: None,
            far_probe: CelestialStaging::default(),
        })
    }
    pub fn active_body(&self) -> Option<BodyId> {
        self.active_body
    }
    /// One observer-local cover shares the inherited aggregate budget. The highest
    /// projected far error admits terrain; all other rocky bodies remain far-only.
    /// This is content streaming admission, not a physical size/range alteration.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        requests: &[CelestialRenderBody],
        sessions: &mut [PlanetSurfaceSession],
        owners: &mut [bool],
        sphere: &Icosphere,
        terrain_enabled: bool,
        morph_duration: Duration,
        vertex_budget: usize,
        wall_budget: Option<Duration>,
        elapsed: Duration,
    ) -> Result<()> {
        #[cfg(feature = "surface-profile")]
        let total_started = std::time::Instant::now();
        #[cfg(feature = "surface-profile")]
        {
            self.profile = TerrainPopulationProfile::default();
        }
        anyhow::ensure!(
            requests.len() == pair.system().body_count() && owners.len() == requests.len(),
            "population request association mismatch"
        );
        owners.fill(false);
        self.work = TerrainWorkReport::default();
        let index_of = |id| {
            pair.system()
                .bodies()
                .position(|(body, _)| body == id)
                .ok_or_else(|| anyhow::anyhow!("unknown surface capability body"))
        };
        let mut candidate = None;
        let mut greatest_error = 0.0;
        #[cfg(feature = "surface-profile")]
        let admission_started = std::time::Instant::now();
        for session in sessions.iter_mut() {
            let index = index_of(session.body())?;
            let body = pair.system().body(session.body())?;
            if terrain_enabled && let Some(definition) = body.terrain() {
                let input = SurfaceViewInput {
                    view,
                    body_fixed_frame: requests[index].body_fixed_frame,
                    reference_radius_m: requests[index].reference_radius_m,
                    projection,
                };
                if session
                    .terrain_required(&input, definition.config().absolute_height_bound_m())?
                    && session.far_error_pixels > greatest_error
                {
                    greatest_error = session.far_error_pixels;
                    candidate = Some(session.body());
                }
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.candidate_admission = admission_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let handoff_started = std::time::Instant::now();
        let mut retaining_source = false;
        if self.active_body != candidate {
            if let Some(old) = self.active_body {
                let index = index_of(old)?;
                let mut probe = CelestialFrame::new(view, &mut self.far_probe, projection, sphere);
                probe.append_bodies(&[requests[index]])?;
                if far_ready(probe.markers()[0].representation) {
                    self.cache.cancel_body_work(old);
                } else {
                    // The whole far sphere can cross the near plane while its
                    // displaced bound is outside the view. A marker is not a
                    // completed opaque replacement: retain coverage and retry.
                    candidate = Some(old);
                    retaining_source = true;
                }
            }
            if self.active_body != candidate {
                self.cover.abandon_construction(&mut self.cache);
                self.cover = AdaptiveTerrainCover::default();
                anyhow::ensure!(
                    self.cache.reserve_external(0),
                    "inactive cache reservation failed"
                );
                self.active_body = candidate;
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.ownership_handoff = handoff_started.elapsed();
        }
        let settings = LodSettings::default()
            .with_limits(2048, 2048, 30)?
            .with_work_limit(32 / sessions.len().max(1))?;
        for session in sessions.iter_mut() {
            let index = index_of(session.body())?;
            let request = requests[index];
            let input = SurfaceViewInput {
                view,
                body_fixed_frame: request.body_fixed_frame,
                reference_radius_m: request.reference_radius_m,
                projection,
            };
            let world_body = pair.system().body(session.body())?;
            if retaining_source
                && !terrain_enabled
                && self.active_body == Some(session.body())
                && world_body.terrain().is_some()
                && self.cover.ready()
            {
                // A disabled terrain request is not permission to remove its
                // last drawable owner before far preparation succeeds.
                self.cover.prepare_visible(&input)?;
                owners[index] = session.state() == SurfaceRepresentationState::Surface;
                continue;
            }
            if terrain_enabled && world_body.terrain().is_some() {
                if self.active_body == Some(session.body()) {
                    let definition = world_body
                        .terrain()
                        .ok_or_else(|| anyhow::anyhow!("terrain definition unavailable"))?;
                    let identity = TerrainGeometryIdentity::new(
                        session.body(),
                        definition.clone(),
                        world_body.terrain_revision(),
                        request.reference_radius_m,
                    )?;
                    self.cover.set_morph_duration(morph_duration)?;
                    self.work = self.cover.update_with_elapsed(
                        &mut self.cache,
                        &identity,
                        &input,
                        &settings,
                        vertex_budget,
                        wall_budget,
                        elapsed,
                    )?;
                    session.update_with_terrain_report(
                        &input,
                        definition.config().absolute_height_bound_m(),
                        self.cover.report,
                    )?;
                    for patch in self.cover.visible() {
                        self.cache.get(&identity, patch.address)?;
                    }
                    owners[index] = session.state() == SurfaceRepresentationState::Surface
                        && self.cover.ready();
                } else {
                    let mut probe =
                        CelestialFrame::new(view, &mut self.far_probe, projection, sphere);
                    probe.append_bodies(&[request])?;
                    session
                        .relinquish_to_far_if_ready(far_ready(probe.markers()[0].representation));
                }
            } else {
                session.update(&input, &settings.with_limits(2048, 32768, 30)?)?;
                if session.far_error_pixels < 0.05 {
                    let mut probe =
                        CelestialFrame::new(view, &mut self.far_probe, projection, sphere);
                    probe.append_bodies(&[request])?;
                    session.return_to_far_if_ready(far_ready(probe.markers()[0].representation));
                }
                owners[index] = session.state() == SurfaceRepresentationState::Surface;
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.total = total_started.elapsed();
        }
        Ok(())
    }
}
fn far_ready(representation: SphereRepresentation) -> bool {
    matches!(
        representation,
        SphereRepresentation::PhysicalSphere
            | SphereRepresentation::SubpixelMarker
            | SphereRepresentation::Culled
    )
}
