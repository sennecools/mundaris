//! Disposable, generator-bound canonical field pages. Never world authority.
use super::{TileBuildError, TileBuildIdentity};
use glam::DVec3;
use mundaris_math::{Direction3, surface::CubeFace};
use mundaris_world::terrain::SurfaceGenerator;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Independent field density over one resident patch footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldDensity {
    Cells32,
    Cells64,
    Cells128,
}

impl FieldDensity {
    pub const fn cells(self) -> u32 {
        match self {
            Self::Cells32 => 32,
            Self::Cells64 => 64,
            Self::Cells128 => 128,
        }
    }
    /// Version includes density, bilinear radius/classified-material channels,
    /// and the unchanged seven-tap resident filter applied after reconstruction.
    pub const fn filter_version(self) -> u32 {
        0x0002_4700 + self.cells()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct FieldValue {
    pub radius_m: f64,
    pub material: [f64; 4],
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct FieldStatistics {
    pub authoritative_queries: u64,
    pub node_hits: u64,
    pub page_allocations: u64,
    pub evictions: u64,
    pub retained_bytes: usize,
    pub capacity_bytes: usize,
    /// Summed source service across workers; it may exceed wall elapsed time.
    pub source_service_ns: u64,
    /// Cold results already published by another worker before this publication.
    pub duplicate_source_queries: u64,
    pub prepared_nodes: u64,
    pub source_cells_visited: u64,
    pub source_candidate_features: u64,
    /// Complete point queries compute analytic derivatives even when fields retain only values.
    pub derivative_queries: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PageAddress {
    face: CubeFace,
    level: u8,
    xy: [u32; 2],
}
struct Page {
    address: PageAddress,
    nodes: Vec<Option<FieldValue>>,
    touched: u64,
}
struct Store {
    pages: Vec<Page>,
    clock: u64,
    stats: FieldStatistics,
}

/// Full immutable definition/radius and persistent body/revisions bind a store.
/// Pages add canonical address, density and filter/channel identity. Pose is absent.
pub struct SharedFieldPages {
    generator: Arc<SurfaceGenerator>,
    identity: TileBuildIdentity,
    density: FieldDensity,
    page_cap: usize,
    page_bytes: usize,
    store: Mutex<Store>,
}
impl SharedFieldPages {
    /// Reserve this entire logical capacity inside the existing terrain budget.
    /// Nodes are evaluated lazily, but every admitted page's storage is charged.
    pub fn new(
        generator: Arc<SurfaceGenerator>,
        identity: TileBuildIdentity,
        density: FieldDensity,
        capacity_bytes: usize,
    ) -> Result<Self, TileBuildError> {
        let side = density.cells() as usize + 1;
        let page_bytes =
            side * side * std::mem::size_of::<Option<FieldValue>>() + std::mem::size_of::<Page>();
        // Fixed owner/lock/statistics metadata plus conservative allocation allowance.
        let overhead = std::mem::size_of::<Self>() + std::mem::size_of::<Store>() + 256;
        // One replacement page can coexist with the old allocation during eviction.
        let page_cap = capacity_bytes.saturating_sub(overhead + page_bytes) / page_bytes;
        if page_cap == 0 {
            return Err(TileBuildError::InvalidSample);
        }
        let pages = Vec::with_capacity(page_cap);
        let actual_capacity = overhead + (pages.capacity() + 1) * page_bytes;
        if actual_capacity > capacity_bytes {
            return Err(TileBuildError::InvalidSample);
        }
        Ok(Self {
            generator,
            identity,
            density,
            page_cap,
            page_bytes,
            store: Mutex::new(Store {
                pages,
                clock: 0,
                stats: FieldStatistics {
                    retained_bytes: overhead + page_cap * std::mem::size_of::<Page>(),
                    capacity_bytes: actual_capacity,
                    ..FieldStatistics::default()
                },
            }),
        })
    }
    pub fn density(&self) -> FieldDensity {
        self.density
    }
    pub fn statistics(&self) -> FieldStatistics {
        self.store.lock().unwrap_or_else(|p| p.into_inner()).stats
    }
    pub(super) fn matches(
        &self,
        generator: &SurfaceGenerator,
        identity: TileBuildIdentity,
    ) -> bool {
        self.generator.definition() == generator.definition()
            && self.generator.radius_m().to_bits() == generator.radius_m().to_bits()
            && self.identity.body_identity == identity.body_identity
            && self.identity.surface_revision == identity.surface_revision
            && self.identity.material_revision == identity.material_revision
    }
    pub(super) fn sample(
        &self,
        direction: Direction3,
        level: u8,
    ) -> Result<(FieldValue, u64), TileBuildError> {
        if level > 30 {
            return Err(TileBuildError::InvalidAddress);
        }
        let n = direction.unit();
        // First face in fixed order wins exact dominant-axis ties, including corners.
        let face = CubeFace::ALL
            .into_iter()
            .max_by(|a, b| {
                n.dot(a.basis()[0])
                    .total_cmp(&n.dot(b.basis()[0]))
                    .then_with(|| b.cmp(a))
            })
            .ok_or(TileBuildError::InvalidAddress)?;
        let [normal, u, v] = face.basis();
        let scale = f64::from(self.density.cells()) * (1u64 << level) as f64;
        let q = [
            ((n.dot(u) / n.dot(normal) + 1.0) * 0.5 * scale).clamp(0.0, scale),
            ((n.dot(v) / n.dot(normal) + 1.0) * 0.5 * scale).clamp(0.0, scale),
        ];
        let lower = q.map(|x| (x.floor() as u64).min(scale as u64 - 1));
        let fractions = [q[0] - lower[0] as f64, q[1] - lower[1] as f64];
        let cells = u64::from(self.density.cells());
        let address = PageAddress {
            face,
            level,
            xy: [(lower[0] / cells) as u32, (lower[1] / cells) as u32],
        };
        let local = [(lower[0] % cells) as usize, (lower[1] % cells) as usize];
        let side = self.density.cells() as usize + 1;
        let indices = [
            local[1] * side + local[0],
            local[1] * side + local[0] + 1,
            (local[1] + 1) * side + local[0],
            (local[1] + 1) * side + local[0] + 1,
        ];
        let mut values = [None; 4];
        {
            let mut store = self.store.lock().unwrap_or_else(|p| p.into_inner());
            store.clock = store.clock.saturating_add(1);
            let clock = store.clock;
            if let Some(page) = store.pages.iter_mut().find(|p| p.address == address) {
                page.touched = clock;
                values = indices.map(|i| page.nodes[i]);
            }
            store.stats.node_hits += values.iter().filter(|v| v.is_some()).count() as u64;
        }
        // Complete queries happen without the cache lock. Racing cold workers may
        // duplicate work; count it instead of holding a global geological lock.
        let mut queries = 0;
        let missing = values.map(|value| value.is_none());
        let mut source_service_ns = 0u64;
        let mut source_cells_visited = 0u64;
        let mut source_candidate_features = 0u64;
        for (index, value) in values.iter_mut().enumerate() {
            if value.is_some() {
                continue;
            }
            let x = lower[0] + (index % 2) as u64;
            let y = lower[1] + (index / 2) as u64;
            let denominator = cells << level;
            let a = 2 * x as i64 - denominator as i64;
            let b = 2 * y as i64 - denominator as i64;
            let mut xyz: [i64; 3] = std::array::from_fn(|k| {
                normal[k] as i64 * denominator as i64 + u[k] as i64 * a + v[k] as i64 * b
            });
            while xyz.iter().all(|x| x % 2 == 0) {
                xyz = xyz.map(|x| x / 2);
            }
            let d = Direction3::try_new(DVec3::new(xyz[0] as f64, xyz[1] as f64, xyz[2] as f64))
                .map_err(|_| TileBuildError::InvalidAddress)?;
            let source_started = Instant::now();
            let sample = self
                .generator
                .evaluate_point(mundaris_math::surface::SurfaceLocation::new(d))?;
            source_service_ns = source_service_ns.saturating_add(
                source_started
                    .elapsed()
                    .as_nanos()
                    .min(u128::from(u64::MAX)) as u64,
            );
            source_cells_visited =
                source_cells_visited.saturating_add(u64::from(sample.work().cells_visited));
            source_candidate_features = source_candidate_features
                .saturating_add(u64::from(sample.work().candidate_features));
            *value = Some(FieldValue {
                radius_m: sample.radius_m(),
                material: sample.material_weights(),
            });
            queries += 1;
        }
        {
            let mut store = self.store.lock().unwrap_or_else(|p| p.into_inner());
            store.stats.authoritative_queries += queries;
            store.stats.source_service_ns = store
                .stats
                .source_service_ns
                .saturating_add(source_service_ns);
            store.stats.source_cells_visited = store
                .stats
                .source_cells_visited
                .saturating_add(source_cells_visited);
            store.stats.source_candidate_features = store
                .stats
                .source_candidate_features
                .saturating_add(source_candidate_features);
            store.stats.derivative_queries += queries;
            store.clock = store.clock.saturating_add(1);
            let clock = store.clock;
            let page_index = if let Some(i) = store.pages.iter().position(|p| p.address == address)
            {
                i
            } else {
                let page = Page {
                    address,
                    nodes: vec![None; side * side],
                    touched: clock,
                };
                store.stats.page_allocations += 1;
                if store.pages.len() < self.page_cap {
                    store.pages.push(page);
                    store.stats.retained_bytes += self.page_bytes - std::mem::size_of::<Page>();
                    store.pages.len() - 1
                } else {
                    let i = store
                        .pages
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, p)| p.touched)
                        .map(|(i, _)| i)
                        .ok_or(TileBuildError::InvalidSample)?;
                    store.pages[i] = page;
                    store.stats.evictions += 1;
                    i
                }
            };
            let page = &mut store.pages[page_index];
            page.touched = clock;
            let mut duplicates = 0;
            let mut prepared = 0;
            for ((i, value), was_missing) in indices.into_iter().zip(values).zip(missing) {
                duplicates += u64::from(was_missing && page.nodes[i].is_some());
                prepared += u64::from(was_missing && page.nodes[i].is_none());
                page.nodes[i] = value;
            }
            store.stats.duplicate_source_queries += duplicates;
            store.stats.prepared_nodes += prepared;
        }
        let weights = [
            (1.0 - fractions[0]) * (1.0 - fractions[1]),
            fractions[0] * (1.0 - fractions[1]),
            (1.0 - fractions[0]) * fractions[1],
            fractions[0] * fractions[1],
        ];
        let mut result = FieldValue {
            radius_m: 0.0,
            material: [0.0; 4],
        };
        for (value, weight) in values.into_iter().zip(weights) {
            let value = value.ok_or(TileBuildError::InvalidSample)?;
            result.radius_m += value.radius_m * weight;
            for (out, input) in result.material.iter_mut().zip(value.material) {
                *out += input * weight;
            }
        }
        Ok((result, queries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::surface::{CubePatchAddress, PatchEdge};
    use mundaris_world::terrain::{
        SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
    };

    fn fixture(radius: f64) -> (Arc<SurfaceGenerator>, TileBuildIdentity) {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(71),
            TerrainSeed(2),
            SurfaceAlgorithm::MoonFieldsV1,
        );
        (
            Arc::new(SurfaceGenerator::new(&definition, radius).unwrap()),
            TileBuildIdentity {
                body_identity: 5,
                surface_revision: 3,
                material_revision: 7,
            },
        )
    }
    #[test]
    fn field_pages_rebuild_deterministically_under_pressure() {
        let (generator, identity) = fixture(1_737_400.0);
        let pages =
            SharedFieldPages::new(generator.clone(), identity, FieldDensity::Cells32, 140_000)
                .unwrap();
        let direction = Direction3::try_new(DVec3::new(0.3, 0.7, 1.0)).unwrap();
        let initial = pages.sample(direction, 12).unwrap().0;
        for face in CubeFace::ALL {
            let d = face.direction([0.33, 0.17]).unwrap();
            pages.sample(d, 12).unwrap();
        }
        assert_eq!(initial, pages.sample(direction, 12).unwrap().0);
        let stats = pages.statistics();
        assert!(stats.evictions > 0);
        assert!(stats.retained_bytes <= stats.capacity_bytes && stats.capacity_bytes <= 140_000);
        assert!(pages.matches(&generator, identity));
        assert!(!pages.matches(
            &generator,
            TileBuildIdentity {
                surface_revision: 4,
                ..identity
            }
        ));
        let (other_radius, _) = fixture(109_081.776_8);
        assert!(!pages.matches(&other_radius, identity));
    }
    #[test]
    fn canonical_page_nodes_agree_on_all_faces_edges_and_corners() {
        for radius in [109_081.776_8, 1_737_400.0] {
            let (generator, identity) = fixture(radius);
            let pages = SharedFieldPages::new(
                generator.clone(),
                identity,
                FieldDensity::Cells32,
                8 * 1024 * 1024,
            )
            .unwrap();
            for face in CubeFace::ALL {
                let root = CubePatchAddress::root(face);
                for edge in PatchEdge::ALL {
                    let neighbor = root.neighbor(edge);
                    for k in 0..=32 {
                        let [x, y] = edge.grid(k, 32);
                        let mapped = if neighbor.reversed { 32 - k } else { k };
                        let [nx, ny] = neighbor.edge.grid(mapped, 32);
                        let a = root.sample_direction(x, y, 32).unwrap();
                        let b = neighbor.address.sample_direction(nx, ny, 32).unwrap();
                        assert_eq!(a, b);
                        let value = pages.sample(a, 0).unwrap().0;
                        assert_eq!(value, pages.sample(b, 0).unwrap().0);
                        let oracle = generator
                            .evaluate_point(mundaris_math::surface::SurfaceLocation::new(a))
                            .unwrap();
                        assert!((value.radius_m - oracle.radius_m()).abs() < 1e-8);
                        for (v, w) in value.material.into_iter().zip(oracle.material_weights()) {
                            assert!((v - w).abs() < 1e-12);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn field_mesh_warm_rebuild_uses_no_new_source_queries_and_density_keys_differ() {
        let (generator, identity) = fixture(109_081.776_8);
        let address = CubePatchAddress::try_new(CubeFace::PositiveZ, 16, 32768, 32768).unwrap();
        for density in [
            FieldDensity::Cells32,
            FieldDensity::Cells64,
            FieldDensity::Cells128,
        ] {
            let pages =
                SharedFieldPages::new(generator.clone(), identity, density, 8 * 1024 * 1024)
                    .unwrap();
            let (cold, cost) = super::super::ResidentTileBuilder::build_fields(
                &generator, identity, address, 32, &pages,
            )
            .unwrap();
            let (warm, warm_cost) = super::super::ResidentTileBuilder::build_fields(
                &generator, identity, address, 32, &pages,
            )
            .unwrap();
            assert_eq!(cold, warm);
            assert!(cost.authoritative_query_count > 7);
            assert_eq!(warm_cost.authoritative_query_count, 7);
            assert_eq!(cold.key.filter_version, density.filter_version());
            assert_ne!(cold.key.filter_version, super::super::TILE_FILTER_VERSION);
        }
    }
}
