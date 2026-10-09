//! Pure CDLOD quadtree geometry and selection over cube-sphere charts.
use glam::{DMat3, DVec3};
use mundaris_math::surface::{CubeFace, CubePatchAddress};

/// f64 cube-chart placement of one node; mirrors the GPU chart reconstruction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeChart {
    pub n0: DVec3,
    pub q0_length: f64,
    pub face_u: DVec3,
    pub face_v: DVec3,
    pub width: f64,
}

pub fn chart(address: CubePatchAddress) -> NodeChart {
    let [n, u, v] = address.face().basis();
    let scale = 2.0 / (1u64 << address.level()) as f64;
    let [x, y] = address.coordinates();
    let centre = [
        -1.0 + scale * (f64::from(x) + 0.5),
        -1.0 + scale * (f64::from(y) + 0.5),
    ];
    let q0 = n + u * centre[0] + v * centre[1];
    let length = q0.length();
    NodeChart {
        n0: q0 / length,
        q0_length: length,
        face_u: u,
        face_v: v,
        width: scale,
    }
}

impl NodeChart {
    pub fn direction(&self, st: [f64; 2]) -> DVec3 {
        (self.n0 * self.q0_length
            + self.face_u * ((st[0] - 0.5) * self.width)
            + self.face_v * ((st[1] - 0.5) * self.width))
            .normalize()
    }
}

/// Sub-rectangle of `node`'s chart inside ancestor `ancestor`'s chart.
pub fn rect_within(node: CubePatchAddress, ancestor: CubePatchAddress) -> ([f64; 2], f64) {
    let depth = node.level() - ancestor.level();
    let scale = 1.0 / (1u64 << depth) as f64;
    let [x, y] = node.coordinates();
    let [ax, ay] = ancestor.coordinates();
    (
        [
            f64::from(x - (ax << depth)) * scale,
            f64::from(y - (ay << depth)) * scale,
        ],
        scale,
    )
}

/// Conservative-by-sampling bounding sphere of a node whose radial offsets lie
/// in `[min_m, max_m]`. Samples the centre, corners and edge midpoints at both
/// radii and pads by 2 % to cover curvature between samples.
pub fn bounding_sphere(chart: &NodeChart, radius_m: f64, bounds: [f64; 2]) -> (DVec3, f64) {
    let mid = 0.5 * (bounds[0] + bounds[1]);
    let centre = chart.n0 * (radius_m + mid);
    let mut extent: f64 = 0.0;
    for s in [0.0, 0.5, 1.0] {
        for t in [0.0, 0.5, 1.0] {
            let direction = chart.direction([s, t]);
            for offset in bounds {
                extent = extent.max((direction * (radius_m + offset) - centre).length());
            }
        }
    }
    (centre, extent * 1.02 + 1.0e-3)
}

/// Largest angle between the chart centre and its corners or edge midpoints.
pub fn angular_radius(chart: &NodeChart) -> f64 {
    let mut angle: f64 = 0.0;
    for s in [0.0, 0.5, 1.0] {
        for t in [0.0, 0.5, 1.0] {
            angle = angle.max(
                chart
                    .n0
                    .dot(chart.direction([s, t]))
                    .clamp(-1.0, 1.0)
                    .acos(),
            );
        }
    }
    angle * 1.02
}

/// Symmetric view frustum in view axes (+X right, +Y up, -Z forward).
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    planes: [DVec3; 4],
    /// Accept every direction.
    all: bool,
    /// Cull against one sun shadow cascade's light-space box instead.
    cascade: Option<(mundaris_renderer::Cascades, usize)>,
}

impl Frustum {
    pub fn new(focal_px: f64, viewport: [f64; 2]) -> Self {
        let tx = 0.5 * viewport[0] / focal_px;
        let ty = 0.5 * viewport[1] / focal_px;
        let plane = |v: DVec3| v.normalize();
        Self {
            planes: [
                plane(DVec3::new(1.0, 0.0, tx)),
                plane(DVec3::new(-1.0, 0.0, tx)),
                plane(DVec3::new(0.0, 1.0, ty)),
                plane(DVec3::new(0.0, -1.0, ty)),
            ],
            all: false,
            cascade: None,
        }
    }

    /// Frustum that culls nothing.
    pub fn everything() -> Self {
        Self {
            planes: [DVec3::ZERO; 4],
            all: true,
            cascade: None,
        }
    }

