//! Anti-aliased pixel-width guide lines, prepared only after f64 source centering.
//!
//! Each connected run of a polyline is expanded on the CPU into screen-space
//! quads with mitred joins, so adjacent segments share edges and never blend
//! twice. Quads are widened by one pixel and carry their signed edge distance;
//! the fragment shader turns that into analytic coverage.
use crate::{CelestialProjection, PreparedView, RenderPreparationError};
use glam::DVec3;
use astrum_math::FramePosition;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CelestialLineStyle {
    Solid,
    Dashed,
}
pub struct CelestialPolyline<'a> {
    pub points: &'a [FramePosition],
    pub colors: &'a [[f32; 4]],
    pub width_pixels: f32,
    pub style: CelestialLineStyle,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct PolylinePreparationReport {
    pub segments: usize,
    pub vertices: usize,
    pub max_projected_error_pixels: f64,
}

/// Session overlay style applied while preparing lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineStyleScale {
    pub width: f32,
    pub opacity: f32,
}
impl Default for LineStyleScale {
    fn default() -> Self {
        Self {
            width: 1.0,
            opacity: 1.0,
        }
    }
}

/// Bytes per line vertex: clip position, colour, edge (signed px, half width px).
pub(crate) const LINE_VERTEX_BYTES: usize = 48;
const DASH_ON_PX: f64 = 6.0;
const DASH_PERIOD_PX: f64 = 10.0;
/// Mitre length limit in half widths; sharper corners fall back to a bevel.
const MITER_LIMIT: f64 = 2.5;

