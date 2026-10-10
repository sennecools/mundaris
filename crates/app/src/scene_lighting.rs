//! Authored scene light and surface reflectance (docs/RENDER_PIPELINE_HDR.md §3).
//!
//! Loaded from `content/lighting.json` beside the system content. The sun is a
//! body of the system; its illuminance is authored at a reference distance and
//! falls off with the inverse square, so the scaled test system stays
//! physically ordered without real solar luminosity.
use anyhow::{Context, Result, ensure};
use astrum_renderer::{Atmosphere, Brdf, Grade, StylisedLook, SurfaceMaterial};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub const LIGHTING_FILE: &str = "lighting.json";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LightingContent {
    schema: u32,
    sun: SunContent,
    ambient: AmbientContent,
    bodies: BTreeMap<String, MaterialContent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SunContent {
    body: String,
    illuminance_lux: f64,
    reference_distance_m: f64,
    color: [f32; 3],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AmbientContent {
    illuminance_lux: f64,
    color: [f32; 3],
    bounce_fraction: f64,
    /// Sky light at high sun as a fraction of the sun's illuminance (0 = airless).
    #[serde(default)]
    sky_fraction: f64,
    #[serde(default = "white")]
    sky_color: [f32; 3],
}

fn white() -> [f32; 3] {
    [1.0; 3]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialContent {
    albedo: [f32; 3],
    brdf: String,
    /// Optional atmosphere drawn as sky and aerial perspective.
    #[serde(default)]
    atmosphere: Option<AtmosphereContent>,
    /// Optional stylised look of the planet (any field omitted keeps the
    /// generic stylised value).
    #[serde(default)]
    look: Option<LookContent>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LookContent {
    sky_tint: Option<[f32; 3]>,
    sky_saturation: Option<f32>,
    haze: Option<f32>,
    sky: Option<f32>,
    shadow_sky: Option<f32>,
    bloom: Option<f32>,
    grade_lift: Option<[f32; 3]>,
    grade_gain: Option<[f32; 3]>,
    grade_saturation: Option<f32>,
    grade_contrast: Option<f32>,
}

impl LookContent {
    fn resolve(&self) -> StylisedLook {
        let d = StylisedLook::default();
        StylisedLook {
            sky_tint: self.sky_tint.unwrap_or(d.sky_tint),
            sky_saturation: self.sky_saturation.unwrap_or(d.sky_saturation),
            haze: self.haze.unwrap_or(d.haze),
            sky: self.sky.unwrap_or(d.sky),
            shadow_sky: self.shadow_sky.unwrap_or(d.shadow_sky),
            bloom: self.bloom.unwrap_or(d.bloom),
            grade: Grade {
                lift: self.grade_lift.unwrap_or(d.grade.lift),
                gain: self.grade_gain.unwrap_or(d.grade.gain),
                saturation: self.grade_saturation.unwrap_or(d.grade.saturation),
                contrast: self.grade_contrast.unwrap_or(d.grade.contrast),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AtmosphereContent {
    top_height_m: f64,
    rayleigh_per_m: [f32; 3],
    rayleigh_scale_height_m: f64,
    mie_per_m: f32,
    mie_scale_height_m: f64,
    mie_g: f32,
    #[serde(default)]
    clouds: Option<CloudsContent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudsContent {
    altitude_m: f64,
    coverage: f32,
    scale_m: f64,
    opacity: f32,
}

/// Validated scene light; colours are normalised chromaticities.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneLighting {
    pub sun_body: String,
    pub illuminance_lux: f64,
    pub reference_distance_m: f64,
    pub sun_color: [f32; 3],
    pub ambient_lux: f64,
    pub ambient_color: [f32; 3],
    pub bounce_fraction: f64,
    pub sky_fraction: f64,
    pub sky_color: [f32; 3],
    materials: BTreeMap<String, SurfaceMaterial>,
    atmospheres: BTreeMap<String, Atmosphere>,
    looks: BTreeMap<String, StylisedLook>,
    pub sha256: String,
}

fn chromaticity(color: [f32; 3], what: &str) -> Result<[f32; 3]> {
    let max = color.iter().copied().fold(0.0f32, f32::max);
    ensure!(
        color.iter().all(|c| c.is_finite() && *c >= 0.0) && max > 0.0,
        "{what} colour must be finite, non-negative and non-black"
    );
    Ok(color.map(|c| c / max))
}

impl SceneLighting {
    pub fn load_canonical() -> Result<Self> {
        Self::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content"))
    }

    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(LIGHTING_FILE);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let content: LightingContent = serde_json::from_slice(bytes)?;
        ensure!(content.schema == 1, "unsupported lighting schema");
        let sun = &content.sun;
        ensure!(
            sun.illuminance_lux.is_finite() && sun.illuminance_lux >= 0.0,
            "sun illuminance must be finite and non-negative"
        );
        ensure!(
            sun.reference_distance_m.is_finite() && sun.reference_distance_m > 0.0,
            "sun reference distance must be positive"
        );
        let ambient = &content.ambient;
        ensure!(
            ambient.illuminance_lux.is_finite() && ambient.illuminance_lux >= 0.0,
            "ambient illuminance must be finite and non-negative"
        );
        ensure!(
            (0.0..=1.0).contains(&ambient.bounce_fraction),
            "bounce fraction must be within 0..1"
        );
        ensure!(
            (0.0..=1.0).contains(&ambient.sky_fraction),
            "sky fraction must be within 0..1"
        );
        let mut materials = BTreeMap::new();
        let mut atmospheres = BTreeMap::new();
        let mut looks = BTreeMap::new();
        for (body, content_material) in &content.bodies {
            let material = content_material;
            let brdf = Brdf::from_name(&material.brdf)
                .with_context(|| format!("{body}: unknown BRDF {}", material.brdf))?;
            let material = SurfaceMaterial {
                albedo: material.albedo,
                brdf,
            };
            ensure!(material.validate(), "{body}: albedo must be within 0..1");
            materials.insert(body.clone(), material);
            if let Some(a) = &content_material.atmosphere {
                let atmosphere = Atmosphere {
                    top_height_m: a.top_height_m,
                    rayleigh_per_m: a.rayleigh_per_m,
                    rayleigh_scale_height_m: a.rayleigh_scale_height_m,
                    mie_per_m: a.mie_per_m,
                    mie_scale_height_m: a.mie_scale_height_m,
                    mie_g: a.mie_g,
                    clouds: a.clouds.as_ref().map(|c| astrum_renderer::Clouds {
                        altitude_m: c.altitude_m,
                        coverage: c.coverage,
                        scale_m: c.scale_m,
                        opacity: c.opacity,
                    }),
                };
                ensure!(atmosphere.validate(), "{body}: invalid atmosphere");
                atmospheres.insert(body.clone(), atmosphere);
            }
            if let Some(look) = &content_material.look {
                let look = look.resolve();
                ensure!(look.validate(), "{body}: invalid look");
                looks.insert(body.clone(), look);
            }
        }
        Ok(Self {
            sun_body: sun.body.clone(),
            illuminance_lux: sun.illuminance_lux,
            reference_distance_m: sun.reference_distance_m,
            sun_color: chromaticity(sun.color, "sun")?,
            ambient_lux: ambient.illuminance_lux,
            ambient_color: chromaticity(ambient.color, "ambient")?,
            bounce_fraction: ambient.bounce_fraction,
            sky_fraction: ambient.sky_fraction,
            sky_color: chromaticity(ambient.sky_color, "sky")?,
            materials,
            atmospheres,
            looks,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }

    /// Stylised look of a body (authored, else the generic stylised look).
    pub fn look(&self, semantic_id: &str) -> StylisedLook {
        self.looks.get(semantic_id).copied().unwrap_or_default()
    }

    /// Authored atmosphere of a body, if it has one.
    pub fn atmosphere(&self, semantic_id: &str) -> Option<Atmosphere> {
        self.atmospheres.get(semantic_id).copied()
    }

    /// Authored material of a body, or a neutral 18 % grey.
    pub fn material(&self, semantic_id: &str) -> SurfaceMaterial {
        self.materials.get(semantic_id).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_lighting_loads_and_names_the_star() {
        let lighting = SceneLighting::load_canonical().unwrap();
        assert_eq!(lighting.sun_body, "sol");
        assert!(lighting.illuminance_lux > 1000.0);
        assert_eq!(lighting.material("moon").brdf, Brdf::LunarLambert);
        assert_eq!(lighting.material("unknown"), SurfaceMaterial::default());
        assert!(lighting.sun_color.contains(&1.0));
    }

    #[test]
    fn invalid_lighting_is_rejected() {
        let base = r#"{"schema":1,"sun":{"body":"sol","illuminance_lux":1.0,"reference_distance_m":1.0,"color":[1,1,1]},
            "ambient":{"illuminance_lux":0.0,"color":[1,1,1],"bounce_fraction":0.0},"bodies":{BODIES}}"#;
        assert!(SceneLighting::parse(base.replace("BODIES", "").as_bytes()).is_ok());
        for bodies in [
            r#""moon":{"albedo":[2,0,0],"brdf":"lambert"}"#,
            r#""moon":{"albedo":[0.1,0.1,0.1],"brdf":"phong"}"#,
        ] {
            assert!(SceneLighting::parse(base.replace("BODIES", bodies).as_bytes()).is_err());
        }
        assert!(SceneLighting::parse(br#"{"schema":2}"#).is_err());
    }
}
