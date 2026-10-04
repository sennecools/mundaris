//! Exact local triangle overlays of two valid stitched surfaces.
use super::{ActiveSurfacePatch, StitchedSurface, SurfaceGeometrySample, SurfaceTopology};
use crate::RenderPreparationError;
use mundaris_math::surface::CubePatchAddress;
use std::mem::size_of;
#[cfg(feature = "surface-profile")]
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, RenderPreparationError>;
fn invalid() -> RenderPreparationError {
    RenderPreparationError::InvalidDebugGeometry
}

/// Bounded local exact predicates. Integer triangle coordinates are at most 32;
/// checked reduced fractions also cover stitched-diagonal intersections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rational {
    n: i128,
    d: i128,
}
impl Rational {
    fn integer(n: i128) -> Self {
        Self { n, d: 1 }
    }
    fn new(n: i128, d: i128) -> Result<Self> {
        if d == 0 || n == i128::MIN || d == i128::MIN {
            return Err(invalid());
        }
        // Canonical dyadic overlay endpoints overwhelmingly have denominator
        // one. Their exact representation requires no GCD or integer division.
        if d == 1 {
            return Ok(Self::integer(n));
        }
        if n == 0 {
            return Ok(Self::integer(0));
        }
        let sign = if d < 0 { -1 } else { 1 };
        let (mut a, mut b) = (n.abs(), d.abs());
        while b != 0 {
            let r = a % b;
            a = b;
            b = r;
        }
        let gcd = a.max(1);
        Ok(Self {
            n: n / gcd * sign,
            d: d.abs() / gcd,
        })
    }
    fn add(self, b: Self) -> Result<Self> {
        Self::new(
            self.n
                .checked_mul(b.d)
                .and_then(|a| b.n.checked_mul(self.d).and_then(|b| a.checked_add(b)))
                .ok_or_else(invalid)?,
            self.d.checked_mul(b.d).ok_or_else(invalid)?,
        )
    }
    fn sub(self, b: Self) -> Result<Self> {
        self.add(Self { n: -b.n, d: b.d })
    }
    fn mul(self, b: Self) -> Result<Self> {
        Self::new(
            self.n.checked_mul(b.n).ok_or_else(invalid)?,
            self.d.checked_mul(b.d).ok_or_else(invalid)?,
        )
    }
    fn div(self, b: Self) -> Result<Self> {
        Self::new(
            self.n.checked_mul(b.d).ok_or_else(invalid)?,
            self.d.checked_mul(b.n).ok_or_else(invalid)?,
        )
    }
    fn value(self) -> f64 {
        self.n as f64 / self.d as f64
    }
}
type Point = [Rational; 2];
#[derive(Clone, Copy)]
struct TriangleBounds {
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
}
#[derive(Clone, Copy)]
struct MappedTriangle {
    points: [Point; 3],
    indices: [u16; 3],
    bounds: TriangleBounds,
}
#[derive(Clone, Copy)]
struct BinLink {
    triangle: u16,
    next: u16,
}
fn bounds(points: [Point; 3]) -> TriangleBounds {
    TriangleBounds {
        min_x: points.iter().map(|p| p[0].n as u32).min().unwrap(),
        max_x: points.iter().map(|p| p[0].n as u32).max().unwrap(),
        min_y: points.iter().map(|p| p[1].n as u32).min().unwrap(),
        max_y: points.iter().map(|p| p[1].n as u32).max().unwrap(),
    }
}
fn bbox_overlaps(a: TriangleBounds, b: TriangleBounds) -> bool {
    a.min_x <= b.max_x && b.min_x <= a.max_x && a.min_y <= b.max_y && b.min_y <= a.max_y
}
/// Rejects pairs whose interiors are separated by an edge of either CCW triangle.
/// Mapped coordinates are integers in [0, 32], so differences are at most 32
/// and checked i128 determinants are comfortably bounded. Equality is rejected:
/// a shared edge/vertex has zero area and cannot contribute overlay triangles.
fn positive_area_overlap(a: [Point; 3], b: [Point; 3]) -> Result<bool> {
    for (triangle, other) in [(a, b), (b, a)] {
        for edge in 0..3 {
            let p = triangle[edge];
            let q = triangle[(edge + 1) % 3];
            let dx = q[0].n.checked_sub(p[0].n).ok_or_else(invalid)?;
            let dy = q[1].n.checked_sub(p[1].n).ok_or_else(invalid)?;
            let mut any_inside = false;
            for point in other {
                let x = point[0].n.checked_sub(p[0].n).ok_or_else(invalid)?;
                let y = point[1].n.checked_sub(p[1].n).ok_or_else(invalid)?;
                let cross = dx
                    .checked_mul(y)
                    .and_then(|v| dy.checked_mul(x).and_then(|w| v.checked_sub(w)))
                    .ok_or_else(invalid)?;
                any_inside |= cross > 0;
            }
            if !any_inside {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
fn bin_range(min: u32, max: u32, cells: u32) -> std::ops::RangeInclusive<usize> {
    (min.min(cells - 1) as usize)..=(max.min(cells - 1) as usize)
}
fn subtract(a: Point, b: Point) -> Result<Point> {
    Ok([a[0].sub(b[0])?, a[1].sub(b[1])?])
}
fn cross(a: Point, b: Point) -> Result<Rational> {
    a[0].mul(b[1])?.sub(a[1].mul(b[0])?)
}
fn oriented(a: Point, b: Point, c: Point) -> Result<Rational> {
    if a.into_iter().chain(b).chain(c).all(|p| p.d == 1) {
        let dx = b[0].n.checked_sub(a[0].n).ok_or_else(invalid)?;
        let dy = b[1].n.checked_sub(a[1].n).ok_or_else(invalid)?;
        let x = c[0].n.checked_sub(a[0].n).ok_or_else(invalid)?;
        let y = c[1].n.checked_sub(a[1].n).ok_or_else(invalid)?;
        let determinant = dx
            .checked_mul(y)
            .and_then(|v| dy.checked_mul(x).and_then(|w| v.checked_sub(w)))
            .ok_or_else(invalid)?;
        return Rational::new(determinant, 1);
    }
    cross(subtract(b, a)?, subtract(c, a)?)
}
fn intersection(a: Point, b: Point, da: Rational, db: Rational) -> Result<Point> {
    let t = da.div(da.sub(db)?)?;
    Ok([
        a[0].add(b[0].sub(a[0])?.mul(t)?)?,
        a[1].add(b[1].sub(a[1])?.mul(t)?)?,
    ])
}
fn overlay(old: [Point; 3], new: [Point; 3]) -> Result<([Point; 8], usize)> {
    let zero = [Rational::integer(0); 2];
    let mut vertices = [zero; 8];
    vertices[..3].copy_from_slice(&old);
    let mut len = 3;
    for edge in 0..3 {
        let mut next = [zero; 8];
        let mut count = 0;
        if len == 0 {
            break;
        }
        let mut previous = vertices[len - 1];
        let mut dp = oriented(new[edge], new[(edge + 1) % 3], previous)?;
        for current in vertices[..len].iter().copied() {
            let dc = oriented(new[edge], new[(edge + 1) % 3], current)?;
            if (dp.n < 0 && dc.n > 0) || (dp.n > 0 && dc.n < 0) {
                if count == 8 {
                    return Err(invalid());
                }
                next[count] = intersection(previous, current, dp, dc)?;
                count += 1;
            }
            if dc.n >= 0 {
                if count == 8 {
                    return Err(invalid());
                }
                if count == 0 || next[count - 1] != current {
                    next[count] = current;
                    count += 1;
                }
            }
            previous = current;
            dp = dc;
        }
        if count > 1 && next[0] == next[count - 1] {
            count -= 1;
        }
        vertices = next;
        len = count;
    }
    Ok((vertices, len))
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceTriangleReference {
    pub address: CubePatchAddress,
    pub indices: [u16; 3],
    pub weights: [f64; 3],
}
#[derive(Debug, Clone, Copy)]
pub struct TransitionVertex {
    pub old: SurfaceGeometrySample,
    pub new: SurfaceGeometrySample,
    pub old_reference: SurfaceTriangleReference,
    pub new_reference: SurfaceTriangleReference,
    pub old_elevation: f64,
    pub new_elevation: f64,
}
impl TransitionVertex {
    pub fn sample(self, fraction: f64) -> Result<SurfaceGeometrySample> {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return Err(invalid());
        }
        // Preserve the affine analytic normal field through overlay subdivision.
        // The fragment shader renormalizes after spatial interpolation; doing it
        // here would change the endpoint lighting of the captured triangles.
        let sample = if fraction == 0.0 {
            self.old
        } else if fraction == 1.0 {
            self.new
        } else {
            SurfaceGeometrySample {
                position_body_m: self
                    .old
                    .position_body_m
                    .lerp(self.new.position_body_m, fraction),
                normal_body: self.old.normal_body.lerp(self.new.normal_body, fraction),
            }
        };
        if !sample.position_body_m.is_finite()
            || !sample.normal_body.is_finite()
            || sample.normal_body.length_squared() < 1e-20
            || sample.position_body_m.dot(sample.normal_body) <= 0.0
        {
            return Err(invalid());
        }
        Ok(sample)
    }
}
pub struct SurfaceTransition {
    triangles: Vec<[TransitionVertex; 3]>,
    affected_old: Vec<CubePatchAddress>,
    affected_new: Vec<CubePatchAddress>,
    max_displacement_m: f64,
    #[cfg(feature = "surface-profile")]
    profile: SurfaceTransitionProfile,
}

/// Opt-in CPU stage measurements and work counts for a surface transition build.
#[cfg(feature = "surface-profile")]
#[derive(Debug, Default, Clone, Copy)]
pub struct SurfaceTransitionProfile {
    /// Input validation, changed-patch discovery, and build setup.
    pub validation_setup: Duration,
    /// Pair mapping and exact integer bounding-box tests.
    pub pair_mapping_bbox: Duration,
    /// Exact triangle overlay clipping and polygon triangulation.
    pub exact_overlay_clipping: Duration,
    /// Barycentric endpoint capture, including normals and elevation.
    pub barycentric_endpoint_capture: Duration,
    /// Canonical boundary key lookup and reuse.
    pub boundary_canonicalization: Duration,
    /// Output capacity growth and transition-triangle emission.
    pub allocation_triangle_emission: Duration,
    pub overlapping_patch_pairs: usize,
    pub destination_triangles_precomputed: usize,
    /// Destination triangles supplied by identical-address/mask templates.
    pub identity_template_triangles: usize,
    pub potential_triangle_pairs: usize,
    pub triangle_pairs_considered: usize,
    /// Candidates rejected by integer bounds or exact separating edges.
    pub bbox_rejected_pairs: usize,
    pub clipped_pairs: usize,
    pub emitted_triangles: usize,
    pub boundary_vertices: usize,
}
impl SurfaceTransition {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        old_cover: &[ActiveSurfacePatch],
        old: &StitchedSurface,
        new_cover: &[ActiveSurfacePatch],
        new: &StitchedSurface,
        topology: &SurfaceTopology,
        max_bytes: usize,
    ) -> Result<Self> {
        Self::build_cancellable(old_cover, old, new_cover, new, topology, max_bytes, || {
            false
        })?
        .ok_or_else(invalid)
    }

    /// Builds an exact transition, returning `Ok(None)` when the caller cancels.
    /// The callback is polled throughout candidate enumeration and overlay emission.
    #[allow(clippy::too_many_arguments)]
    pub fn build_cancellable(
        old_cover: &[ActiveSurfacePatch],
        old: &StitchedSurface,
        new_cover: &[ActiveSurfacePatch],
        new: &StitchedSurface,
        topology: &SurfaceTopology,
        max_bytes: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<Self>> {
        Self::build_impl(
            old_cover,
            old,
            new_cover,
            new,
            topology,
            max_bytes,
            true,
            true,
            &mut cancelled,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_impl(
        old_cover: &[ActiveSurfacePatch],
        old: &StitchedSurface,
        new_cover: &[ActiveSurfacePatch],
        new: &StitchedSurface,
        topology: &SurfaceTopology,
        max_bytes: usize,
        use_identity_templates: bool,
        reject_touching_pairs: bool,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Option<Self>> {
        #[cfg(feature = "surface-profile")]
        let build_started = Instant::now();
        if old_cover.len() != old.patches().len() || new_cover.len() != new.patches().len() {
            return Err(invalid());
        }
        if old.patches().first().map(|p| p.reference_radius_m())
            != new.patches().first().map(|p| p.reference_radius_m())
        {
            return Err(invalid());
        }
        for (cover, surface) in [(old_cover, old), (new_cover, new)] {
            if cover
                .iter()
                .zip(surface.patches())
                .enumerate()
                .any(|(i, (p, g))| {
                    p.address != g.address() || surface.stitch_mask(i) != Some(p.stitch_mask)
                })
            {
                return Err(invalid());
            }
        }
        let changed = |p: &ActiveSurfacePatch,
                       g: &super::GeneratedSurfacePatch,
                       cover: &[ActiveSurfacePatch],
                       surface: &StitchedSurface| {
            !cover
                .binary_search_by_key(&p.address, |p| p.address)
                .is_ok_and(|i| {
                    cover[i].stitch_mask == p.stitch_mask
                        && surface.patches()[i].samples() == g.samples()
                })
        };
        let old_count = old_cover
            .iter()
            .zip(old.patches())
            .filter(|(p, g)| changed(p, g, new_cover, new))
            .count();
        let new_count = new_cover
            .iter()
            .zip(new.patches())
            .filter(|(p, g)| changed(p, g, old_cover, old))
            .count();
        // Only local boundary records persist during construction. Charge a
        // pessimistic sorted-reference capacity and fixed predicate/clip stack
        // allowance before allocating even the affected-address lists.
        let count = old_count.saturating_add(new_count);
        // Only integer overlay-domain points enter the canonical boundary map.
        // An old grid16 leaf occupies at most a 17x17 integer lattice, or 33x33
        // when its destination is one level finer. Counting each affected old
        // leaf separately overcounts shared edges, but never misses an integer
        // clipping intersection. This is a proved capacity bound, not a sampled
        // estimate or a smaller output-quality budget.
        let boundary_limit = old_cover
            .iter()
            .zip(old.patches())
            .filter(|(p, g)| changed(p, g, new_cover, new))
            .map(|(p, _)| {
                if new_cover
                    .iter()
                    .any(|n| p.address.contains(n.address) && n.address.level() > p.address.level())
                {
                    33usize * 33
                } else {
                    17usize * 17
                }
            })
            .sum::<usize>();
        let scratch_bytes = boundary_limit
            .saturating_mul(size_of::<(
                u128,
                (SurfaceGeometrySample, SurfaceGeometrySample),
            )>())
            .saturating_add(16 * 1024);
        let mapped_triangles = topology.indices(0).len() / 3;
        let mapped_triangle_bytes = mapped_triangles.saturating_mul(size_of::<MappedTriangle>());
        // A triangle spans at most five inclusive bins per axis, including an
        // endpoint exactly on a bin boundary. The insertion guard below keeps
        // the actual scratch allocation within this preflight reservation.
        let bin_link_capacity = mapped_triangles.saturating_mul(25);
        let bin_scratch_bytes = bin_link_capacity.saturating_mul(size_of::<BinLink>());
        let scratch_bytes = scratch_bytes
            .saturating_add(mapped_triangle_bytes)
            .saturating_add(bin_scratch_bytes);
        let overhead = size_of::<Self>()
            .saturating_add(count.saturating_mul(size_of::<CubePatchAddress>()))
            .saturating_add(scratch_bytes);
        if overhead > max_bytes {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let mut affected_old = Vec::with_capacity(old_count);
        affected_old.extend(
            old_cover
                .iter()
                .zip(old.patches())
                .filter(|(p, g)| changed(p, g, new_cover, new))
                .map(|(p, _)| p.address),
        );
        let mut affected_new = Vec::with_capacity(new_count);
        affected_new.extend(
            new_cover
                .iter()
                .zip(new.patches())
                .filter(|(p, g)| changed(p, g, old_cover, old))
                .map(|(p, _)| p.address),
        );
        let mut transition = Self {
            triangles: Vec::new(),
            affected_old,
            affected_new,
            max_displacement_m: 0.0,
            #[cfg(feature = "surface-profile")]
            profile: SurfaceTransitionProfile::default(),
        };
        #[cfg(feature = "surface-profile")]
        let mut validation_finished = false;
        let mut boundary =
            Vec::<(u128, (SurfaceGeometrySample, SurfaceGeometrySample))>::with_capacity(
                boundary_limit,
            );
        if transition.resident_bytes().saturating_add(scratch_bytes) > max_bytes {
            return Err(RenderPreparationError::InvalidBudget);
        }
        for (oi, op) in old_cover
            .iter()
            .enumerate()
            .filter(|(_, p)| transition.affected_old.binary_search(&p.address).is_ok())
        {
            if cancelled() {
                return Ok(None);
            }
            for (ni, np) in new_cover.iter().enumerate() {
                if cancelled() {
                    return Ok(None);
                }
                if !op.address.contains(np.address) && !np.address.contains(op.address) {
                    continue;
                }
                if transition.affected_new.binary_search(&np.address).is_err() {
                    return Err(invalid());
                }
                if op.address.level().abs_diff(np.address.level()) > 1 {
                    return Err(invalid());
                }
                let base = if op.address.level() < np.address.level() {
                    op.address
                } else {
                    np.address
                };
                let level = op.address.level().max(np.address.level());
                let cells = 16u32 << (level - base.level());
                let mapped = |address: CubePatchAddress, indices: [u16; 3]| -> [Point; 3] {
                    let shift = level - address.level();
                    let offset = address.coordinates();
                    let origin = base.coordinates();
                    indices.map(|k| {
                        [
                            Rational::integer(
                                i128::from(
                                    (u64::from(offset[0]) * 16 + u64::from(k % 17)) << shift,
                                ) - i128::from(u64::from(origin[0]) * u64::from(cells)),
                            ),
                            Rational::integer(
                                i128::from(
                                    (u64::from(offset[1]) * 16 + u64::from(k / 17)) << shift,
                                ) - i128::from(u64::from(origin[1]) * u64::from(cells)),
                            ),
                        ]
                    })
                };
                let identity_topology = use_identity_templates
                    && op.address == np.address
                    && op.stitch_mask == np.stitch_mask;
                let mut mapped_new = Vec::new();
                let mut bin_heads = [u16::MAX; 32 * 32];
                let mut bin_links = Vec::new();
                #[cfg(feature = "surface-profile")]
                let mapping_started = Instant::now();
                if !identity_topology {
                    mapped_new = Vec::with_capacity(mapped_triangles);
                    for &nt in topology.indices(np.stitch_mask).as_chunks::<3>().0 {
                        if cancelled() {
                            return Ok(None);
                        }
                        let points = mapped(np.address, nt);
                        mapped_new.push(MappedTriangle {
                            points,
                            indices: nt,
                            bounds: bounds(points),
                        });
                    }
                    bin_links = Vec::with_capacity(bin_link_capacity);
                    for (triangle_index, triangle) in mapped_new.iter().enumerate() {
                        if cancelled() {
                            return Ok(None);
                        }
                        for y in bin_range(triangle.bounds.min_y, triangle.bounds.max_y, cells) {
                            for x in bin_range(triangle.bounds.min_x, triangle.bounds.max_x, cells)
                            {
                                if bin_links.len() == bin_link_capacity
                                    || triangle_index >= u16::MAX as usize
                                {
                                    return Err(RenderPreparationError::InvalidBudget);
                                }
                                let bin = y * cells as usize + x;
                                let link_index =
                                    u16::try_from(bin_links.len()).map_err(|_| invalid())?;
                                bin_links.push(BinLink {
                                    triangle: triangle_index as u16,
                                    next: bin_heads[bin],
                                });
                                bin_heads[bin] = link_index;
                            }
                        }
                    }
                }
                #[cfg(feature = "surface-profile")]
                {
                    if cancelled() {
                        return Ok(None);
                    }
                    transition.profile.pair_mapping_bbox += mapping_started.elapsed();
                    transition.profile.overlapping_patch_pairs += 1;
                    transition.profile.destination_triangles_precomputed += mapped_new.len();
                    transition.profile.potential_triangle_pairs +=
                        (topology.indices(op.stitch_mask).len() / 3)
                            .saturating_mul(topology.indices(np.stitch_mask).len() / 3);
                    validation_finished = true;
                }
                for (old_triangle_index, &ot) in topology
                    .indices(op.stitch_mask)
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .enumerate()
                {
                    let od = mapped(op.address, ot);
                    let old_bounds = bounds(od);
                    #[cfg(feature = "surface-profile")]
                    let bbox_started = Instant::now();
                    let mut candidates = [0u64; 8];
                    if identity_topology {
                        candidates[old_triangle_index / 64] |= 1u64 << (old_triangle_index % 64);
                    } else {
                        for y in bin_range(old_bounds.min_y, old_bounds.max_y, cells) {
                            for x in bin_range(old_bounds.min_x, old_bounds.max_x, cells) {
                                let mut link = bin_heads[y * cells as usize + x];
                                while link != u16::MAX {
                                    let entry = bin_links[usize::from(link)];
                                    let index = usize::from(entry.triangle);
                                    candidates[index / 64] |= 1u64 << (index % 64);
                                    link = entry.next;
                                }
                            }
                        }
                    }
                    let mut overlaps = [0u64; 8];
                    for (word_index, word) in candidates.into_iter().enumerate() {
                        let mut remaining = word;
                        while remaining != 0 {
                            if cancelled() {
                                return Ok(None);
                            }
                            let bit = remaining.trailing_zeros() as usize;
                            remaining &= remaining - 1;
                            let triangle_index = word_index * 64 + bit;
                            if identity_topology {
                                overlaps[word_index] |= 1u64 << bit;
                                #[cfg(feature = "surface-profile")]
                                {
                                    transition.profile.identity_template_triangles += 1;
                                    transition.profile.triangle_pairs_considered += 1;
                                }
                                continue;
                            }
                            let triangle = mapped_new[triangle_index];
                            #[cfg(feature = "surface-profile")]
                            {
                                transition.profile.triangle_pairs_considered += 1;
                            }
                            // Exact integer endpoint bounding-box rejection after
                            // conservative closed-bin candidate collection.
                            if bbox_overlaps(old_bounds, triangle.bounds) {
                                if !reject_touching_pairs
                                    || positive_area_overlap(od, triangle.points)?
                                {
                                    overlaps[word_index] |= 1u64 << bit;
                                } else {
                                    #[cfg(feature = "surface-profile")]
                                    {
                                        transition.profile.bbox_rejected_pairs += 1;
                                    }
                                }
                            } else {
                                #[cfg(feature = "surface-profile")]
                                {
                                    transition.profile.bbox_rejected_pairs += 1;
                                }
                            }
                        }
                    }
                    #[cfg(feature = "surface-profile")]
                    {
                        transition.profile.pair_mapping_bbox += bbox_started.elapsed();
                    }
                    for (word_index, word) in overlaps.into_iter().enumerate() {
                        let mut remaining = word;
                        while remaining != 0 {
                            let bit = remaining.trailing_zeros() as usize;
                            remaining &= remaining - 1;
                            let triangle_index = word_index * 64 + bit;
                            let (nd, nt) = if identity_topology {
                                (od, ot)
                            } else {
                                let triangle = mapped_new[triangle_index];
                                (triangle.points, triangle.indices)
                            };
                            #[cfg(feature = "surface-profile")]
                            let (polygon, len) = if identity_topology {
                                let zero = [Rational::integer(0); 2];
                                let mut polygon = [zero; 8];
                                polygon[..3].copy_from_slice(&od);
                                (polygon, 3)
                            } else {
                                transition.profile.clipped_pairs += 1;
                                let clipping_started = Instant::now();
                                let result = overlay(od, nd)?;
                                transition.profile.exact_overlay_clipping +=
                                    clipping_started.elapsed();
                                result
                            };
                            #[cfg(not(feature = "surface-profile"))]
                            let (polygon, len) = if identity_topology {
                                let zero = [Rational::integer(0); 2];
                                let mut polygon = [zero; 8];
                                polygon[..3].copy_from_slice(&od);
                                (polygon, 3)
                            } else {
                                overlay(od, nd)?
                            };
                            for k in 1..len.saturating_sub(1) {
                                if cancelled() {
                                    return Ok(None);
                                }
                                let points = [polygon[0], polygon[k], polygon[k + 1]];
                                let area = oriented(points[0], points[1], points[2])?;
                                if area.n == 0 {
                                    continue;
                                }
                                if area.n < 0 {
                                    return Err(invalid());
                                }
                                let initial = TransitionVertex {
                                    old: old.patches()[oi].samples()[0],
                                    new: new.patches()[ni].samples()[0],
                                    old_elevation: 0.0,
                                    new_elevation: 0.0,
                                    old_reference: SurfaceTriangleReference {
                                        address: op.address,
                                        indices: ot,
                                        weights: [0.0; 3],
                                    },
                                    new_reference: SurfaceTriangleReference {
                                        address: np.address,
                                        indices: nt,
                                        weights: [0.0; 3],
                                    },
                                };
                                let mut vertices = [initial; 3];
                                for (vertex_index, point) in points.into_iter().enumerate() {
                                    if cancelled() {
                                        return Ok(None);
                                    }
                                    #[cfg(feature = "surface-profile")]
                                    let capture_started = Instant::now();
                                    let capture = |domain: [Point; 3],
                                                   indices: [u16; 3],
                                                   patch: &super::GeneratedSurfacePatch|
                                     -> Result<(
                                        SurfaceGeometrySample,
                                        SurfaceTriangleReference,
                                        f64,
                                    )> {
                                        let denominator =
                                            oriented(domain[0], domain[1], domain[2])?;
                                        let weights = [
                                            oriented(domain[1], domain[2], point)?
                                                .div(denominator)?,
                                            oriented(domain[2], domain[0], point)?
                                                .div(denominator)?,
                                            oriented(domain[0], domain[1], point)?
                                                .div(denominator)?,
                                        ];
                                        if weights.iter().any(|w| w.n < 0) {
                                            return Err(invalid());
                                        }
                                        let weights = weights.map(Rational::value);
                                        let samples =
                                            indices.map(|i| patch.samples()[usize::from(i)]);
                                        let sample = if let Some(i) =
                                            weights.iter().position(|&w| w == 1.0)
                                        {
                                            samples[i]
                                        } else {
                                            SurfaceGeometrySample {
                                                position_body_m: samples[0].position_body_m
                                                    * weights[0]
                                                    + samples[1].position_body_m * weights[1]
                                                    + samples[2].position_body_m * weights[2],
                                                normal_body: samples[0].normal_body * weights[0]
                                                    + samples[1].normal_body * weights[1]
                                                    + samples[2].normal_body * weights[2],
                                            }
                                        };
                                        let envelope = patch
                                            .extent()
                                            .min_height_m
                                            .abs()
                                            .max(patch.extent().max_height_m.abs());
                                        let elevation = if envelope == 0.0 {
                                            0.0
                                        } else {
                                            samples
                                                .iter()
                                                .zip(weights)
                                                .map(|(s, w)| {
                                                    ((s.position_body_m.length()
                                                        - patch.reference_radius_m())
                                                        / envelope)
                                                        .clamp(-1.0, 1.0)
                                                        * w
                                                })
                                                .sum()
                                        };
                                        Ok((
                                            sample,
                                            SurfaceTriangleReference {
                                                address: patch.address(),
                                                indices,
                                                weights,
                                            },
                                            elevation,
                                        ))
                                    };
                                    let (mut a, ar, ae) = capture(od, ot, &old.patches()[oi])?;
                                    let (mut b, br, be) = capture(nd, nt, &new.patches()[ni])?;
                                    #[cfg(feature = "surface-profile")]
                                    {
                                        transition.profile.barycentric_endpoint_capture +=
                                            capture_started.elapsed();
                                    }
                                    #[cfg(feature = "surface-profile")]
                                    let canonical_started = Instant::now();
                                    if point.iter().all(|p| p.d == 1) {
                                        let x = u32::try_from(point[0].n).map_err(|_| invalid())?;
                                        let y = u32::try_from(point[1].n).map_err(|_| invalid())?;
                                        // Canonicalize all dyadic overlay vertices, including
                                        // cube seams/corners and internal child boundaries.
                                        let key = base
                                            .sample_key(x, y, cells)
                                            .map_err(|_| invalid())?
                                            .compact_key();
                                        match boundary.binary_search_by_key(&key, |r| r.0) {
                                            Ok(index) => {
                                                a = boundary[index].1.0;
                                                b = boundary[index].1.1;
                                            }
                                            Err(index) => {
                                                if boundary.len() == boundary_limit {
                                                    return Err(
                                                        RenderPreparationError::InvalidBudget,
                                                    );
                                                }
                                                boundary.insert(index, (key, (a, b)));
                                                #[cfg(feature = "surface-profile")]
                                                {
                                                    transition.profile.boundary_vertices += 1;
                                                }
                                            }
                                        }
                                    }
                                    #[cfg(feature = "surface-profile")]
                                    {
                                        transition.profile.boundary_canonicalization +=
                                            canonical_started.elapsed();
                                    }
                                    if !a.position_body_m.is_finite()
                                        || !b.position_body_m.is_finite()
                                        || !a.normal_body.is_finite()
                                        || !b.normal_body.is_finite()
                                        || a.normal_body.length_squared() < 1e-20
                                        || b.normal_body.length_squared() < 1e-20
                                        || a.position_body_m.dot(a.normal_body) <= 0.0
                                        || b.position_body_m.dot(b.normal_body) <= 0.0
                                    {
                                        return Err(invalid());
                                    }
                                    transition.max_displacement_m = transition
                                        .max_displacement_m
                                        .max((a.position_body_m - b.position_body_m).length());
                                    vertices[vertex_index] = TransitionVertex {
                                        old: a,
                                        new: b,
                                        old_reference: ar,
                                        new_reference: br,
                                        old_elevation: ae,
                                        new_elevation: be,
                                    };
                                }
                                #[cfg(feature = "surface-profile")]
                                let emission_started = Instant::now();
                                if (transition.triangles.len() + 1)
                                    * size_of::<[TransitionVertex; 3]>()
                                    + size_of::<Self>()
                                    + (transition.affected_old.capacity()
                                        + transition.affected_new.capacity())
                                        * size_of::<CubePatchAddress>()
                                    + scratch_bytes
                                    > max_bytes
                                {
                                    return Err(RenderPreparationError::InvalidBudget);
                                }
                                // Grow geometrically only up to the preflighted
                                // reservation. Repeated 128-triangle reallocations
                                // copy an increasingly large exact overlay.
                                if transition.triangles.len() == transition.triangles.capacity() {
                                    let overhead = size_of::<Self>()
                                        + scratch_bytes
                                        + (transition.affected_old.capacity()
                                            + transition.affected_new.capacity())
                                            * size_of::<CubePatchAddress>();
                                    let limit = max_bytes.saturating_sub(overhead)
                                        / size_of::<[TransitionVertex; 3]>();
                                    let capacity = transition.triangles.capacity();
                                    let next = capacity
                                        .saturating_mul(2)
                                        .max(capacity.saturating_add(128))
                                        .min(limit);
                                    if next <= transition.triangles.len() {
                                        return Err(RenderPreparationError::InvalidBudget);
                                    }
                                    transition
                                        .triangles
                                        .reserve_exact(next - transition.triangles.len());
                                    if transition.resident_bytes().saturating_add(scratch_bytes)
                                        > max_bytes
                                    {
                                        return Err(RenderPreparationError::InvalidBudget);
                                    }
                                }
                                transition.triangles.push(vertices);
                                #[cfg(feature = "surface-profile")]
                                {
                                    transition.profile.allocation_triangle_emission +=
                                        emission_started.elapsed();
                                    transition.profile.emitted_triangles += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            transition.profile.validation_setup = if validation_finished {
                build_started.elapsed().saturating_sub(
                    transition.profile.pair_mapping_bbox
                        + transition.profile.exact_overlay_clipping
                        + transition.profile.barycentric_endpoint_capture
                        + transition.profile.boundary_canonicalization
                        + transition.profile.allocation_triangle_emission,
                )
            } else {
                build_started.elapsed()
            };
        }
        Ok(Some(transition))
    }
    pub fn triangles(&self) -> &[[TransitionVertex; 3]] {
        &self.triangles
    }
    #[cfg(feature = "surface-profile")]
    pub fn profile(&self) -> &SurfaceTransitionProfile {
        &self.profile
    }
    pub fn affected_old(&self) -> &[CubePatchAddress] {
        &self.affected_old
    }
    pub fn affected_new(&self) -> &[CubePatchAddress] {
        &self.affected_new
    }
    pub fn max_displacement_m(&self) -> f64 {
        self.max_displacement_m
    }
    pub fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + self.triangles.capacity() * size_of::<[TransitionVertex; 3]>()
            + (self.affected_old.capacity() + self.affected_new.capacity())
                * size_of::<CubePatchAddress>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planet_surface::{
        GeneratedSurfacePatch, SurfaceErrorContributions, SurfaceExtent, active_surface_cover,
    };
    use mundaris_math::surface::CubeFace;

    #[test]
    fn integer_separation_agrees_with_exact_clipping_for_every_stitch_mask() {
        let topology = SurfaceTopology::new();
        let address = CubePatchAddress::root(CubeFace::PositiveX);
        for old_mask in 0..16 {
            for new_mask in 0..16 {
                for &old_indices in topology.indices(old_mask).as_chunks::<3>().0 {
                    let old = old_indices.map(|index| {
                        let x = i128::from(index % 17);
                        let y = i128::from(index / 17);
                        [Rational::integer(x), Rational::integer(y)]
                    });
                    for &new_indices in topology.indices(new_mask).as_chunks::<3>().0 {
                        let new = new_indices.map(|index| {
                            let x = i128::from(index % 17);
                            let y = i128::from(index / 17);
                            [Rational::integer(x), Rational::integer(y)]
                        });
                        if !bbox_overlaps(bounds(old), bounds(new)) {
                            continue;
                        }
                        let (polygon, len) = overlay(old, new).unwrap();
                        let has_area = (1..len.saturating_sub(1)).any(|index| {
                            oriented(polygon[0], polygon[index], polygon[index + 1])
                                .unwrap()
                                .n
                                > 0
                        });
                        assert_eq!(
                            positive_area_overlap(old, new).unwrap(),
                            has_area,
                            "masks {old_mask}/{new_mask}, address {address:?}"
                        );
                    }
                }
            }
        }
    }

    fn root_surface(
        topology: &SurfaceTopology,
        elevation_m: f64,
    ) -> (Vec<ActiveSurfacePatch>, StitchedSurface) {
        let addresses: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        addressed_surface(topology, &addresses, elevation_m)
    }

    fn addressed_surface(
        topology: &SurfaceTopology,
        addresses: &[CubePatchAddress],
        elevation_m: f64,
    ) -> (Vec<ActiveSurfacePatch>, StitchedSurface) {
        let mut addresses = addresses.to_vec();
        addresses.sort_unstable();
        let cover = active_surface_cover(&addresses, topology).unwrap();
        let patches: Vec<_> = addresses
            .into_iter()
            .map(|address| {
                let samples = (0..289)
                    .map(|index| {
                        let direction = address
                            .sample_direction((index % 17) as u32, (index / 17) as u32, 16)
                            .unwrap()
                            .unit();
                        SurfaceGeometrySample {
                            position_body_m: direction * (1000.0 + elevation_m),
                            normal_body: direction,
                        }
                    })
                    .collect();
                GeneratedSurfacePatch::new(
                    address,
                    1000.0,
                    1.0,
                    samples,
                    SurfaceExtent {
                        min_height_m: -10.0,
                        max_height_m: 10.0,
                        guaranteed_opaque_radius_m: 0.0,
                    },
                    SurfaceErrorContributions::default(),
                )
                .unwrap()
            })
            .collect();
        let refs: Vec<_> = patches.iter().collect();
        (
            cover.clone(),
            StitchedSurface::build(&cover, &refs, topology).unwrap(),
        )
    }

    fn assert_same_transition(a: &SurfaceTransition, b: &SurfaceTransition) {
        assert_eq!(a.affected_old(), b.affected_old());
        assert_eq!(a.affected_new(), b.affected_new());
        assert_eq!(
            a.max_displacement_m().to_bits(),
            b.max_displacement_m().to_bits()
        );
        assert_eq!(a.triangles().len(), b.triangles().len());
        for (a, b) in a
            .triangles()
            .iter()
            .flatten()
            .zip(b.triangles().iter().flatten())
        {
            for t in [0.0, 0.125, 0.5, 0.875, 1.0] {
                let (a, b) = (a.sample(t).unwrap(), b.sample(t).unwrap());
                assert_eq!(
                    a.position_body_m.to_array().map(f64::to_bits),
                    b.position_body_m.to_array().map(f64::to_bits)
                );
                assert_eq!(
                    a.normal_body.to_array().map(f64::to_bits),
                    b.normal_body.to_array().map(f64::to_bits)
                );
            }
            for (a, b) in [
                (a.old_reference, b.old_reference),
                (a.new_reference, b.new_reference),
            ] {
                assert_eq!(a.address, b.address);
                assert_eq!(a.indices, b.indices);
                assert_eq!(a.weights.map(f64::to_bits), b.weights.map(f64::to_bits));
            }
            assert_eq!(a.old_elevation.to_bits(), b.old_elevation.to_bits());
            assert_eq!(a.new_elevation.to_bits(), b.new_elevation.to_bits());
        }
    }

    #[test]
    fn integer_rejection_preserves_exact_split_merge_endpoints_on_every_face() {
        let topology = SurfaceTopology::new();
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let (coarse_cover, coarse) = addressed_surface(&topology, &roots, 0.0);
        for face in CubeFace::ALL {
            let parent = CubePatchAddress::root(face);
            let fine_addresses: Vec<_> = roots
                .iter()
                .copied()
                .filter(|p| *p != parent)
                .chain(parent.children().unwrap())
                .collect();
            let (fine_cover, fine) = addressed_surface(&topology, &fine_addresses, 2.0);
            for (old_cover, old, new_cover, new) in [
                (&coarse_cover, &coarse, &fine_cover, &fine),
                (&fine_cover, &fine, &coarse_cover, &coarse),
            ] {
                let reference = SurfaceTransition::build_impl(
                    old_cover,
                    old,
                    new_cover,
                    new,
                    &topology,
                    16 * 1024 * 1024,
                    false,
                    false,
                    &mut || false,
                )
                .unwrap()
                .unwrap();
                let optimized = SurfaceTransition::build(
                    old_cover,
                    old,
                    new_cover,
                    new,
                    &topology,
                    16 * 1024 * 1024,
                )
                .unwrap();
                assert_same_transition(&optimized, &reference);
            }
        }
    }

    #[test]
    fn identical_topology_templates_match_full_exact_overlay() {
        let topology = SurfaceTopology::new();
        let (old_cover, old) = root_surface(&topology, 0.0);
        let (new_cover, new) = root_surface(&topology, 2.0);
        let reference = SurfaceTransition::build_impl(
            &old_cover,
            &old,
            &new_cover,
            &new,
            &topology,
            16 * 1024 * 1024,
            false,
            false,
            &mut || false,
        )
        .unwrap()
        .unwrap();
        let optimized = SurfaceTransition::build(
            &old_cover,
            &old,
            &new_cover,
            &new,
            &topology,
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(optimized.triangles().len(), reference.triangles().len());
        for (optimized, reference) in optimized
            .triangles()
            .iter()
            .flatten()
            .zip(reference.triangles().iter().flatten())
        {
            assert_eq!(optimized.old.position_body_m, reference.old.position_body_m);
            assert_eq!(optimized.new.position_body_m, reference.new.position_body_m);
            assert_eq!(optimized.old.normal_body, reference.old.normal_body);
            assert_eq!(optimized.new.normal_body, reference.new.normal_body);
            for (a, b) in [
                (optimized.old_reference, reference.old_reference),
                (optimized.new_reference, reference.new_reference),
            ] {
                assert_eq!(a.address, b.address);
                assert_eq!(a.indices, b.indices);
                assert_eq!(a.weights.map(f64::to_bits), b.weights.map(f64::to_bits));
            }
            assert_eq!(
                optimized.old_elevation.to_bits(),
                reference.old_elevation.to_bits()
            );
            assert_eq!(
                optimized.new_elevation.to_bits(),
                reference.new_elevation.to_bits()
            );
        }
        #[cfg(feature = "surface-profile")]
        {
            assert!(optimized.profile().identity_template_triangles > 0);
            assert_eq!(optimized.profile().clipped_pairs, 0);
            assert!(reference.profile().clipped_pairs > 0);
        }
    }
}
