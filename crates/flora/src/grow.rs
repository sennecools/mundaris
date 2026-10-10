//! Genome → branch skeleton and organ placements (genesis design §4.2).
//!
//! Trees, shrubs, columnar and cushion forms use a self-organising tree model
//! with space colonisation as the environment (Palubicki et al. 2009, after
//! Runions et al. 2007):
//! 1. markers fill the crown envelope (free space);
//! 2. each marker feeds the nearest bud that perceives it (radius and cone);
//! 3. bud signals `Q` flow to the base, resource `v = α Q` flows back up and
//!    splits at every node with the Borchert–Honda rule (apical dominance λ);
//! 4. buds with `v ≥ 1` grow `⌊v⌋` internodes toward
//!    `straightness·d + space_seeking·V + gravitropism·up + wind + wobble`,
//!    placing lateral buds by phyllotaxis and branch angle;
//! 5. markers near new nodes are consumed;
//! 6. branches starved of space are shed (open space under the crown).
//!
//! Radii follow the pipe model `r_parent^n = Σ r_child^n`. Tufts, rosettes
//! and caps are placed directly (no competition to simulate).
//!
//! Pure, deterministic CPU f64 code: fixed iteration order, counter-based
//! hashing only (`hash.rs`), no hash-map iteration.

use std::collections::HashMap as StdHashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// FxHash (rustc): fast, deterministic; grid keys are small integers.
#[derive(Default, Clone, Copy)]
struct Fx(u64);

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }
}

type HashMap<K, V> = StdHashMap<K, V, BuildHasherDefault<Fx>>;

use glam::DVec3;

use crate::genome::{AxisMode, CrownShape, FruitKind, Genome, GrowthForm, OrganOrientation};
use crate::hash::{Stream, domain};

const UP: DVec3 = DVec3::Z;
/// Perception radius in internode lengths (Palubicki: 4–6).
const PERCEPTION: f64 = 4.0;
/// Occupancy (marker kill) radius in internode lengths (Palubicki: 2).
const OCCUPANCY: f64 = 1.6;
/// Cosine of the perception cone half-angle (Palubicki: 90° full cone).
const CONE_COS: f64 = 0.5; // 60° half-angle; Palubicki use 45°, wider keeps laterals fed.
const MAX_MARKERS: usize = 6000;
const MAX_SHOOT: u32 = 3;
/// Idle cycles after which a lateral / apical bud aborts.
const BUD_ABORT_LATERAL: u32 = 3;
const BUD_ABORT_APICAL: u32 = 6;
const MAX_NODES: usize = 6000;

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub pos: DVec3,
    pub parent: Option<u32>,
    pub order: u32,
    pub axis: u32,
    /// Radius of the internode ending at this node (root: base radius).
    pub radius: f64,
    /// Child that continues this node's axis.
    pub main_child: Option<u32>,
    pub children: Vec<u32>,
    /// Internodes to the nearest tip.
    pub tip_distance: u32,
    /// Index of this node along its axis (0 = first).
    pub rank: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    Organ,
    Fruit,
}

/// A kit shape placed on the plant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub kind: PartKind,
    pub pos: DVec3,
    /// Unit organ axis (kit x).
    pub forward: DVec3,
    /// Unit face normal (kit z).
    pub normal: DVec3,
    pub size: f64,
    /// Order of the bearing branch.
    pub order: u32,
    /// 0 at the inner crown, 1 at the tips (colour gradient).
    pub tipness: f32,
}

/// Special geometry that is not a branch tube or a kit part.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Extra {
    /// Mushroom-like cap: centre, radius, height of the dome.
    Cap {
        centre: DVec3,
        radius: f64,
        height: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Skeleton {
    pub nodes: Vec<Node>,
    pub parts: Vec<Part>,
    pub extras: Vec<Extra>,
    /// Growth statistics (diagnostics).
    pub markers: usize,
    pub markers_left: usize,
    pub cycles_run: u32,
    pub shed_branches: u32,
}

impl Skeleton {
    pub fn internodes(&self) -> usize {
        self.nodes.iter().filter(|n| n.parent.is_some()).count()
    }
}

#[derive(Debug, Clone)]
struct Bud {
    node: u32,
    dir: DVec3,
    order: u32,
    axis: u32,
    /// Apical (true) or dormant lateral (false).
    apical: bool,
    alive: bool,
    q: f64,
    v: f64,
    space: DVec3,
    /// Node counter on its axis for phyllotaxis.
    serial: u32,
    /// Consecutive cycles without perceived space.
    idle: u32,
}

struct Grower<'g> {
    g: &'g Genome,
    seed: u64,
    nodes: Vec<Node>,
    node_alive: Vec<bool>,
    buds: Vec<Bud>,
    markers: Vec<DVec3>,
    marker_alive: Vec<bool>,
    marker_grid: HashMap<(i64, i64, i64), Vec<u32>>,
    marker_cell: f64,
    marker_lo: DVec3,
    marker_hi: DVec3,
    next_axis: u32,
    shed: u32,
}

