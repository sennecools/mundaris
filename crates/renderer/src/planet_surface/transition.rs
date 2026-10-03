//! Exact local triangle overlays of two valid stitched surfaces.
use super::{ActiveSurfacePatch, StitchedSurface, SurfaceGeometrySample, SurfaceTopology};
use crate::RenderPreparationError;
use mundaris_math::surface::CubePatchAddress;
use std::mem::size_of;

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
fn subtract(a: Point, b: Point) -> Result<Point> {
    Ok([a[0].sub(b[0])?, a[1].sub(b[1])?])
}
fn cross(a: Point, b: Point) -> Result<Rational> {
    a[0].mul(b[1])?.sub(a[1].mul(b[0])?)
}
fn oriented(a: Point, b: Point, c: Point) -> Result<Rational> {
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
        let boundary_limit = count.saturating_mul(64 * 4);
        let scratch_bytes = boundary_limit
            .saturating_mul(size_of::<(
                u128,
                (SurfaceGeometrySample, SurfaceGeometrySample),
            )>())
            .saturating_add(16 * 1024);
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
        };
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
            for (ni, np) in new_cover.iter().enumerate() {
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
                for &ot in topology.indices(op.stitch_mask).as_chunks::<3>().0 {
                    let od = mapped(op.address, ot);
                    for &nt in topology.indices(np.stitch_mask).as_chunks::<3>().0 {
                        let nd = mapped(np.address, nt);
                        // Exact integer endpoint bounding-box rejection.
                        if (0..2).any(|a| {
                            od.iter().map(|p| p[a].n).max() < nd.iter().map(|p| p[a].n).min()
                                || nd.iter().map(|p| p[a].n).max() < od.iter().map(|p| p[a].n).min()
                        }) {
                            continue;
                        }
                        let (polygon, len) = overlay(od, nd)?;
                        for k in 1..len.saturating_sub(1) {
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
                                let capture = |domain: [Point; 3],
                                               indices: [u16; 3],
                                               patch: &super::GeneratedSurfacePatch|
                                 -> Result<(
                                    SurfaceGeometrySample,
                                    SurfaceTriangleReference,
                                    f64,
                                )> {
                                    let denominator = oriented(domain[0], domain[1], domain[2])?;
                                    let weights = [
                                        oriented(domain[1], domain[2], point)?.div(denominator)?,
                                        oriented(domain[2], domain[0], point)?.div(denominator)?,
                                        oriented(domain[0], domain[1], point)?.div(denominator)?,
                                    ];
                                    if weights.iter().any(|w| w.n < 0) {
                                        return Err(invalid());
                                    }
                                    let weights = weights.map(Rational::value);
                                    let samples = indices.map(|i| patch.samples()[usize::from(i)]);
                                    let sample =
                                        if let Some(i) = weights.iter().position(|&w| w == 1.0) {
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
                                                return Err(RenderPreparationError::InvalidBudget);
                                            }
                                            boundary.insert(index, (key, (a, b)));
                                        }
                                    }
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
                            if (transition.triangles.len() + 1) * size_of::<[TransitionVertex; 3]>()
                                + size_of::<Self>()
                                + (transition.affected_old.capacity()
                                    + transition.affected_new.capacity())
                                    * size_of::<CubePatchAddress>()
                                + scratch_bytes
                                > max_bytes
                            {
                                return Err(RenderPreparationError::InvalidBudget);
                            }
                            // Exact growth is essential: Vec's geometric doubling
                            // must not silently consume the transition reservation.
                            if transition.triangles.len() == transition.triangles.capacity() {
                                let overhead = size_of::<Self>()
                                    + scratch_bytes
                                    + (transition.affected_old.capacity()
                                        + transition.affected_new.capacity())
                                        * size_of::<CubePatchAddress>();
                                let limit = max_bytes.saturating_sub(overhead)
                                    / size_of::<[TransitionVertex; 3]>();
                                let next = (transition.triangles.capacity() + 128).min(limit);
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
                        }
                    }
                }
            }
        }
        Ok(transition)
    }
    pub fn triangles(&self) -> &[[TransitionVertex; 3]] {
        &self.triangles
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
