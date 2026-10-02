//! Portable pixel-width solid/dashed quads, prepared only after f64 source centering.
use crate::{CelestialProjection, PreparedView, RenderPreparationError};
use mundaris_math::FramePosition;
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
/// Output uses 32-byte clip-position/color vertices. Segment caps are butt caps;
/// adjacent quads overlap at corners, with no depth writes and standard alpha blending.
pub(crate) fn prepare_polylines(
    view: &PreparedView<'_>,
    projection: CelestialProjection,
    lines: &[CelestialPolyline<'_>],
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
        let mut previous = source.view_displacement(*first)?.metres();
        let mut phase = 0.0_f64;
        for i in 1..line.points.len() {
            let current = source.view_displacement(line.points[i])?.metres();
            if let Some([a, b]) = projection.clip_segment([previous, current])? {
                let mut clips = [[0.0; 4]; 2];
                let mut screens = [[0.0; 2]; 2];
                for (j, p) in [a, b].into_iter().enumerate() {
                    let (narrowed, _, error) = projection.narrow(p, f64::MAX, f64::MAX)?;
                    report.max_projected_error_pixels =
                        report.max_projected_error_pixels.max(error);
                    let clip = projection.gpu_clip(narrowed);
                    let ndc = [
                        clip[0] as f64 / clip[3] as f64,
                        clip[1] as f64 / clip[3] as f64,
                        projection.near_m() / (-p.z).max(projection.near_m()),
                        1.0,
                    ];
                    clips[j] = ndc;
                    let [w, h] = projection.viewport();
                    screens[j] = [ndc[0] * w as f64 * 0.5, ndc[1] * h as f64 * 0.5];
                }
                let dx = screens[1][0] - screens[0][0];
                let dy = screens[1][1] - screens[0][1];
                let length = dx.hypot(dy);
                if length > 1e-9 {
                    let [w, h] = projection.viewport();
                    let offset = [
                        -dy / length * line.width_pixels as f64 / w as f64,
                        dx / length * line.width_pixels as f64 / h as f64,
                    ];
                    let mut at = 0.0;
                    while at < length {
                        let (finish, visible) = match line.style {
                            CelestialLineStyle::Solid => (length, true),
                            CelestialLineStyle::Dashed => {
                                let cycle = (phase + at).rem_euclid(10.0);
                                (
                                    (at + if cycle < 6.0 {
                                        6.0 - cycle
                                    } else {
                                        10.0 - cycle
                                    })
                                    .min(length),
                                    cycle < 6.0,
                                )
                            }
                        };
                        if visible && finish > at {
                            let endpoints = [at / length, finish / length].map(|t| {
                                std::array::from_fn::<_, 4, _>(|axis| {
                                    clips[0][axis] + (clips[1][axis] - clips[0][axis]) * t
                                })
                            });
                            let colors = [at / length, finish / length].map(|t| {
                                std::array::from_fn::<_, 4, _>(|axis| {
                                    line.colors[i - 1][axis]
                                        + (line.colors[i][axis] - line.colors[i - 1][axis])
                                            * t as f32
                                })
                            });
                            for (endpoint, sign) in [
                                (0, -1.0),
                                (0, 1.0),
                                (1, 1.0),
                                (0, -1.0),
                                (1, 1.0),
                                (1, -1.0),
                            ] {
                                let p = endpoints[endpoint];
                                let values = [
                                    (p[0] + sign * offset[0]) as f32,
                                    (p[1] + sign * offset[1]) as f32,
                                    p[2] as f32,
                                    1.0,
                                ];
                                if !values.iter().all(|v| v.is_finite()) {
                                    return Err(RenderPreparationError::InvalidDebugGeometry);
                                }
                                for value in values.into_iter().chain(colors[endpoint]) {
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
                    report.segments += 1;
                    phase = (phase + length).rem_euclid(10.0);
                }
            }
            previous = current;
        }
    }
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use mundaris_math::*;
    use std::num::NonZeroU64;
    #[test]
    fn quads_physical_width_dashes_age_and_shader_contract() {
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
                    colors: &[[1.0, 0.0, 0.0, 0.15], [1.0; 4]],
                    width_pixels: 2.5,
                    style: CelestialLineStyle::Solid,
                }],
                &mut bytes,
            )
            .unwrap();
            assert_eq!(report.vertices, 6);
            assert_eq!(bytes.len(), 192);
            let y0 = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
            let y1 = f32::from_le_bytes(bytes[36..40].try_into().unwrap());
            assert!(((y1 - y0) as f64 * h as f64 * 0.5 - 2.5).abs() < 1e-5);
            let mut dashed = Vec::new();
            assert!(
                prepare_polylines(
                    &view,
                    projection,
                    &[CelestialPolyline {
                        points: &points,
                        colors: &[[1.0; 4]; 2],
                        width_pixels: 1.0,
                        style: CelestialLineStyle::Dashed
                    }],
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
}