fn cell(p: DVec3, size: f64) -> (i64, i64, i64) {
    (
        (p.x / size).floor() as i64,
        (p.y / size).floor() as i64,
        (p.z / size).floor() as i64,
    )
}

fn perpendicular(d: DVec3) -> (DVec3, DVec3) {
    let r = if d.z.abs() < 0.9 { UP } else { DVec3::X };
    let u = d.cross(r).normalize();
    (u, d.cross(u))
}

impl Genome {
    pub fn crown_bottom_m(&self) -> f64 {
        self.crown_base * self.height_m
    }
    pub fn crown_height_m(&self) -> f64 {
        self.height_m - self.crown_bottom_m()
    }
    pub fn crown_radius_m(&self) -> f64 {
        0.5 * self.crown_aspect * self.crown_height_m()
    }

    /// Crown radius allowed at relative crown height `t` in [0, 1].
    pub fn envelope_radius(&self, t: f64) -> f64 {
        if !(0.0..=1.0).contains(&t) {
            return 0.0;
        }
        let r = self.crown_radius_m();
        r * match self.crown_shape {
            CrownShape::Ellipsoid => (1.0 - (2.0 * t - 1.0).powi(2)).max(0.0).sqrt(),
            CrownShape::Cone => (1.0 - t).powf(0.9),
            CrownShape::Umbrella => {
                let top = ((t - 0.7).max(0.0) / 0.3).min(1.0);
                t.powf(0.45) * (1.0 - top * top).max(0.0).sqrt()
            }
            CrownShape::Cylinder => 1.0,
            CrownShape::Hemisphere => (1.0 - t * t).max(0.0).sqrt(),
        }
    }

    pub fn gravitropism_at(&self, order: u32) -> f64 {
        self.gravitropism + self.gravitropism_per_order * order as f64
    }

    fn internode_at(&self, order: u32) -> f64 {
        self.internode_m * self.internode_per_order.powi(order as i32)
    }

    fn branch_angle(&self, order: u32) -> f64 {
        (self.branch_angle_deg + self.branch_angle_per_order_deg * order.saturating_sub(1) as f64)
            .clamp(2.0, 170.0)
            .to_radians()
    }
}

/// Grow the skeleton and organ placements for one individual.
pub fn grow(g: &Genome, seed: u64) -> Skeleton {
    match g.form {
        GrowthForm::Tree | GrowthForm::Shrub | GrowthForm::Columnar | GrowthForm::Cushion => {
            grow_sot(g, seed)
        }
        GrowthForm::Tuft => grow_tuft(g, seed),
        GrowthForm::Rosette => grow_rosette(g, seed),
        GrowthForm::FungalCap => grow_cap(g, seed),
    }
}

fn grow_sot(g: &Genome, seed: u64) -> Skeleton {
    let mut w = Grower {
        g,
        seed,
        nodes: Vec::new(),
        node_alive: Vec::new(),
        buds: Vec::new(),
        markers: Vec::new(),
        marker_alive: Vec::new(),
        marker_grid: HashMap::default(),
        marker_cell: (OCCUPANCY * g.internode_m).max(1e-3),
        marker_lo: DVec3::ZERO,
        marker_hi: DVec3::ZERO,
        next_axis: 0,
        shed: 0,
    };
    w.scatter_markers();
    w.plant_stems();
    let mut cycles = 0;
    for cycle in 0..g.cycles {
        cycles = cycle + 1;
        if !w.cycle(cycle) || w.nodes.len() >= MAX_NODES {
            break;
        }
    }
    if g.shed_threshold > 0.0 {
        w.shed_shaded();
    }
    w.finish(cycles)
}

