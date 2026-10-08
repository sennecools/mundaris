//! Immutable prepared geological sources, sampled in body-local coordinates.
//! Loading validates published bytes; neither loading nor sampling rebakes geology.
use glam::DVec3;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Logical retained source cap; loader scratch and allocator overhead are separate.
pub const PREPARED_SOURCE_CAP_BYTES: usize = 8 * 1024 * 1024;
const METADATA_CAP: usize = 64 * 1024;
/// Operator version named by the manifest; the control bundle carries its own.
const ALGORITHM: &str = "mundaris.prepared-composition/2";
const CONTROL_VERSION: &str = "mundaris.prepared-composition/1";

#[derive(Debug, thiserror::Error)]
pub enum PreparedError {
    #[error("prepared source I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("prepared source metadata: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid prepared source: {0}")]
    Invalid(&'static str),
}
type Result<T> = std::result::Result<T, PreparedError>;
fn check(ok: bool, name: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(PreparedError::Invalid(name))
    }
}
fn number(v: &Value, key: &'static str) -> Result<f64> {
    v[key]
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or(PreparedError::Invalid(key))
}
fn integer(v: &Value, key: &'static str) -> Result<usize> {
    v[key]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(PreparedError::Invalid(key))
}
fn read(path: &Path, cap: usize) -> Result<Vec<u8>> {
    check(fs::metadata(path)?.len() <= cap as u64, "file byte cap")?;
    // A bounded reader also handles a file growing after metadata inspection.
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes)?;
    check(bytes.len() <= cap, "file grew beyond cap")?;
    Ok(bytes)
}
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn valid_sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn local(root: &Path, name: &str) -> Result<PathBuf> {
    let relative = Path::new(name);
    check(
        !relative.is_absolute()
            && relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "relative asset path",
    )?;
    let path = root.join(relative).canonicalize()?;
    check(
        path.starts_with(root.canonicalize()?),
        "asset escapes content root",
    )?;
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRef {
    path: String,
    content_sha256: String,
    metadata_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    identity: String,
    selector: usize,
    center: [f64; 3],
    tangent_x: [f64; 3],
    horizontal_scale: f64,
    height_scale: f64,
    amplitude: f64,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    algorithm: String,
    body_identity: String,
    revision: u64,
    control: SourceRef,
    profiles: [SourceRef; 2],
    placements: Vec<Placement>,
    material_binding: [String; 3],
    material_mapping: [[f64; 4]; 3],
}

fn metadata(root: &Path, source: &SourceRef) -> Result<(PathBuf, Value)> {
    check(
        valid_sha(&source.metadata_sha256) && valid_sha(&source.content_sha256),
        "source SHA256",
    )?;
    let directory = local(root, &source.path)?;
    let raw = read(&directory.join("metadata.json"), METADATA_CAP)?;
    check(sha(&raw) == source.metadata_sha256, "pinned metadata hash")?;
    let v: Value = serde_json::from_slice(&raw)?;
    check(
        v["schema_version"].as_u64() == Some(1)
            && v["content_sha256"].as_str() == Some(&source.content_sha256),
        "pinned source identity/version",
    )?;
    Ok((directory, v))
}
// Stable Rust cannot use the generic expression C * 4 in as_chunks' const argument.
#[allow(clippy::chunks_exact_to_as_chunks)]
fn floats<const C: usize>(
    directory: &Path,
    descriptor: &Value,
    name: &str,
    count: usize,
) -> Result<Vec<[f32; C]>> {
    check(
        descriptor["file"].as_str() == Some(name),
        "payload path/layout",
    )?;
    let length = count
        .checked_mul(C)
        .and_then(|n| n.checked_mul(4))
        .ok_or(PreparedError::Invalid("payload size overflow"))?;
    check(
        length <= PREPARED_SOURCE_CAP_BYTES && descriptor["bytes"].as_u64() == Some(length as u64),
        "payload byte count",
    )?;
    let bytes = read(&directory.join(name), length)?;
    check(
        bytes.len() == length && descriptor["sha256"].as_str() == Some(&sha(&bytes)),
        "payload integrity",
    )?;
    let mut out = Vec::with_capacity(count);
    for cell in bytes.chunks_exact(C * 4) {
        let mut channels = [0.; C];
        for (i, value) in channels.iter_mut().enumerate() {
            *value = f32::from_le_bytes([
                cell[4 * i],
                cell[4 * i + 1],
                cell[4 * i + 2],
                cell[4 * i + 3],
            ]);
        }
        check(channels.iter().all(|v| v.is_finite()), "finite payload")?;
        out.push(channels);
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
struct Profile {
    fields: Vec<[f32; 10]>,
    n: usize,
    spacing: f64,
    footprint: f64,
    bounds: [f64; 2],
}
impl Profile {
    fn load(root: &Path, source: &SourceRef, kind: &str) -> Result<Self> {
        let (directory, v) = metadata(root, source)?;
        check(
            v["generator"].as_str() == Some("mundaris.compact-crater/1"),
            "profile generator version",
        )?;
        check(
            v["recipe"]["profile"].as_str() == Some(kind) && v["halo_cells"].as_u64() == Some(1),
            "profile kind/halo",
        )?;
        check(
            v["channels"]
                == serde_json::json!([
                    "height_m",
                    "support",
                    "rim",
                    "ejecta",
                    "ground_weight",
                    "rock_weight",
                    "ejecta_weight",
                    "normal_x",
                    "normal_y",
                    "normal_z"
                ]),
            "profile channel semantics",
        )?;
        check(
            v["rules"]["encoding"].as_str()
                == Some("linear little-endian float32; C-order [row,column,channel]")
                && v["rules"]["height_units"].as_str()
                    == Some("signed metres; zero offset; positive outward")
                && v["rules"]["coordinate_basis"].as_str()
                    == Some("right-handed local X,Y horizontal; +Z outward; row increases +Y")
                && v["rules"]["normal_basis"].as_str()
                    == Some(
                        "XYZ local; normalize(-dh/dx,-dh/dy,1); central differences of stored level height",
                    ),
            "profile units/basis",
        )?;
        let n = integer(&v["recipe"], "resolution")?;
        check(
            (16..=256).contains(&n) && n.is_power_of_two(),
            "bounded profile resolution",
        )?;
        let footprint = number(&v["recipe"], "footprint_m")?;
        let support = number(&v, "support_radius_m")?;
        check(
            (1.0..=1e6).contains(&footprint) && (0.0..footprint * 0.45).contains(&support),
            "profile footprint/support",
        )?;
        let level = &v["levels"][0];
        let spacing = footprint / n as f64;
        check(
            integer(level, "resolution")? == n && number(level, "spacing_m")? == spacing,
            "physical profile spacing",
        )?;
        let fields = floats::<10>(
            &directory,
            &level["fields"],
            "level-0-fields.f32",
            (n + 2) * (n + 2),
        )?;
        let bounds = floats::<2>(
            &directory,
            &level["bounds"],
            "level-0-bounds.f32",
            (n + 2) * (n + 2),
        )?;
        let lo = v["analytic_height_bound_m"][0]
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or(PreparedError::Invalid("height lower bound"))?;
        let hi = v["analytic_height_bound_m"][1]
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or(PreparedError::Invalid("height upper bound"))?;
        let envelope = [(lo as f32).next_down() as f64, (hi as f32).next_up() as f64];
        check(
            lo <= hi && lo.abs() < 1e6 && hi.abs() < 1e6,
            "height bounds",
        )?;
        let flat = [0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];
        for y in 0..n + 2 {
            for x in 0..n + 2 {
                let i = y * (n + 2) + x;
                let f = fields[i];
                let b = bounds[i];
                check(
                    f[1..7].iter().all(|v| (0.0..=1.0).contains(v))
                        && (f[4] + f[5] + f[6] - 1.).abs() < 3e-7,
                    "profile masks/weights",
                )?;
                check(
                    b[0] <= f[0]
                        && f[0] <= b[1]
                        && (f[0] as f64) >= envelope[0]
                        && (f[0] as f64) <= envelope[1],
                    "profile extrema containment",
                )?;
                check(
                    (DVec3::new(f[7] as f64, f[8] as f64, f[9] as f64).length() - 1.).abs() < 2e-7,
                    "profile normal length",
                )?;
                if kind == "flat" || x < 2 || y < 2 || x >= n || y >= n {
                    check(f == flat, "profile flat exterior/halo")?;
                }
                if x > 0 && y > 0 && x < n + 1 && y < n + 1 {
                    let dx = (fields[i + 1][0] as f64 - fields[i - 1][0] as f64) / (2. * spacing);
                    let dy = (fields[i + n + 2][0] as f64 - fields[i - n - 2][0] as f64)
                        / (2. * spacing);
                    let expected = DVec3::new(-dx, -dy, 1.).normalize();
                    check(
                        (expected - DVec3::new(f[7] as f64, f[8] as f64, f[9] as f64)).length()
                            < 1e-7,
                        "profile height/normal agreement",
                    )?;
                }
            }
        }
        Ok(Self {
            fields,
            n,
            spacing,
            footprint,
            bounds: envelope,
        })
    }
    fn sample(&self, x: f64, y: f64) -> ([f64; 7], [[f64; 2]; 7]) {
        if x.abs() >= self.footprint / 2. || y.abs() >= self.footprint / 2. {
            return ([0., 0., 0., 0., 1., 0., 0.], [[0.; 2]; 7]);
        }
        let u = (x + self.footprint / 2.) / self.spacing + 0.5;
        let v = (y + self.footprint / 2.) / self.spacing + 0.5;
        let ix = u.floor() as usize;
        let iy = v.floor() as usize;
        let fx = u - ix as f64;
        let fy = v - iy as f64;
        let mut values = [0.; 7];
        let mut gradient = [[0.; 2]; 7];
        for yy in 0..2 {
            for xx in 0..2 {
                let f = self.fields[(iy + yy) * (self.n + 2) + ix + xx];
                let wx = if xx == 0 { 1. - fx } else { fx };
                let wy = if yy == 0 { 1. - fy } else { fy };
                for c in 0..7 {
                    values[c] += f[c] as f64 * wx * wy;
                    gradient[c][0] += f[c] as f64
                        * if xx == 0 {
                            -wy / self.spacing
                        } else {
                            wy / self.spacing
                        };
                    gradient[c][1] += f[c] as f64
                        * if yy == 0 {
                            -wx / self.spacing
                        } else {
                            wx / self.spacing
                        };
                }
            }
        }
        (values, gradient)
    }
}

/// Immutable source definition and validated linear assets. Clones in world and
/// workers share it through Arc; equality includes the full definition/data.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedSurface {
    manifest: Manifest,
    profiles: [Profile; 2],
    controls: Vec<[f32; 3]>,
    control_n: usize,
    bounds: [f64; 2],
    identity: [u8; 32],
    definition_words: Vec<u64>,
}
/// Complete physical query; gradient is metres per unit body-local direction.
#[derive(Debug, Clone, Copy)]
pub struct PreparedSample {
    pub height_m: f64,
    pub gradient_m: DVec3,
    pub weights: [f64; 4],
}
impl PreparedSurface {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().canonicalize()?;
        let root = path
            .parent()
            .ok_or(PreparedError::Invalid("content root"))?;
        let raw = read(&path, METADATA_CAP)?;
        let manifest: Manifest = serde_json::from_slice(&raw)?;
        check(
            manifest.schema_version == 2
                && manifest.algorithm == ALGORITHM
                && manifest.revision > 0
                && !manifest.body_identity.is_empty()
                && manifest.body_identity.len() <= 128,
            "prepared definition version/identity",
        )?;
        check(
            manifest.material_binding == ["substrate", "rim_rock", "ejecta_deposit"],
            "material roles",
        )?;
        for row in manifest.material_mapping {
            check(
                row.iter().all(|w| w.is_finite() && (0.0..=1.0).contains(w))
                    && (row.iter().sum::<f64>() - 1.).abs() < 1e-12,
                "authored material mapping",
            )?;
        }
        check(
            (1..=8).contains(&manifest.placements.len()),
            "placement cap",
        )?;
        for (i, p) in manifest.placements.iter().enumerate() {
            let c = DVec3::from_array(p.center);
            let x = DVec3::from_array(p.tangent_x);
            check(
                !p.identity.is_empty()
                    && p.identity.len() <= 128
                    && manifest.placements[..i]
                        .iter()
                        .all(|q| q.identity != p.identity)
                    && p.selector < 2,
                "placement identity/selector",
            )?;
            check(
                c.is_finite()
                    && x.is_finite()
                    && (c.length() - 1.).abs() < 1e-10
                    && (x.length() - 1.).abs() < 1e-10
                    && c.dot(x).abs() < 1e-10,
                "placement basis",
            )?;
            check(
                [p.horizontal_scale, p.height_scale]
                    .iter()
                    .all(|s| s.is_finite() && (0.01..=100.).contains(s))
                    && p.amplitude.is_finite()
                    && p.amplitude > 0.
                    && p.amplitude <= 1.,
                "placement scale/amplitude",
            )?;
        }
        let (directory, v) = metadata(root, &manifest.control)?;
        check(
            v["generator"].as_str() == Some(CONTROL_VERSION)
                && v["rules"]["address"].as_str()
                    == Some(
                        "unit body-local XYZ direction; trilinear cube [-1,1]^3; independent of rendering face",
                    )
                && v["rules"]["layout"].as_str()
                    == Some("little-endian float32 C-order [z,y,x,channel]")
                && v["rules"]["channels"]
                    == serde_json::json!(["base_height_m", "flat_preference", "crater_preference"]),
            "control semantics",
        )?;
        let n = integer(&v["recipe"], "resolution")?;
        check(
            (3..=33).contains(&n) && n % 2 == 1 && v["shape"] == serde_json::json!([n, n, n, 3]),
            "control dimensions",
        )?;
        let controls = floats::<3>(&directory, &v["fields"], "controls.f32", n * n * n)?;
        let mut base = [f64::INFINITY, f64::NEG_INFINITY];
        for c in &controls {
            check(
                c[1] >= 0.2 - 1e-7
                    && c[2] >= 0.
                    && c[2] <= 0.8 + 1e-7
                    && (c[1] + c[2] - 1.).abs() < 2e-7,
                "control selector weights",
            )?;
            base[0] = base[0].min(c[0] as f64);
            base[1] = base[1].max(c[0] as f64);
        }
        check(
            v["base_bounds_m"] == serde_json::json!(base),
            "global base bounds",
        )?;
        let profiles = [
            Profile::load(root, &manifest.profiles[0], "flat")?,
            Profile::load(root, &manifest.profiles[1], "crater")?,
        ];
        let mut low = 0f64;
        let mut high = 0f64;
        // Height adds linearly, so any overlap stays inside the sum of the
        // per-placement extrema; a profile that never rises or dips adds nothing.
        for p in &manifest.placements {
            let k = p.amplitude * p.height_scale;
            let [lo, hi] = profiles[p.selector].bounds;
            low += k * lo.min(0.);
            high += k * hi.max(0.);
        }
        let roundoff = 2048.
            * f64::EPSILON
            * (1. + base[0].abs().max(base[1].abs()) + low.abs().max(high.abs()));
        let bounds = [
            (base[0] + low - roundoff).next_down(),
            (base[1] + high + roundoff).next_up(),
        ];
        let mut hasher = Sha256::new();
        // The hash does not fingerprint compiled code: any change to the operator
        // arithmetic in `sample` or the bounds above must bump this prefix.
        hasher.update(b"mundaris.prepared-native/2\0");
        hasher.update(&raw);
        // Full authored JSON is retained in exact keys; immutable external
        // sources are pinned by full metadata/content/payload SHA256 identities.
        let mut definition_words = vec![1, raw.len() as u64];
        for chunk in raw.chunks(8) {
            let mut bytes = [0; 8];
            bytes[..chunk.len()].copy_from_slice(chunk);
            definition_words.push(u64::from_le_bytes(bytes));
        }
        let identity: [u8; 32] = hasher.finalize().into();
        let surface = Self {
            manifest,
            profiles,
            controls,
            control_n: n,
            bounds,
            identity,
            definition_words,
        };
        check(
            surface.resident_bytes() <= PREPARED_SOURCE_CAP_BYTES,
            "retained prepared source cap",
        )?;
        Ok(surface)
    }
    pub fn validate_radius(&self, radius: f64) -> Result<()> {
        check(
            radius.is_finite()
                && radius + self.bounds[0] > 0.001
                && self.manifest.placements.iter().all(|p| {
                    self.profiles[p.selector].footprint * p.horizontal_scale < radius * 0.25
                }),
            "prepared radius/footprint",
        )
    }
    pub fn content_identity(&self) -> [u8; 32] {
        self.identity
    }
    pub fn exact_definition_words(&self) -> &[u64] {
        &self.definition_words
    }
    pub fn displacement_bounds_m(&self) -> [f64; 2] {
        self.bounds
    }
    pub fn revision(&self) -> u64 {
        self.manifest.revision
    }
    pub fn resident_bytes(&self) -> usize {
        self.profiles
            .iter()
            .map(|p| p.fields.capacity() * std::mem::size_of::<[f32; 10]>())
            .sum::<usize>()
            + self.controls.capacity() * 12
            + self.definition_words.capacity() * 8
            + std::mem::size_of::<Self>()
            + self.manifest.placements.capacity() * std::mem::size_of::<Placement>()
            + self
                .manifest
                .placements
                .iter()
                .map(|p| p.identity.capacity())
                .sum::<usize>()
            + self.manifest.algorithm.capacity()
            + self.manifest.body_identity.capacity()
            + self
                .manifest
                .material_binding
                .iter()
                .map(String::capacity)
                .sum::<usize>()
            + std::iter::once(&self.manifest.control)
                .chain(self.manifest.profiles.iter())
                .map(|s| {
                    s.path.capacity() + s.content_sha256.capacity() + s.metadata_sha256.capacity()
                })
                .sum::<usize>()
    }
    fn control(&self, n: DVec3) -> ([f64; 3], [DVec3; 3]) {
        let scale = (self.control_n - 1) as f64 / 2.;
        let u = ((n + DVec3::ONE) * scale)
            .clamp(DVec3::ZERO, DVec3::splat((self.control_n - 1) as f64));
        let ix = (u.x.floor() as usize).min(self.control_n - 2);
        let iy = (u.y.floor() as usize).min(self.control_n - 2);
        let iz = (u.z.floor() as usize).min(self.control_n - 2);
        let f = [u.x - ix as f64, u.y - iy as f64, u.z - iz as f64];
        let mut value = [0.; 3];
        let mut gradient = [DVec3::ZERO; 3];
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    let bit = [x, y, z];
                    let w =
                        std::array::from_fn::<_, 3, _>(
                            |i| if bit[i] == 0 { 1. - f[i] } else { f[i] },
                        );
                    let a = self.controls
                        [((iz + z) * self.control_n + iy + y) * self.control_n + ix + x];
                    let dw = DVec3::new(
                        if x == 0 {
                            -scale * w[1] * w[2]
                        } else {
                            scale * w[1] * w[2]
                        },
                        if y == 0 {
                            -scale * w[0] * w[2]
                        } else {
                            scale * w[0] * w[2]
                        },
                        if z == 0 {
                            -scale * w[0] * w[1]
                        } else {
                            scale * w[0] * w[1]
                        },
                    );
                    for c in 0..3 {
                        value[c] += a[c] as f64 * w[0] * w[1] * w[2];
                        gradient[c] += dw * (a[c] as f64);
                    }
                }
            }
        }
        (value, gradient)
    }
    /// Ordered additive composition: profile heights add onto the base without
    /// normalisation, materials composite in authored order over the substrate.
    pub fn sample(&self, n: DVec3, radius: f64) -> Result<PreparedSample> {
        check(
            n.is_finite() && (n.length() - 1.).abs() < 1e-10,
            "unit body-local query",
        )?;
        self.validate_radius(radius)?;
        let (control, dc) = self.control(n);
        let mut h = control[0];
        let mut gradient = dc[0];
        let mut roles = [1., 0., 0.];
        for p in &self.manifest.placements {
            let c = DVec3::from_array(p.center);
            if n.dot(c) <= 0.5 {
                continue;
            }
            let tx = DVec3::from_array(p.tangent_x);
            let ty = c.cross(tx);
            let scale = radius / p.horizontal_scale;
            let x = scale * n.dot(tx);
            let y = scale * n.dot(ty);
            let (v, d) = self.profiles[p.selector].sample(x, y);
            let k = p.amplitude * p.height_scale;
            h += k * v[0];
            gradient += (tx * d[0][0] + ty * d[0][1]) * (k * scale);
            let alpha = p.amplitude * v[1].clamp(0., 1.);
            for (r, w) in roles.iter_mut().zip(&v[4..7]) {
                *r = (1. - alpha) * *r + alpha * w;
            }
        }
        let mut weights = [0.; 4];
        for (r, row) in roles.into_iter().zip(self.manifest.material_mapping) {
            for (w, a) in weights.iter_mut().zip(row) {
                *w += r * a;
            }
        }
        let sum = weights.iter().sum::<f64>();
        weights = weights.map(|w| w / sum);
        Ok(PreparedSample {
            height_m: h,
            gradient_m: gradient - n * gradient.dot(n),
            weights,
        })
    }
}