pub(crate) fn prepare_polylines(
    view: &PreparedView<'_>,
    projection: CelestialProjection,
    lines: &[CelestialPolyline<'_>],
    style: LineStyleScale,
    bytes: &mut Vec<u8>,
) -> Result<PolylinePreparationReport, RenderPreparationError> {
    let mut report = PolylinePreparationReport::default();
    for line in lines {
        if line.points.len() != line.colors.len()
            || !line.width_pixels.is_finite()
            || line.width_pixels <= 0.0
            || line.width_pixels > 64.0
            || !line.colors.iter().flatten().all(|c| c.is_finite())
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let Some(first) = line.points.first() else {
            continue;
        };
        let source = view.prepare_source(first.frame())?;
        // Even singleton and invisible data are validated; invalid sources cannot be hidden.
        let points = line
            .points
            .iter()
            .map(|p| Ok(source.view_displacement(*p)?.metres()))
            .collect::<Result<Vec<_>, RenderPreparationError>>()?;
        emit_polyline(
            projection,
            &points,
            line.colors,
            line.width_pixels * style.width,
            style.opacity,
            line.style,
            bytes,
            &mut report,
        )?;
    }
    Ok(report)
}

/// One screen-space point of a visible run.
#[derive(Clone, Copy)]
struct RunPoint {
    px: [f64; 2],
    depth: f64,
    color: [f32; 4],
}

/// Expands view-space points into anti-aliased quads.
#[allow(clippy::too_many_arguments)] // One flat expansion step.
pub(crate) fn emit_polyline(
    projection: CelestialProjection,
    points: &[DVec3],
    colors: &[[f32; 4]],
    width_pixels: f32,
    opacity: f32,
    style: CelestialLineStyle,
    bytes: &mut Vec<u8>,
    report: &mut PolylinePreparationReport,
) -> Result<(), RenderPreparationError> {
    if !width_pixels.is_finite() || width_pixels <= 0.0 || !opacity.is_finite() {
        return Err(RenderPreparationError::InvalidDebugGeometry);
    }
    let [w, h] = projection.viewport();
    let half = [f64::from(w) * 0.5, f64::from(h) * 0.5];
    let mut run: Vec<RunPoint> = Vec::new();
    let mut runs: Vec<Vec<RunPoint>> = Vec::new();
    let mut run_open_end = false;
    for i in 1..points.len() {
        let (p0, p1) = (points[i - 1], points[i]);
        let Some([a, b]) = projection.clip_segment([p0, p1])? else {
            if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
            run_open_end = false;
            continue;
        };
        let length = (p1 - p0).length();
        let t_of = |q: DVec3| {
            if length > 0.0 {
                ((q - p0).length() / length).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let lerp = |t: f64| -> [f32; 4] {
            std::array::from_fn(|k| colors[i - 1][k] + (colors[i][k] - colors[i - 1][k]) * t as f32)
        };
        let mut point = |q: DVec3, t: f64| -> Result<RunPoint, RenderPreparationError> {
            let (narrowed, _, error) = projection.narrow(q, f64::MAX, f64::MAX)?;
            report.max_projected_error_pixels = report.max_projected_error_pixels.max(error);
            let clip = projection.gpu_clip(narrowed);
            let ndc = [
                f64::from(clip[0]) / f64::from(clip[3]),
                f64::from(clip[1]) / f64::from(clip[3]),
            ];
            let mut color = lerp(t);
            color[3] *= opacity;
            Ok(RunPoint {
                px: [ndc[0] * half[0], ndc[1] * half[1]],
                depth: projection.near_m() / (-q.z).max(projection.near_m()),
                color,
            })
        };
        let start_clipped = (a - p0).length() > 1e-9 * (1.0 + p0.length());
        let end_clipped = (b - p1).length() > 1e-9 * (1.0 + p1.length());
        if start_clipped || !run_open_end {
            if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
            run.push(point(a, t_of(a))?);
        }
        run.push(point(b, t_of(b))?);
        report.segments += 1;
        run_open_end = !end_clipped;
        if end_clipped {
            runs.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    let half_width = f64::from(width_pixels) * 0.5;
    // Quads extend one pixel beyond the nominal edge for coverage falloff.
    let extent = half_width + 1.0;
    for run in runs.iter().filter(|r| r.len() >= 2) {
        let normals: Vec<Option<[f64; 2]>> = run
            .windows(2)
            .map(|pair| {
                let d = [pair[1].px[0] - pair[0].px[0], pair[1].px[1] - pair[0].px[1]];
                let length = d[0].hypot(d[1]);
                (length > 1e-9).then(|| [-d[1] / length, d[0] / length])
            })
            .collect();
        let mitred = style == CelestialLineStyle::Solid;
        // Offset direction (scaled for mitre length) at each run point.
        let offset_at = |index: usize, segment: usize| -> [f64; 2] {
            let own = normals[segment].unwrap_or([0.0, 0.0]);
            if !mitred {
                return own;
            }
            let neighbour = if index == segment {
                segment.checked_sub(1).and_then(|s| normals[s])
            } else {
                normals.get(segment + 1).copied().flatten()
            };
            let Some(other) = neighbour else {
                return own;
            };
            let sum = [own[0] + other[0], own[1] + other[1]];
            let length = sum[0].hypot(sum[1]);
            if length < 1e-6 {
                return own;
            }
            let m = [sum[0] / length, sum[1] / length];
            let cos = m[0] * own[0] + m[1] * own[1];
            if cos <= 1.0 / MITER_LIMIT {
                return own;
            }
            [m[0] / cos, m[1] / cos]
        };
        let mut phase = 0.0_f64;
        for segment in 0..run.len() - 1 {
            if normals[segment].is_none() {
                continue;
            }
            let (a, b) = (run[segment], run[segment + 1]);
            let length = (b.px[0] - a.px[0]).hypot(b.px[1] - a.px[1]);
            let offsets = [offset_at(segment, segment), offset_at(segment + 1, segment)];
            let mut at = 0.0;
            while at < length {
                let (finish, visible) = match style {
                    CelestialLineStyle::Solid => (length, true),
                    CelestialLineStyle::Dashed => {
                        let cycle = (phase + at).rem_euclid(DASH_PERIOD_PX);
                        (
                            (at + if cycle < DASH_ON_PX {
                                DASH_ON_PX - cycle
                            } else {
                                DASH_PERIOD_PX - cycle
                            })
                            .min(length),
                            cycle < DASH_ON_PX,
                        )
                    }
                };
                if visible && finish > at {
                    let ends = [at / length, finish / length].map(|t| {
                        let mix = |x: f64, y: f64| x + (y - x) * t;
                        let color: [f32; 4] = std::array::from_fn(|k| {
                            a.color[k] + (b.color[k] - a.color[k]) * t as f32
                        });
                        (
                            [mix(a.px[0], b.px[0]), mix(a.px[1], b.px[1])],
                            mix(a.depth, b.depth),
                            color,
                        )
                    });
                    // Interior dash ends use the plain segment normal.
                    let end_offsets = [
                        if at == 0.0 {
                            offsets[0]
                        } else {
                            normals[segment].unwrap()
                        },
                        if finish == length {
                            offsets[1]
                        } else {
                            normals[segment].unwrap()
                        },
                    ];
                    for (end, sign) in [
                        (0, -1.0),
                        (0, 1.0),
                        (1, 1.0),
                        (0, -1.0),
                        (1, 1.0),
                        (1, -1.0),
                    ] {
                        let (px, depth, color) = ends[end];
                        let o = end_offsets[end];
                        let values = [
                            ((px[0] + sign * o[0] * extent) / half[0]) as f32,
                            ((px[1] + sign * o[1] * extent) / half[1]) as f32,
                            depth as f32,
                            1.0,
                        ];
                        if !values.iter().all(|v| v.is_finite()) {
                            return Err(RenderPreparationError::InvalidDebugGeometry);
                        }
                        let edge = [(sign * extent) as f32, half_width as f32, 0.0, 0.0];
                        for value in values.into_iter().chain(color).chain(edge) {
                            bytes.extend_from_slice(&value.to_le_bytes());
                        }
                    }
                    report.vertices += 6;
                }
                if finish <= at {
                    break;
                }
                at = finish;
            }
            phase = (phase + length).rem_euclid(DASH_PERIOD_PX);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrum_math::*;
    use std::num::NonZeroU64;

    fn read(bytes: &[u8], vertex: usize, float: usize) -> f32 {
        let at = vertex * LINE_VERTEX_BYTES + float * 4;
        f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn quads_have_pixel_width_aa_margin_and_shared_mitre_edges() {
        let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(root, LocalPosition::origin()),
                UnitRotation::identity(),
            ),
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let points = [
            DVec3::new(-10.0, 0.0, -100.0),
            DVec3::new(10.0, 0.0, -100.0),
            DVec3::new(10.0, 10.0, -100.0),
        ]
        .map(|p| FramePosition::new(root, LocalPosition::try_metres(p).unwrap()));
        for (w, h) in [(1280, 800), (2560, 1600)] {
            let projection = CelestialProjection::try_new(w, h, 1.0, 0.1).unwrap();
            let mut bytes = Vec::new();
            let report = prepare_polylines(
                &view,
                projection,
                &[CelestialPolyline {
                    points: &points,
                    colors: &[[1.0, 0.0, 0.0, 0.15], [1.0; 4], [1.0; 4]],
                    width_pixels: 2.5,
                    style: CelestialLineStyle::Solid,
                }],
                LineStyleScale::default(),
                &mut bytes,
            )
            .unwrap();
            assert_eq!(report.vertices, 12);
            assert_eq!(bytes.len(), 12 * LINE_VERTEX_BYTES);
            // First segment is horizontal: its start cap spans width + 2 px.
            let y0 = read(&bytes, 0, 1);
            let y1 = read(&bytes, 1, 1);
            assert!(((y1 - y0) as f64 * h as f64 * 0.5 - (2.5 + 2.0)).abs() < 1e-3);
            assert_eq!(read(&bytes, 0, 9), 1.25);
            // The corner is shared: end of segment 1 equals start of segment 2.
            for k in 0..2 {
                assert!((read(&bytes, 2, k) - read(&bytes, 7, k)).abs() < 1e-6);
                assert!((read(&bytes, 5, k) - read(&bytes, 6, k)).abs() < 1e-6);
            }
            let mut dashed = Vec::new();
            assert!(
                prepare_polylines(
                    &view,
                    projection,
                    &[CelestialPolyline {
                        points: &points[..2],
                        colors: &[[1.0; 4]; 2],
                        width_pixels: 1.0,
                        style: CelestialLineStyle::Dashed
                    }],
                    LineStyleScale::default(),
                    &mut dashed
                )
                .unwrap()
                .vertices
                    > 6
            );
        }
        let module =
            naga::front::wgsl::parse_str(include_str!("shaders/celestial_lines.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn near_plane_clipping_splits_runs() {
        let projection = CelestialProjection::try_new(800, 600, 1.0, 0.1).unwrap();
        let points = [
            DVec3::new(-5.0, 0.0, -50.0),
            DVec3::new(0.0, 0.0, 10.0),
            DVec3::new(5.0, 0.0, -50.0),
        ];
        let mut bytes = Vec::new();
        let mut report = PolylinePreparationReport::default();
        emit_polyline(
            projection,
            &points,
            &[[1.0; 4]; 3],
            2.0,
            1.0,
            CelestialLineStyle::Solid,
            &mut bytes,
            &mut report,
        )
        .unwrap();
        // Both segments are clipped at the near plane: two separate runs.
        assert_eq!(report.segments, 2);
        assert_eq!(report.vertices, 12);
        assert!((0..12).all(|v| read(&bytes, v, 2) <= 1.0));
    }
}