impl Grower<'_> {
    fn add_node(
        &mut self,
        parent: Option<u32>,
        pos: DVec3,
        order: u32,
        axis: u32,
        rank: u32,
    ) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            pos,
            parent,
            order,
            axis,
            radius: 0.0,
            main_child: None,
            children: Vec::new(),
            tip_distance: 0,
            rank,
        });
        self.node_alive.push(true);
        if let Some(p) = parent {
            let pn = &mut self.nodes[p as usize];
            pn.children.push(id);
            if pn.axis == axis {
                pn.main_child = Some(id);
            }
        }
        id
    }

    fn scatter_markers(&mut self) {
        let g = self.g;
        let s = Stream::new(self.seed, domain::MARKERS);
        let zb = g.crown_bottom_m();
        let h = g.crown_height_m().max(1e-3);
        let r = g.crown_radius_m().max(1e-3);
        // Envelope volume by midpoint integration of the radius profile.
        let mut vol = 0.0;
        for i in 0..64 {
            let t = (i as f64 + 0.5) / 64.0;
            vol += std::f64::consts::PI * g.envelope_radius(t).powi(2) * h / 64.0;
        }
        let spacing = g.internode_m;
        let target = ((vol / spacing.powi(3)) * 2.0).clamp(150.0, MAX_MARKERS as f64) as usize;
        let mut i = 0u64;
        while self.markers.len() < target && i < (target as u64) * 20 {
            let x = s.signed(i, 0) * r;
            let y = s.signed(i, 1) * r;
            let t = s.unit(i, 2);
            i += 1;
            if x * x + y * y <= g.envelope_radius(t).powi(2) {
                // Wind lean shifts the envelope downwind with height.
                let lean = g.wind_bias * 0.25 * r * t;
                self.markers.push(DVec3::new(x + lean, y, zb + t * h));
            }
        }
        self.marker_alive = vec![true; self.markers.len()];
        self.marker_lo = self
            .markers
            .iter()
            .fold(DVec3::splat(f64::MAX), |a, m| a.min(*m));
        self.marker_hi = self
            .markers
            .iter()
            .fold(DVec3::splat(f64::MIN), |a, m| a.max(*m));
        if self.markers.is_empty() {
            (self.marker_lo, self.marker_hi) = (DVec3::ZERO, DVec3::ZERO);
        }
        for (k, m) in self.markers.iter().enumerate() {
            self.marker_grid
                .entry(cell(*m, self.marker_cell))
                .or_default()
                .push(k as u32);
        }
    }

    /// Root, stems and the clear bole up to the crown.
    fn plant_stems(&mut self) {
        let g = self.g;
        let root = self.add_node(None, DVec3::ZERO, 0, u32::MAX, 0);
        let stems = g.stems.max(1);
        let st = Stream::new(self.seed, domain::STEM);
        let bole_top = (g.crown_bottom_m() - 0.5 * PERCEPTION * g.internode_m).max(0.0);
        for k in 0..stems {
            let axis = self.next_axis;
            self.next_axis += 1;
            let mut dir = UP;
            if stems > 1 {
                let az =
                    (k as f64 / stems as f64) * std::f64::consts::TAU + st.unit(k as u64, 0) * 0.6;
                let tilt = (0.25 + 0.35 * st.unit(k as u64, 1)) * g.branch_angle(1).min(1.2);
                dir = (UP * tilt.cos() + DVec3::new(az.cos(), az.sin(), 0.0) * tilt.sin())
                    .normalize();
            }
            let mut node = root;
            let mut pos = DVec3::ZERO;
            let mut rank = 0;
            let l = g.internode_at(0);
            // Always at least one internode so every stem has a bud above ground.
            loop {
                let wob = DVec3::new(
                    st.signed(k as u64 + 100, rank as u64),
                    st.signed(k as u64 + 200, rank as u64),
                    0.0,
                );
                dir = (dir * (1.0 + g.straightness)
                    + UP * (0.6 + g.gravitropism_at(0).max(0.0))
                    + wob * g.wobble * 0.3)
                    .normalize();
                pos += dir * l;
                node = self.add_node(Some(node), pos, 0, axis, rank);
                self.add_laterals(node, dir, 0, axis, rank);
                rank += 1;
                if pos.z >= bole_top || rank > 400 {
                    break;
                }
            }
            self.buds.push(Bud {
                node,
                dir,
                order: 0,
                axis,
                apical: true,
                alive: true,
                q: 0.0,
                v: 0.0,
                space: DVec3::ZERO,
                serial: rank,
                idle: 0,
            });
        }
    }

    fn add_laterals(&mut self, node: u32, dir: DVec3, order: u32, axis: u32, serial: u32) {
        let g = self.g;
        if order + 1 > g.max_order {
            return;
        }
        let (u, w) = perpendicular(dir);
        let beta = g.branch_angle(order + 1);
        let jit = Stream::new(self.seed, domain::BUD_JITTER);
        let n = g.buds_per_node.max(1);
        for j in 0..n {
            let phi = (serial as f64 * g.phyllotaxis_deg + j as f64 * 360.0 / n as f64)
                .to_radians()
                + jit.signed(node as u64, j as u64) * 0.15;
            let out = u * phi.cos() + w * phi.sin();
            let d = (dir * beta.cos() + out * beta.sin()).normalize();
            self.buds.push(Bud {
                node,
                dir: d,
                order: order + 1,
                axis,
                apical: false,
                alive: true,
                q: 0.0,
                v: 0.0,
                space: DVec3::ZERO,
                serial: 0,
                idle: 0,
            });
        }
    }

    /// One growth cycle. Returns false when nothing can grow any more.
    fn cycle(&mut self, cycle: u32) -> bool {
        let g = self.g;
        // 1. Environment: each marker feeds the nearest perceiving bud.
        let max_l = g.internode_m.max(g.internode_at(g.max_order));
        let bud_cell = PERCEPTION * max_l;
        // Alive buds packed with their perception reach, in a dense grid
        // over the marker box (a bud farther than its reach from that box
        // perceives nothing).
        struct Seen {
            pos: DVec3,
            dir: DVec3,
            reach2: f64,
            bud: u32,
        }
        let mut seen: Vec<Seen> = Vec::new();
        for (i, b) in self.buds.iter_mut().enumerate() {
            b.q = 0.0;
            b.v = 0.0;
            b.space = DVec3::ZERO;
            if b.alive {
                let reach = PERCEPTION * g.internode_at(b.order);
                seen.push(Seen {
                    pos: self.nodes[b.node as usize].pos,
                    dir: b.dir,
                    reach2: reach * reach,
                    bud: i as u32,
                });
            }
        }
        let grid = DenseGrid::build(
            self.marker_lo - DVec3::splat(bud_cell),
            self.marker_hi + DVec3::splat(bud_cell),
            bud_cell,
            seen.iter().map(|s| s.pos),
        );
        let mut any = false;
        for (m, &mp) in self.markers.iter().enumerate() {
            if !self.marker_alive[m] {
                continue;
            }
            let mut best: Option<(f64, u32)> = None;
            grid.around(mp, |k| {
                let s = &seen[k as usize];
                let to = mp - s.pos;
                let d2 = to.length_squared();
                if d2 > s.reach2 || d2 < 1e-18 {
                    return;
                }
                let d = d2.sqrt();
                if to.dot(s.dir) < CONE_COS * d {
                    return;
                }
                let bi = s.bud;
                if best.is_none_or(|(bd, bb)| d < bd || (d == bd && bi < bb)) {
                    best = Some((d, bi));
                }
            });
            if let Some((d, bi)) = best {
                let b = &mut self.buds[bi as usize];
                b.q = 1.0;
                b.space += (mp - self.nodes[b.node as usize].pos) / d;
                any = true;
            }
        }
        // Buds that perceive no free space for a while abort (dormant buds
        // die), so the crown interior stops costing perception work.
        for b in &mut self.buds {
            if b.alive {
                b.idle = if b.q > 0.0 { 0 } else { b.idle + 1 };
                if b.idle
                    >= if b.apical {
                        BUD_ABORT_APICAL
                    } else {
                        BUD_ABORT_LATERAL
                    }
                {
                    b.alive = false;
                }
            }
        }
        if !any {
            return false;
        }
        // 2. Basipetal: Q per node subtree (children have larger indices).
        let n = self.nodes.len();
        let mut node_q = vec![0.0f64; n];
        for b in &self.buds {
            if b.alive {
                node_q[b.node as usize] += b.q;
            }
        }
        for i in (1..n).rev() {
            if self.node_alive[i]
                && let Some(p) = self.nodes[i].parent
            {
                node_q[p as usize] += node_q[i];
            }
        }
        // 3. Acropetal: Borchert–Honda allocation.
        let lambda = g.apical_dominance;
        let mut node_v = vec![0.0f64; n];
        node_v[0] = g.vigor * node_q[0];
        let mut buds_at: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (i, b) in self.buds.iter().enumerate() {
            if b.alive && b.q > 0.0 {
                buds_at[b.node as usize].push(i as u32);
            }
        }
        for i in 0..n {
            if !self.node_alive[i] || node_v[i] <= 0.0 {
                continue;
            }
            let v = node_v[i];
            // Consumers: (is_main, q, target) where target = node or bud.
            let mut main_q = 0.0;
            let mut lat_q = 0.0;
            let node = &self.nodes[i];
            for &c in &node.children {
                if !self.node_alive[c as usize] {
                    continue;
                }
                if Some(c) == node.main_child && i != 0 {
                    main_q += node_q[c as usize];
                } else {
                    lat_q += node_q[c as usize];
                }
            }
            for &bi in &buds_at[i] {
                let b = &self.buds[bi as usize];
                if b.apical && i != 0 {
                    main_q += b.q;
                } else {
                    lat_q += b.q;
                }
            }
            // At the root all stems are equal laterals.
            let (lm, ll) = if i == 0 {
                (0.5, 0.5)
            } else {
                (lambda, 1.0 - lambda)
            };
            let denom = lm * main_q + ll * lat_q;
            if denom <= 0.0 {
                continue;
            }
            let v_main_per_q = v * lm / denom;
            let v_lat_per_q = v * ll / denom;
            let children = self.nodes[i].children.clone();
            let main_child = self.nodes[i].main_child;
            for c in children {
                if !self.node_alive[c as usize] {
                    continue;
                }
                let per = if Some(c) == main_child && i != 0 {
                    v_main_per_q
                } else {
                    v_lat_per_q
                };
                node_v[c as usize] = per * node_q[c as usize];
            }
            for &bi in &buds_at[i] {
                let b = &mut self.buds[bi as usize];
                let per = if b.apical && i != 0 {
                    v_main_per_q
                } else {
                    v_lat_per_q
                };
                b.v = per * b.q;
            }
        }
        // 4. Shoot growth.
        let wobble = Stream::new(self.seed, domain::BUD_ROLL);
        let wind = DVec3::X * g.wind_bias;
        let bud_count = self.buds.len();
        let mut new_nodes: Vec<u32> = Vec::new();
        for bi in 0..bud_count {
            let b = self.buds[bi].clone();
            if !b.alive || b.q <= 0.0 || b.v < 1.0 || !self.node_alive[b.node as usize] {
                continue;
            }
            let shoots = (b.v.floor() as u32).min(MAX_SHOOT);
            let (axis, rank0) = if b.apical {
                (b.axis, b.serial)
            } else {
                let a = self.next_axis;
                self.next_axis += 1;
                (a, 0)
            };
            let space = if b.space.length_squared() > 0.0 {
                b.space.normalize()
            } else {
                DVec3::ZERO
            };
            let l = g.internode_at(b.order);
            let mut dir = b.dir;
            let mut node = b.node;
            let mut rank = rank0;
            for s in 0..shoots {
                let r = DVec3::new(
                    wobble.signed(bi as u64, (cycle * 8 + s) as u64 * 3),
                    wobble.signed(bi as u64, (cycle * 8 + s) as u64 * 3 + 1),
                    wobble.signed(bi as u64, (cycle * 8 + s) as u64 * 3 + 2),
                );
                let first_lateral = !b.apical && s == 0;
                let pull = if first_lateral { 0.35 } else { 1.0 };
                let d = dir * g.straightness.max(0.05)
                    + (space * g.space_seeking
                        + UP * g.gravitropism_at(b.order)
                        + wind
                        + r * g.wobble)
                        * pull;
                dir = if d.length_squared() > 1e-12 {
                    d.normalize()
                } else {
                    dir
                };
                let pos = self.nodes[node as usize].pos + dir * l;
                // Never grow into the ground.
                let pos = DVec3::new(pos.x, pos.y, pos.z.max(0.02 * l));
                node = self.add_node(Some(node), pos, b.order, axis, rank);
                new_nodes.push(node);
                self.add_laterals(node, dir, b.order, axis, rank);
                rank += 1;
            }
            let bud = &mut self.buds[bi];
            bud.node = node;
            bud.dir = dir;
            bud.axis = axis;
            bud.serial = rank;
            bud.apical = true;
            if g.axis_mode == AxisMode::Sympodial && b.apical {
                bud.alive = false;
            }
        }
        // 5. Consume markers in the occupancy zone of new nodes.
        for &nn in &new_nodes {
            let p = self.nodes[nn as usize].pos;
            let kill = OCCUPANCY * g.internode_at(self.nodes[nn as usize].order);
            let c = cell(p, self.marker_cell);
            let reach = (kill / self.marker_cell).ceil() as i64;
            for dx in -reach..=reach {
                for dy in -reach..=reach {
                    for dz in -reach..=reach {
                        if let Some(list) = self.marker_grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) {
                            for &m in list {
                                if (self.markers[m as usize] - p).length_squared() < kill * kill {
                                    self.marker_alive[m as usize] = false;
                                }
                            }
                        }
                    }
                }
            }
        }
        !new_nodes.is_empty() || self.buds.iter().any(|b| b.alive && b.q > 0.0)
    }

    /// Light-based shedding (Palubicki 2009 shadow propagation): every tip
    /// (foliage) casts a pyramid of shadow into the voxels below it; a
    /// lateral branch whose tips get too little light is dropped. This gives
    /// the open space under the crown. Runs once after growth, because the
    /// space markers are exhausted by then and say nothing about light.
    fn shed_shaded(&mut self) {
        const DEPTH: i64 = 8;
        const FALLOFF: f64 = 1.35;
        // Shadow is relative to the most shaded tips of this plant (95th
        // percentile), so dense and sparse species shed alike.
        let g = self.g;
        let vox = g.internode_m.max(1e-3);
        let n = self.nodes.len();
        let mut has_child = vec![false; n];
        for i in 1..n {
            if self.node_alive[i]
                && let Some(p) = self.nodes[i].parent
            {
                has_child[p as usize] = true;
            }
        }
        let tips: Vec<usize> = (1..n)
            .filter(|&i| self.node_alive[i] && !has_child[i])
            .collect();
        let mut shadow: HashMap<(i64, i64, i64), f64> = HashMap::default();
        for &t in &tips {
            let c = cell(self.nodes[t].pos, vox);
            for q in 1..=DEPTH {
                let s = FALLOFF.powi(-(q as i32));
                let r = q / 3;
                for dx in -r..=r {
                    for dy in -r..=r {
                        *shadow.entry((c.0 + dx, c.1 + dy, c.2 - q)).or_default() += s;
                    }
                }
            }
        }
        let mut light = vec![0.0f64; n];
        let mut count = vec![0u32; n];
        let tip_shadow: Vec<f64> = tips
            .iter()
            .map(|&t| {
                shadow
                    .get(&cell(self.nodes[t].pos, vox))
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect();
        let mut sorted = tip_shadow.clone();
        sorted.sort_by(f64::total_cmp);
        let full = sorted
            .get(sorted.len() * 95 / 100)
            .copied()
            .unwrap_or(1.0)
            .max(1e-6);
        for (k, &t) in tips.iter().enumerate() {
            light[t] = (1.0 - tip_shadow[k] / full).max(0.0);
            count[t] = 1;
        }
        // Accumulate tip light to every node (children after parents).
        for i in (1..n).rev() {
            if !self.node_alive[i] {
                continue;
            }
            if let Some(p) = self.nodes[i].parent {
                light[p as usize] += light[i];
                count[p as usize] += count[i];
            }
        }
        for i in 1..n {
            if !self.node_alive[i] {
                continue;
            }
            let p = self.nodes[i].parent.unwrap() as usize;
            let starts_branch = self.nodes[p].axis != self.nodes[i].axis && self.nodes[i].order > 0;
            if starts_branch
                && count[i] > 0
                && light[i] / (count[i] as f64) < g.shed_threshold * 0.5
            {
                self.kill_subtree(i as u32);
                self.shed += 1;
            }
        }
    }

    fn kill_subtree(&mut self, root: u32) {
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            self.node_alive[i as usize] = false;
            stack.extend(self.nodes[i as usize].children.iter().copied());
        }
        for b in &mut self.buds {
            if !self.node_alive[b.node as usize] {
                b.alive = false;
            }
        }
    }

    fn finish(self, cycles: u32) -> Skeleton {
        let g = self.g;
        // Compact live nodes (order preserved, so parents stay before children).
        let mut remap = vec![u32::MAX; self.nodes.len()];
        let mut nodes = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if self.node_alive[i] {
                remap[i] = nodes.len() as u32;
                nodes.push(n.clone());
            }
        }
        for n in &mut nodes {
            n.parent = n.parent.map(|p| remap[p as usize]);
            n.children = n
                .children
                .iter()
                .filter(|&&c| remap[c as usize] != u32::MAX)
                .map(|&c| remap[c as usize])
                .collect();
            n.main_child = n
                .main_child
                .and_then(|c| (remap[c as usize] != u32::MAX).then(|| remap[c as usize]));
        }
        // Drop stems that never reached their crown? Keep: a bare stem is valid.
        finish_radii(&mut nodes, g);
        let mut sk = Skeleton {
            nodes,
            markers: self.markers.len(),
            markers_left: self.marker_alive.iter().filter(|a| **a).count(),
            cycles_run: cycles,
            shed_branches: self.shed,
            ..Default::default()
        };
        place_parts(&mut sk, g, self.seed);
        sk
    }
}

