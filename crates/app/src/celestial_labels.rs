//! Bounded deterministic screen-space label placement. Coordinates are physical pixels.
use mundaris_world::BodyId;
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub min: [f64; 2],
    pub max: [f64; 2],
}
impl ScreenRect {
    pub fn contains(self, p: [f64; 2]) -> bool {
        (0..2).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
    pub fn intersects(self, other: Self) -> bool {
        (0..2).all(|i| self.min[i] < other.max[i] && other.min[i] < self.max[i])
    }
    pub fn contains_rect(self, r: Self) -> bool {
        self.contains(r.min) && self.contains(r.max)
    }
}
#[derive(Debug, Clone, Copy)]
pub struct LabelInput {
    pub body: BodyId,
    pub marker: [f64; 2],
    pub size: [f64; 2],
    pub selected: bool,
    pub focused: bool,
    pub hovered: bool,
    pub diameter: f64,
    pub distance_m: f64,
}
#[derive(Debug, Clone, Copy)]
pub struct PlacedLabel {
    pub body: BodyId,
    pub rect: ScreenRect,
    pub marker: [f64; 2],
    pub slot: usize,
    pub leader: bool,
}
#[derive(Default)]
pub struct LabelLayout {
    order: Vec<usize>,
    previous: Vec<PlacedLabel>,
}
impl LabelLayout {
    pub fn layout(
        &mut self,
        inputs: &[LabelInput],
        viewport: ScreenRect,
        protected: &[ScreenRect],
        output: &mut Vec<PlacedLabel>,
    ) {
        self.order.clear();
        self.order.extend(0..inputs.len());
        output.clear();
        self.order.sort_by(|&a, &b| {
            let (a, b) = (&inputs[a], &inputs[b]);
            (b.selected || b.focused)
                .cmp(&(a.selected || a.focused))
                .then(b.hovered.cmp(&a.hovered))
                .then(b.diameter.total_cmp(&a.diameter))
                .then(a.distance_m.total_cmp(&b.distance_m))
        });
        for &index in &self.order {
            let input = inputs[index];
            let old = self
                .previous
                .iter()
                .find(|l| l.body == input.body)
                .map(|l| l.slot);
            let slots = old.into_iter().chain((0..24).filter(|s| Some(*s) != old));
            for slot in slots {
                let ring = slot / 8;
                let gap = 12.0 + ring as f64 * 24.0;
                let [w, h] = input.size;
                let [x, y] = input.marker;
                let min = match slot % 8 {
                    0 => [x + gap, y - h * 0.5],
                    1 => [x + gap, y - gap - h],
                    2 => [x - w * 0.5, y - gap - h],
                    3 => [x - gap - w, y - gap - h],
                    4 => [x - gap - w, y - h * 0.5],
                    5 => [x - gap - w, y + gap],
                    6 => [x - w * 0.5, y + gap],
                    _ => [x + gap, y + gap],
                };
                let rect = ScreenRect {
                    min,
                    max: [min[0] + w, min[1] + h],
                };
                if !viewport.contains_rect(rect)
                    || protected.iter().any(|p| p.intersects(rect))
                    || output.iter().any(|l| l.rect.intersects(rect))
                {
                    continue;
                }
                if inputs
                    .iter()
                    .any(|other| other.body != input.body && rect.contains(other.marker))
                {
                    continue;
                }
                output.push(PlacedLabel {
                    body: input.body,
                    rect,
                    marker: input.marker,
                    slot,
                    leader: slot != 0,
                });
                break;
            }
            if (input.selected || input.focused) && !output.iter().any(|l| l.body == input.body) {
                let min = [viewport.min[0] + 4.0, viewport.min[1] + 4.0];
                let rect = ScreenRect {
                    min,
                    max: [min[0] + input.size[0], min[1] + input.size[1]],
                };
                if viewport.contains_rect(rect)
                    && !protected.iter().any(|p| p.intersects(rect))
                    && !output.iter().any(|l| l.rect.intersects(rect))
                {
                    output.push(PlacedLabel {
                        body: input.body,
                        rect,
                        marker: input.marker,
                        slot: 24,
                        leader: true,
                    });
                }
            }
        }
        self.previous.clear();
        self.previous.extend_from_slice(output);
    }
}
pub fn compact_distance(distance_m: f64) -> String {
    if distance_m >= 1.0e10 {
        format!("{:.3} AU", distance_m / 149_597_870_700.0)
    } else if distance_m >= 1000.0 {
        format!("{:.2} km", distance_m / 1000.0)
    } else {
        format!("{distance_m:.2} m")
    }
}
/// Physical-sphere fade; unavailable geometry must retain its navigation fallback.
pub fn marker_opacity(apparent_diameter: f64, physical_sphere: bool) -> f32 {
    if !physical_sphere {
        return 1.0;
    }
    let t = ((apparent_diameter - 8.0) / 16.0).clamp(0.0, 1.0);
    (1.0 - t * t * (3.0 - 2.0 * t)) as f32
}
