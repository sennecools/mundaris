//! Normalized radial cube charts. U × V = N on every face; increasing (u,v)
//! gives outward CCW triangles. Addresses identify disposable render regions,
//! while a surface location is a body-fixed direction independent of the charts.

use crate::{Direction3, MathError};
use glam::DVec3;

/// Canonical face traversal order. Signed axes are specified by `basis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CubeFace {
    PositiveX,
    NegativeX,
    PositiveY,
    NegativeY,
    PositiveZ,
    NegativeZ,
}
impl CubeFace {
    pub const ALL: [Self; 6] = [
        Self::PositiveX,
        Self::NegativeX,
        Self::PositiveY,
        Self::NegativeY,
        Self::PositiveZ,
        Self::NegativeZ,
    ];
    /// [normal, increasing U, increasing V], in right-handed body axes.
    pub fn basis(self) -> [DVec3; 3] {
        use CubeFace::*;
        match self {
            PositiveX => [DVec3::X, -DVec3::Z, DVec3::Y],
            NegativeX => [-DVec3::X, DVec3::Z, DVec3::Y],
            PositiveY => [DVec3::Y, DVec3::X, -DVec3::Z],
            NegativeY => [-DVec3::Y, DVec3::X, DVec3::Z],
            PositiveZ => [DVec3::Z, DVec3::X, DVec3::Y],
            NegativeZ => [-DVec3::Z, -DVec3::X, DVec3::Y],
        }
    }
    pub fn direction(self, uv: [f64; 2]) -> Result<Direction3, SurfaceMathError> {
        if !uv.iter().all(|x| x.is_finite() && (-1.0..=1.0).contains(x)) {
            return Err(SurfaceMathError::InvalidDomain);
        }
        let [n, u, v] = self.basis();
        Ok(Direction3::try_new(n + uv[0] * u + uv[1] * v)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PatchEdge {
    UMin,
    UMax,
    VMin,
    VMax,
}
impl PatchEdge {
    pub const ALL: [Self; 4] = [Self::UMin, Self::UMax, Self::VMin, Self::VMax];
    pub fn bit(self) -> u8 {
        1 << self as u8
    }
    pub fn opposite(self) -> Self {
        match self {
            Self::UMin => Self::UMax,
            Self::UMax => Self::UMin,
            Self::VMin => Self::VMax,
            Self::VMax => Self::VMin,
        }
    }
    pub fn grid(self, along: u32, cells: u32) -> [u32; 2] {
        match self {
            Self::UMin => [0, along],
            Self::UMax => [cells, along],
            Self::VMin => [along, 0],
            Self::VMax => [along, cells],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum SurfaceMathError {
    #[error("invalid cube patch level/coordinates")]
    InvalidAddress,
    #[error("invalid face/patch domain or power-of-two sample grid")]
    InvalidDomain,
    #[error(transparent)]
    Math(#[from] MathError),
}

/// Checked computed region; maximum representable level is 30, not required detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CubePatchAddress {
    face: CubeFace,
    level: u8,
    x: u32,
    y: u32,
}
impl Ord for CubePatchAddress {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.face
            .cmp(&other.face)
            .then_with(|| self.traversal_key().cmp(&other.traversal_key()))
            .then(self.level.cmp(&other.level))
    }
}
impl PartialOrd for CubePatchAddress {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl CubePatchAddress {
    fn traversal_key(self) -> u64 {
        fn spread(x: u32) -> u64 {
            let mut x = u64::from(x);
            x = (x | x << 16) & 0x0000ffff0000ffff;
            x = (x | x << 8) & 0x00ff00ff00ff00ff;
            x = (x | x << 4) & 0x0f0f0f0f0f0f0f0f;
            x = (x | x << 2) & 0x3333333333333333;
            (x | x << 1) & 0x5555555555555555
        }
        (spread(self.x) | (spread(self.y) << 1)) << (2 * (30 - self.level))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeNeighbor {
    pub address: CubePatchAddress,
    pub edge: PatchEdge,
    pub reversed: bool,
}
impl CubePatchAddress {
    pub fn try_new(face: CubeFace, level: u8, x: u32, y: u32) -> Result<Self, SurfaceMathError> {
        if level > 30 || x >= 1u32 << level || y >= 1u32 << level {
            return Err(SurfaceMathError::InvalidAddress);
        }
        Ok(Self { face, level, x, y })
    }
    pub fn root(face: CubeFace) -> Self {
        Self {
            face,
            level: 0,
            x: 0,
            y: 0,
        }
    }
    pub fn face(self) -> CubeFace {
        self.face
    }
    pub fn level(self) -> u8 {
        self.level
    }
    pub fn coordinates(self) -> [u32; 2] {
        [self.x, self.y]
    }
    pub fn parent(self) -> Option<Self> {
        (self.level > 0).then(|| Self {
            level: self.level - 1,
            x: self.x / 2,
            y: self.y / 2,
            ..self
        })
    }
    /// Lower left, lower right, upper left, upper right (dx + 2dy).
    pub fn children(self) -> Result<[Self; 4], SurfaceMathError> {
        if self.level == 30 {
            return Err(SurfaceMathError::InvalidAddress);
        }
        Ok(std::array::from_fn(|i| Self {
            level: self.level + 1,
            x: self.x * 2 + i as u32 % 2,
            y: self.y * 2 + i as u32 / 2,
            ..self
        }))
    }
    pub fn contains(self, other: Self) -> bool {
        self.face == other.face
            && self.level <= other.level
            && other.x >> (other.level - self.level) == self.x
            && other.y >> (other.level - self.level) == self.y
    }
    pub fn face_uv(self, st: [f64; 2]) -> Result<[f64; 2], SurfaceMathError> {
        if !st.iter().all(|x| x.is_finite() && (0.0..=1.0).contains(x)) {
            return Err(SurfaceMathError::InvalidDomain);
        }
        let scale = 2.0 / (1u64 << self.level) as f64;
        Ok([
            -1.0 + scale * (self.x as f64 + st[0]),
            -1.0 + scale * (self.y as f64 + st[1]),
        ])
    }
    pub fn patch_local(self, uv: [f64; 2]) -> Result<[f64; 2], SurfaceMathError> {
        let scale = (1u64 << self.level) as f64 * 0.5;
        let st = [
            (uv[0] + 1.0) * scale - self.x as f64,
            (uv[1] + 1.0) * scale - self.y as f64,
        ];
        self.face_uv(st)?;
        Ok(st)
    }
    /// Exact signed-axis transition construction. All dot products here are of
    /// integer cardinal axes; no direction matching/epsilon participates.
    pub fn neighbor(self, edge: PatchEdge) -> EdgeNeighbor {
        let count = 1u32 << self.level;
        let interior = match edge {
            PatchEdge::UMin if self.x > 0 => Some((self.x - 1, self.y)),
            PatchEdge::UMax if self.x + 1 < count => Some((self.x + 1, self.y)),
            PatchEdge::VMin if self.y > 0 => Some((self.x, self.y - 1)),
            PatchEdge::VMax if self.y + 1 < count => Some((self.x, self.y + 1)),
            _ => None,
        };
        if let Some((x, y)) = interior {
            return EdgeNeighbor {
                address: Self { x, y, ..self },
                edge: edge.opposite(),
                reversed: false,
            };
        }
        let [n, u, v] = self.face.basis();
        let (adjacent, tangent, k) = match edge {
            PatchEdge::UMin => (-u, v, self.y),
            PatchEdge::UMax => (u, v, self.y),
            PatchEdge::VMin => (-v, u, self.x),
            PatchEdge::VMax => (v, u, self.x),
        };
        let face = CubeFace::ALL
            .into_iter()
            .find(|f| f.basis()[0] == adjacent)
            .expect("cardinal face table is complete");
        let [_, nu, nv] = face.basis();
        let mapped_edge = if n.dot(nu) == -1.0 {
            PatchEdge::UMin
        } else if n.dot(nu) == 1.0 {
            PatchEdge::UMax
        } else if n.dot(nv) == -1.0 {
            PatchEdge::VMin
        } else {
            PatchEdge::VMax
        };
        let along = if matches!(mapped_edge, PatchEdge::UMin | PatchEdge::UMax) {
            nv
        } else {
            nu
        };
        let reversed = tangent.dot(along) == -1.0;
        let k = if reversed { count - 1 - k } else { k };
        let [x, y] = mapped_edge.grid(k, count - 1);
        EdgeNeighbor {
            address: Self { face, x, y, ..self },
            edge: mapped_edge,
            reversed,
        }
    }
    /// Canonical dyadic Cartesian cube key. Reduction occurs before conversion;
    /// seam/corner/sibling samples share identical keys and normalization order.
    pub fn sample_key(self, i: u32, j: u32, cells: u32) -> Result<CubeSampleKey, SurfaceMathError> {
        if !cells.is_power_of_two() || cells > 32 || i > cells || j > cells {
            return Err(SurfaceMathError::InvalidDomain);
        }
        let denominator = u64::from(cells) << self.level;
        let a =
            2 * (u64::from(cells) * u64::from(self.x) + u64::from(i)) as i64 - denominator as i64;
        let b =
            2 * (u64::from(cells) * u64::from(self.y) + u64::from(j)) as i64 - denominator as i64;
        let [n, u, v] = self.face.basis();
        let mut xyz = std::array::from_fn(|axis| {
            n[axis] as i64 * denominator as i64 + u[axis] as i64 * a + v[axis] as i64 * b
        });
        let mut exponent = denominator.trailing_zeros() as u8;
        while exponent > 0 && xyz.iter().all(|x| x % 2 == 0) {
            xyz = xyz.map(|x| x / 2);
            exponent -= 1;
        }
        Ok(CubeSampleKey { xyz, exponent })
    }
    pub fn sample_direction(
        self,
        i: u32,
        j: u32,
        cells: u32,
    ) -> Result<Direction3, SurfaceMathError> {
        Ok(self.sample_key(i, j, cells)?.direction())
    }
}
/// Temporary canonical boundary-sharing key, not a persistent location/object ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CubeSampleKey {
    xyz: [i64; 3],
    exponent: u8,
}
impl CubeSampleKey {
    /// Compact exact temporary sharing key. Encoding is intentionally not a
    /// persistence format: two signed 37-bit numerators, exponent and fixed axis.
    pub fn compact_key(self) -> u128 {
        let denominator = 1i64 << self.exponent;
        let axis = self
            .xyz
            .iter()
            .position(|x| x.abs() == denominator)
            .expect("cube has a fixed axis");
        let fixed = axis * 2 + usize::from(self.xyz[axis] < 0);
        let mut varying = self
            .xyz
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i != axis)
            .map(|(_, x)| x);
        let mask = (1u128 << 37) - 1;
        (varying.next().expect("first varying axis") as u128 & mask)
            | ((varying.next().expect("second varying axis") as u128 & mask) << 37)
            | (u128::from(self.exponent) << 74)
            | ((fixed as u128) << 80)
    }
    pub fn direction(self) -> Direction3 {
        let denominator = (1u64 << self.exponent) as f64;
        Direction3::try_new(DVec3::from_array(self.xyz.map(|x| x as f64 / denominator)))
            .expect("nonzero bounded cube tuple")
    }
}

/// Body-fixed spatial coordinate. No Hash/Eq encoding or persistence semantics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceLocation(Direction3);
impl SurfaceLocation {
    pub fn new(direction: Direction3) -> Self {
        Self(direction)
    }
    pub fn direction(self) -> Direction3 {
        self.0
    }
    /// Largest absolute component, ties X then Y then Z. Geometric boundaries
    /// remain closed while this point-query assignment chooses a single chart.
    pub fn face_uv(self) -> (CubeFace, [f64; 2]) {
        let p = self.0.unit();
        let a = p.abs();
        let axis = if a.x >= a.y && a.x >= a.z {
            0
        } else if a.y >= a.z {
            1
        } else {
            2
        };
        let face = CubeFace::ALL[axis * 2 + usize::from(p[axis] < 0.0)];
        let [n, u, v] = face.basis();
        (face, [p.dot(u) / p.dot(n), p.dot(v) / p.dot(n)])
    }
}

/// E × N = up. Camera/regional columns [east, up, -north] are right-handed.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceTangentBasis {
    east: Direction3,
    up: Direction3,
    north: Direction3,
}
impl SurfaceTangentBasis {
    pub fn new(up: Direction3) -> Self {
        let n = up.unit();
        let mut axis = DVec3::Y;
        if axis.cross(n).length() < 1e-6 {
            axis = [DVec3::X, DVec3::Y, DVec3::Z]
                .into_iter()
                .min_by(|a, b| a.dot(n).abs().total_cmp(&b.dot(n).abs()))
                .expect("three axes");
        }
        Self::with_east(up, axis.cross(n))
    }
    fn with_east(up: Direction3, east: DVec3) -> Self {
        let east = Direction3::try_new(east).expect("conditioned tangent");
        let north = Direction3::try_new(up.unit().cross(east.unit())).expect("orthogonal tangent");
        Self { east, up, north }
    }
    pub fn transported(self, up: Direction3) -> Self {
        let east = self.east.unit() - up.unit() * self.east.unit().dot(up.unit());
        if east.length() < 1e-6 {
            Self::new(up)
        } else {
            Self::with_east(up, east)
        }
    }
    pub fn east(self) -> Direction3 {
        self.east
    }
    pub fn up(self) -> Direction3 {
        self.up
    }
    pub fn north(self) -> Direction3 {
        self.north
    }
}