/// Pipe-model radii and tip distances, leaves to root.
fn finish_radii(nodes: &mut [Node], g: &Genome) {
    let n = nodes.len();
    let pn = g.pipe_exponent;
    let mut acc = vec![0.0f64; n];
    for i in (0..n).rev() {
        let r = if nodes[i].children.is_empty() {
            nodes[i].tip_distance = 0;
            g.tip_radius_m
        } else {
            let td = nodes[i]
                .children
                .iter()
                .map(|&c| nodes[c as usize].tip_distance)
                .min()
                .unwrap_or(0);
            nodes[i].tip_distance = td + 1;
            acc[i].powf(1.0 / pn).max(g.tip_radius_m)
        };
        nodes[i].radius = r;
        if let Some(p) = nodes[i].parent {
            acc[p as usize] += r.powf(pn);
        }
    }
    // Base flare on the lowest part of each stem.
    let flare_h = 0.08 * g.height_m.max(1e-3);
    for node in nodes.iter_mut() {
        if node.order == 0 && node.pos.z < flare_h {
            let t = 1.0 - node.pos.z / flare_h;
            node.radius *= 1.0 + g.flare * t * t;
        }
    }
    if let Some(root) = nodes.first_mut() {
        root.radius *= 1.0 + g.flare;
    }
}