    /// Casters of one shadow cascade: inside its light-space box, including
    /// off-screen terrain between receivers and the sun.
    pub fn cascade(cascades: mundaris_renderer::Cascades, index: usize) -> Self {
        Self {
            planes: [DVec3::ZERO; 4],
            all: false,
            cascade: Some((cascades, index)),
        }
    }

    pub fn intersects(&self, centre_view: DVec3, radius: f64) -> bool {
        if self.all {
            return true;
        }
        if let Some((cascades, index)) = &self.cascade {
            return cascades.may_cast(*index, centre_view, radius);
        }
        if centre_view.z - radius > 0.0 {
            return false;
        }
        self.planes
            .iter()
            .all(|plane| plane.dot(centre_view) <= radius)
    }
}

/// Inputs shared by one body's selection.
pub struct SelectionInput<'a> {
    pub observer_body: DVec3,
    pub body_to_view: DMat3,
    pub frustum: Frustum,
    pub radius_m: f64,
    /// Radius of a sphere that terrain never dips below; occludes nodes
    /// entirely behind its horizon.
    pub occluder_radius_m: f64,
    /// Distance below which a node of level L must be refined.
    pub ranges: &'a [f64],
    pub max_level: u8,
    pub visit_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selected {
    pub address: CubePatchAddress,
    pub distance_m: f64,
}

#[derive(Debug, Default, Clone)]
pub struct SelectionResult {
    pub selected: Vec<Selected>,
    pub visited: usize,
    /// The visit limit stopped wanted refinement.
    pub truncated: bool,
    /// Wanted refinements waiting for measured height bounds.
    pub deferred: usize,
    pub horizon_or_frustum_culled: usize,
}

/// Conservative horizon test: true only when the whole bounding sphere lies
/// behind the horizon of the occluder sphere as seen from `observer`.
/// The node is a cap of directions within `angular_radius` of `axis` whose
/// points never exceed radius `highest`.
pub fn beyond_horizon(
    observer: DVec3,
    occluder_radius_m: f64,
    axis: DVec3,
    angular_radius: f64,
    highest: f64,
) -> bool {
    let d = observer.length();
    if !occluder_radius_m.is_finite()
        || occluder_radius_m <= 0.0
        || d <= occluder_radius_m
        || highest <= 0.0
    {
        return false;
    }
    let theta = (observer.dot(axis) / (d * axis.length()))
        .clamp(-1.0, 1.0)
        .acos();
    let alpha = angular_radius;
    let limit = (occluder_radius_m / d).acos() + (occluder_radius_m / highest).min(1.0).acos();
    theta - alpha > limit
}