fn orient(g: &Genome, branch: DVec3, out: DVec3) -> (DVec3, DVec3) {
    let fwd = match g.organ_orientation {
        OrganOrientation::TowardLight => out * 0.8 + UP * 0.6 + branch * 0.3,
        OrganOrientation::Vertical => UP + out * 0.25,
        OrganOrientation::Pendant => out * 0.45 - UP,
        OrganOrientation::Along => branch + out * 0.7,
    }
    .normalize();
    let mut nrm = UP - fwd * UP.dot(fwd);
    if nrm.length_squared() < 1e-6 {
        nrm = out - fwd * out.dot(fwd);
    }
    (fwd, nrm.normalize())
}

fn place_parts(sk: &mut Skeleton, g: &Genome, seed: u64) {
    let os = Stream::new(seed, domain::ORGAN);
    let fs = Stream::new(seed, domain::FRUIT);
    let reach = g.organ_reach.max(1);
    let mut parts = Vec::new();
    for (i, node) in sk.nodes.iter().enumerate() {
        let Some(p) = node.parent else { continue };
        let parent = &sk.nodes[p as usize];
        let seg = node.pos - parent.pos;
        let len = seg.length();
        if len < 1e-9 {
            continue;
        }
        let dir = seg / len;
        if g.organ != crate::genome::OrganKind::Leafless
            && node.tip_distance < reach
            && node.pos.z > 0.02 * g.height_m
        {
            let (u, w) = perpendicular(dir);
            let tipness = 1.0 - node.tip_distance as f32 / reach as f32;
            let k = g.organ_cluster.max(1);
            for j in 0..k {
                let phi = (node.rank as f64 * g.phyllotaxis_deg + j as f64 * 360.0 / k as f64)
                    .to_radians()
                    + os.signed(i as u64, j as u64 * 4) * 0.4;
                let out = u * phi.cos() + w * phi.sin();
                let (fwd, nrm) = orient(g, dir, out);
                let along = 0.35 + 0.65 * (j as f64 + 0.5) / k as f64;
                let size = g.organ_size_m * (0.8 + 0.4 * os.unit(i as u64, j as u64 * 4 + 1));
                parts.push(Part {
                    kind: PartKind::Organ,
                    pos: parent.pos + seg * along + out * node.radius,
                    forward: fwd,
                    normal: nrm,
                    size,
                    order: node.order,
                    tipness,
                });
            }
        }
        if g.fruit != FruitKind::Barren
            && node.children.is_empty()
            && fs.unit(i as u64, 0) < g.fruit_chance
        {
            let fwd = match g.fruit {
                FruitKind::Bloom | FruitKind::SporeBody => dir,
                _ => -UP,
            };
            let (u, _) = perpendicular(fwd);
            parts.push(Part {
                kind: PartKind::Fruit,
                pos: node.pos,
                forward: fwd,
                normal: u,
                size: g.fruit_size_m * (0.85 + 0.3 * fs.unit(i as u64, 1)),
                order: node.order,
                tipness: 1.0,
            });
        }
    }
    sk.parts = parts;
}

fn single_stalk(g: &Genome, height: f64, seed: u64) -> Vec<Node> {
    let st = Stream::new(seed, domain::STEM);
    let segs = ((height / g.internode_m).ceil() as u32).clamp(1, 24);
    let mut nodes = vec![Node {
        pos: DVec3::ZERO,
        parent: None,
        order: 0,
        axis: u32::MAX,
        radius: 0.0,
        main_child: None,
        children: Vec::new(),
        tip_distance: 0,
        rank: 0,
    }];
    let mut dir = UP;
    for k in 0..segs {
        let wob = DVec3::new(st.signed(k as u64, 0), st.signed(k as u64, 1), 0.0) * g.wobble * 0.2;
        dir = (dir + wob + DVec3::X * g.wind_bias * 0.1).normalize();
        let id = nodes.len() as u32;
        let pos = nodes[id as usize - 1].pos + dir * (height / segs as f64);
        nodes[id as usize - 1].children.push(id);
        nodes[id as usize - 1].main_child = Some(id);
        nodes.push(Node {
            pos,
            parent: Some(id - 1),
            order: 0,
            axis: 0,
            radius: 0.0,
            main_child: None,
            children: Vec::new(),
            tip_distance: 0,
            rank: k,
        });
    }
    nodes
}

fn grow_tuft(g: &Genome, seed: u64) -> Skeleton {
    let s = Stream::new(seed, domain::TUFT);
    let blades = (g.stems * g.organ_cluster).clamp(1, 64);
    let mut parts = Vec::new();
    for k in 0..blades {
        let phi = (k as f64 * g.phyllotaxis_deg).to_radians() + s.signed(k as u64, 0) * 0.3;
        let out = DVec3::new(phi.cos(), phi.sin(), 0.0);
        let tilt = g.branch_angle(1) * (0.3 + 0.7 * s.unit(k as u64, 1));
        let fwd = (UP * tilt.cos() + out * tilt.sin() + DVec3::X * g.wind_bias * 0.2).normalize();
        let nrm = (out.cross(UP)).cross(fwd).normalize();
        parts.push(Part {
            kind: PartKind::Organ,
            pos: out * g.crown_radius_m() * 0.2 * s.unit(k as u64, 2),
            forward: fwd,
            normal: if nrm.is_finite() { nrm } else { out },
            size: g.height_m * (0.6 + 0.4 * s.unit(k as u64, 3)),
            order: 1,
            tipness: s.unit(k as u64, 4) as f32,
        });
    }
    let mut nodes = single_stalk(g, g.internode_m.min(g.height_m * 0.1), seed);
    finish_radii(&mut nodes, g);
    let mut sk = Skeleton {
        nodes,
        parts,
        ..Default::default()
    };
    add_tip_fruit(&mut sk, g, seed);
    sk
}