/// CDLOD selection: refine while the node's bounding sphere is closer than
/// its level range; frustum-culled nodes are neither refined nor drawn.
pub fn select(
    input: &SelectionInput<'_>,
    mut bounds: impl FnMut(CubePatchAddress) -> ([f64; 2], bool),
) -> SelectionResult {
    let mut result = SelectionResult::default();
    let mut stack: Vec<CubePatchAddress> = CubeFace::ALL
        .iter()
        .rev()
        .map(|face| CubePatchAddress::root(*face))
        .collect();
    while let Some(node) = stack.pop() {
        result.visited += 1;
        let chart = chart(node);
        let (node_bounds, refinable) = bounds(node);
        let (centre, extent) = bounding_sphere(&chart, input.radius_m, node_bounds);
        let relative = centre - input.observer_body;
        if !input
            .frustum
            .intersects(input.body_to_view * relative, extent)
            || beyond_horizon(
                input.observer_body,
                input.occluder_radius_m,
                chart.n0,
                angular_radius(&chart),
                input.radius_m + node_bounds[1],
            )
        {
            result.horizon_or_frustum_culled += 1;
            continue;
        }
        let distance = (relative.length() - extent).max(0.0);
        let level = node.level();
        let wanted = level < input.max_level && distance < input.ranges[level as usize];
        let refine = wanted && refinable && result.visited + stack.len() + 4 <= input.visit_limit;
        if wanted && !refine {
            if refinable {
                result.truncated = true;
            } else {
                result.deferred += 1;
            }
        }
        if refine {
            let children = node
                .children()
                .expect("level below the representable maximum");
            stack.extend(children.into_iter().rev());
        } else {
            result.selected.push(Selected {
                address: node,
                distance_m: distance,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(radius: f64, cells: u32, max_level: u8) -> Vec<f64> {
        (0..=max_level)
            .map(|level| {
                let cell = mundaris_world::terrain::producer::tile_texel_m(radius, level, cells);
                (cell * 700.0 / 4.0).max(3.0 * cell * f64::from(cells))
            })
            .collect()
    }

    #[test]
    fn chart_corners_match_cube_face_directions() {
        let address = CubePatchAddress::try_new(CubeFace::PositiveZ, 3, 2, 5).unwrap();
        let chart = chart(address);
        for st in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.3, 0.7]] {
            let uv = address.face_uv(st).unwrap();
            let expected = address.face().direction(uv).unwrap().unit();
            assert!((chart.direction(st) - expected).length() < 1e-14);
        }
    }

    #[test]
    fn horizon_culls_only_fully_hidden_spheres() {
        let radius = 100_000.0;
        let observer = DVec3::Z * (radius + 2.0);
        // Just ahead along the surface: visible.
        let near = DVec3::new(100.0, 0.0, radius).normalize() * radius;
        assert!(!beyond_horizon(
            observer,
            radius - 1.0,
            near,
            1.0e-5,
            radius + 1.0
        ));
        // 5 km away along the surface (0.05 rad): hidden at ground level.
        let direction = DVec3::new(0.05f64.sin(), 0.0, 0.05f64.cos());
        assert!(beyond_horizon(
            observer,
            radius - 1.0,
            direction,
            1.0e-4,
            radius + 10.0
        ));
        // A 500 m tall feature there pokes above the horizon.
        assert!(!beyond_horizon(
            observer,
            radius - 1.0,
            direction,
            1.0e-4,
            radius + 500.0
        ));
        let far = DVec3::X * radius;
        // Observer below the occluder never culls.
        assert!(!beyond_horizon(
            DVec3::Z * radius * 0.5,
            radius,
            far,
            1.0e-4,
            radius
        ));
    }

    #[test]
    fn rect_within_maps_descendants_into_ancestor_charts() {
        let root = CubePatchAddress::root(CubeFace::NegativeY);
        let child = root.children().unwrap()[3];
        let grandchild = child.children().unwrap()[0];
        assert_eq!(rect_within(child, root), ([0.5, 0.5], 0.5));
        assert_eq!(rect_within(grandchild, root), ([0.5, 0.5], 0.25));
        assert_eq!(rect_within(grandchild, grandchild), ([0.0, 0.0], 1.0));
    }

    #[test]
    fn selection_is_balanced_and_covers_the_view_near_the_ground() {
        let radius = 109_081.776_801_130_12;
        let observer = DVec3::new(2001.4, 1250.9, 109_158.1).normalize() * (radius + 2.0);
        // Camera looking along the surface towards +X from above +Z.
        let forward = DVec3::X;
        let up = observer.normalize();
        let right = forward.cross(up).normalize();
        let forward = up.cross(right);
        let view_to_body = DMat3::from_cols(right, up, -forward);
        let ranges = ranges(radius, 32, 22);
        let input = SelectionInput {
            observer_body: observer,
            body_to_view: view_to_body.transpose(),
            frustum: Frustum::new(700.0, [660.0, 726.0]),
            radius_m: radius,
            occluder_radius_m: radius - 0.2,
            ranges: &ranges,
            max_level: 22,
            visit_limit: 100_000,
        };
        let result = select(&input, |_| ([-0.2, 0.2], true));
        assert_eq!(result.deferred, 0);
        assert!(!result.truncated);
        assert!(!result.selected.is_empty());
        let finest = result
            .selected
            .iter()
            .map(|s| s.address.level())
            .max()
            .unwrap();
        assert!(finest >= 18, "finest level {finest}");
        // 2:1 balance between selected edge neighbours.
        let levels: std::collections::HashMap<_, _> = result
            .selected
            .iter()
            .map(|s| (s.address, s.address.level()))
            .collect();
        for selected in &result.selected {
            for edge in mundaris_math::surface::PatchEdge::ALL {
                let mut neighbour = selected.address.neighbor(edge).address;
                // Find the selected node covering the neighbour region, if any.
                loop {
                    if let Some(level) = levels.get(&neighbour) {
                        assert!(selected.address.level().abs_diff(*level) <= 1);
                        break;
                    }
                    match neighbour.parent() {
                        Some(parent) => neighbour = parent,
                        None => break,
                    }
                }
            }
        }
        let mut histogram = [0usize; 23];
        for s in &result.selected {
            histogram[s.address.level() as usize] += 1;
        }
        eprintln!("selected per level: {histogram:?}");
        assert!(result.selected.len() < 2000, "{}", result.selected.len());
    }
}