fn grow_rosette(g: &Genome, seed: u64) -> Skeleton {
    let s = Stream::new(seed, domain::TUFT);
    let leaves = (g.stems * g.organ_cluster).clamp(1, 64);
    let mut parts = Vec::new();
    for k in 0..leaves {
        let phi = (k as f64 * g.phyllotaxis_deg).to_radians();
        let out = DVec3::new(phi.cos(), phi.sin(), 0.0);
        // Inner leaves stand up, outer leaves lie flat.
        let inner = k as f64 / leaves as f64;
        let tilt = g.branch_angle(1) * (1.0 - 0.6 * inner) + s.signed(k as u64, 0) * 0.1;
        let fwd = (UP * tilt.cos() + out * tilt.sin()).normalize();
        let nrm = out.cross(UP).cross(fwd).normalize();
        parts.push(Part {
            kind: PartKind::Organ,
            pos: out * 0.02 * g.organ_size_m,
            forward: fwd,
            normal: nrm,
            size: g.organ_size_m * (1.0 - 0.5 * inner) * (0.85 + 0.3 * s.unit(k as u64, 1)),
            order: 1,
            tipness: inner as f32,
        });
    }
    let stalk = g.height_m.max(g.internode_m);
    let mut nodes = single_stalk(g, stalk, seed);
    finish_radii(&mut nodes, g);
    let mut sk = Skeleton {
        nodes,
        parts,
        ..Default::default()
    };
    add_tip_fruit(&mut sk, g, seed);
    sk
}

fn grow_cap(g: &Genome, seed: u64) -> Skeleton {
    let mut nodes = single_stalk(g, g.height_m, seed);
    finish_radii(&mut nodes, g);
    let top = nodes.last().map(|n| n.pos).unwrap_or(DVec3::ZERO);
    let radius = g.crown_radius_m().max(g.tip_radius_m * 2.0);
    let mut sk = Skeleton {
        nodes,
        extras: vec![Extra::Cap {
            centre: top,
            radius,
            height: radius * 0.6,
        }],
        ..Default::default()
    };
    add_tip_fruit(&mut sk, g, seed);
    sk
}

fn add_tip_fruit(sk: &mut Skeleton, g: &Genome, seed: u64) {
    if g.fruit == FruitKind::Barren {
        return;
    }
    let fs = Stream::new(seed, domain::FRUIT);
    if fs.unit(0, 7) >= g.fruit_chance {
        return;
    }
    if let Some(top) = sk.nodes.last() {
        sk.parts.push(Part {
            kind: PartKind::Fruit,
            pos: top.pos,
            forward: UP,
            normal: DVec3::X,
            size: g.fruit_size_m,
            order: 0,
            tipness: 1.0,
        });
    }
}

/// Dense uniform grid (compressed rows) over a box; points outside are not
/// stored. Iteration order is deterministic (insertion order per cell).
struct DenseGrid {
    lo: DVec3,
    inv: f64,
    dims: [i64; 3],
    start: Vec<u32>,
    items: Vec<u32>,
}

impl DenseGrid {
    fn build(lo: DVec3, hi: DVec3, cell: f64, points: impl Iterator<Item = DVec3> + Clone) -> Self {
        let inv = 1.0 / cell;
        let ext = ((hi - lo) * inv).ceil();
        let dims = [
            ext.x.max(1.0) as i64,
            ext.y.max(1.0) as i64,
            ext.z.max(1.0) as i64,
        ];
        let mut g = DenseGrid {
            lo,
            inv,
            dims,
            start: Vec::new(),
            items: Vec::new(),
        };
        let n = (dims[0] * dims[1] * dims[2]) as usize;
        let mut count = vec![0u32; n + 1];
        for p in points.clone() {
            if let Some(c) = g.index(p) {
                count[c + 1] += 1;
            }
        }
        for i in 0..n {
            count[i + 1] += count[i];
        }
        let mut fill = count.clone();
        g.items = vec![0; count[n] as usize];
        for (k, p) in points.enumerate() {
            if let Some(c) = g.index(p) {
                g.items[fill[c] as usize] = k as u32;
                fill[c] += 1;
            }
        }
        g.start = count;
        g
    }

    fn coords(&self, p: DVec3) -> [i64; 3] {
        let q = (p - self.lo) * self.inv;
        [q.x.floor() as i64, q.y.floor() as i64, q.z.floor() as i64]
    }

    fn index(&self, p: DVec3) -> Option<usize> {
        let c = self.coords(p);
        (0..3)
            .all(|a| c[a] >= 0 && c[a] < self.dims[a])
            .then(|| ((c[2] * self.dims[1] + c[1]) * self.dims[0] + c[0]) as usize)
    }

    /// Visit items in the 3×3×3 cells around `p`.
    fn around(&self, p: DVec3, mut f: impl FnMut(u32)) {
        let c = self.coords(p);
        for z in (c[2] - 1).max(0)..=(c[2] + 1).min(self.dims[2] - 1) {
            for y in (c[1] - 1).max(0)..=(c[1] + 1).min(self.dims[1] - 1) {
                let row = (z * self.dims[1] + y) * self.dims[0];
                let x0 = (c[0] - 1).max(0);
                let x1 = (c[0] + 1).min(self.dims[0] - 1);
                if x0 > x1 {
                    continue;
                }
                let a = self.start[(row + x0) as usize] as usize;
                let b = self.start[(row + x1 + 1) as usize] as usize;
                for &k in &self.items[a..b] {
                    f(k);
                }
            }
        }
    }
}
