//! Versioned, bounded terrain authoring graphs.
use crate::{Error, cube_direction};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{self, Write};

pub const GRAPH_SCHEMA_VERSION: u32 = 2;
pub const MAX_GRAPH_NODES: usize = 128;
pub const MAX_GRAPH_BYTES: usize = 256 * 1024;
pub const GRAPH_EVALUATOR_VERSION: &str = "typed-graph-evaluator-2";
pub const DIFFERENTIAL_METHOD_VERSION: &str = "centered-tangent-secant-scale-aware-v2";
const MIN_FEATURE_ANGULAR_RATIO: f64 = 1.0e-8;
const MIN_DIFFERENTIAL_STEP_RAD: f64 = 1.0e-11;
const MAX_DIFFERENTIAL_STEP_RAD: f64 = 1.0e-5;
pub const MAX_JS_SAFE_SEED: u64 = 9_007_199_254_740_991;
pub const MAX_PREVIEW_WORK: u64 = 20_000_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub schema_version: u32,
    pub recipe_id: String,
    pub reference_radius_m: f64,
    pub seeds: Seeds,
    pub nodes: Vec<Node>,
    pub outputs: Outputs,
    #[serde(default)]
    pub layout: BTreeMap<String, LayoutPoint>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Seeds {
    pub geometry: u64,
    pub climate: u64,
    pub material: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Outputs {
    pub height: String,
    pub humidity: String,
    pub temperature: String,
    pub material: String,
    #[serde(default)]
    pub support: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Meters,
    Normalized,
    Kelvin,
    Material,
}
impl Unit {
    fn code(self) -> &'static str {
        match self {
            Self::Meters => "m",
            Self::Normalized => "1",
            Self::Kelvin => "K",
            Self::Material => "material",
        }
    }
}
#[derive(Clone, Copy, Debug)]
struct Type {
    unit: Unit,
    lo: f64,
    hi: f64,
}
#[derive(Clone, Debug)]
struct CompiledNode {
    node: Node,
    output: Type,
    input_indexes: [Option<usize>; 3],
}
#[derive(Clone, Debug)]
pub struct CompiledGraph {
    graph: Graph,
    order: Vec<CompiledNode>,
    indexes: HashMap<String, usize>,
    identities: Identities,
    work_units_per_query: u64,
    output_indexes: [usize; 4],
    support_index: Option<usize>,
    palette: [[f64; 3]; 3],
    height_bounds_m: [f64; 2],
    differential_step_rad: f64,
    minimum_feature_angular_ratio: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identities {
    pub geometry: String,
    pub climate: String,
    pub material: String,
    pub palette: String,
    pub graph: String,
}
#[derive(Clone, Copy, Debug)]
enum ValueOut {
    Scalar(f64),
    Material([f64; 3]),
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Evaluated {
    pub height_m: f64,
    pub humidity: f64,
    pub temperature_k: f64,
    pub material: [f64; 3],
    pub weights: [f64; 3],
    pub color: [f64; 3],
    pub support: f64,
    pub node_values: BTreeMap<String, f64>,
}
/// Small authoritative output without the inspection-only per-node map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceEvaluation {
    pub height_m: f64,
    pub humidity: f64,
    pub temperature_k: f64,
    pub weights: [f64; 3],
    pub support: f64,
}
/// Tangent height derivative and outward radial-surface normal in body space.
/// The derivative is a centered secant in metres per radian with a graph-specific validated step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceDifferential {
    pub height_gradient_tangent_m_per_radian: [f64; 3],
    pub outward_normal: [f64; 3],
}

struct BoundedSizeWriter(usize);
impl Write for BoundedSizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.saturating_add(bytes.len()) > MAX_GRAPH_BYTES {
            return Err(io::Error::other("graph exceeds bounded serialized size"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn validate_serialized_size(graph: &Graph) -> Result<(), Error> {
    let mut writer = BoundedSizeWriter(0);
    serde_json::to_writer(&mut writer, graph).map_err(|error| {
        if writer.0 >= MAX_GRAPH_BYTES || error.to_string().contains("bounded serialized size") {
            invalid(format!("serialized graph exceeds {MAX_GRAPH_BYTES} bytes"))
        } else {
            Error::from(error)
        }
    })
}

fn normalize_direction(direction: [f64; 3]) -> Result<[f64; 3], Error> {
    if direction.iter().any(|value| !value.is_finite()) {
        return Err(invalid("direction must be finite and nonzero"));
    }
    let length = direction[0].hypot(direction[1]).hypot(direction[2]);
    if !length.is_finite() || length <= f64::MIN_POSITIVE {
        return Err(invalid("direction must be finite and nonzero"));
    }
    Ok(direction.map(|value| value / length))
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
fn num(n: &Node, k: &str) -> Result<f64, Error> {
    n.params
        .get(k)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| {
            invalid(format!(
                "node {} requires finite numeric parameter {k}",
                n.id
            ))
        })
}
fn uint(n: &Node, k: &str) -> Result<u64, Error> {
    n.params
        .get(k)
        .and_then(Value::as_u64)
        .filter(|v| *v <= MAX_JS_SAFE_SEED)
        .ok_or_else(|| invalid(format!("node {} requires integer parameter {k}", n.id)))
}
fn text<'a>(n: &'a Node, k: &str) -> Result<&'a str, Error> {
    n.params
        .get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("node {} requires text parameter {k}", n.id)))
}
fn vec3(n: &Node, k: &str) -> Result<[f64; 3], Error> {
    let a = n
        .params
        .get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("node {} requires {k} array", n.id)))?;
    if a.len() != 3 {
        return Err(invalid(format!("node {} {k} must have three values", n.id)));
    };
    let mut v = [0.; 3];
    for i in 0..3 {
        v[i] = a[i]
            .as_f64()
            .filter(|x| x.is_finite())
            .ok_or_else(|| invalid(format!("node {} has invalid {k}", n.id)))?;
    }
    Ok(v)
}
fn triplets(n: &Node, k: &str) -> Result<[[f64; 3]; 3], Error> {
    let a = n
        .params
        .get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("node {} requires {k}", n.id)))?;
    if a.len() != 3 {
        return Err(invalid(format!(
            "node {} {k} must contain three colors",
            n.id
        )));
    };
    let mut out = [[0.; 3]; 3];
    for i in 0..3 {
        let row = a[i]
            .as_array()
            .filter(|x| x.len() == 3)
            .ok_or_else(|| invalid(format!("node {} {k} must be RGB triplets", n.id)))?;
        for c in 0..3 {
            out[i][c] = row[c]
                .as_f64()
                .filter(|x| (0.0..=1.0).contains(x))
                .ok_or_else(|| invalid(format!("node {} has invalid palette color", n.id)))?;
        }
    }
    Ok(out)
}
fn validate_params(n: &Node) -> Result<Type, Error> {
    let allowed: &[&str] = match n.kind.as_str() {
        "constant" => &["value", "unit"],
        "noise" => &[
            "unit",
            "wavelength_m",
            "amplitude",
            "offset",
            "octaves",
            "persistence",
            "seed_namespace",
            "seed",
        ],
        "remap" => &["in_min", "in_max", "out_min", "out_max"],
        "add" | "blend" => &[],
        "crater_height" | "crater_support" => &["center_direction", "radius_m", "depth_m", "rim_m"],
        "material" => &[
            "material_names",
            "palette",
            "height_transition_m",
            "temperature_transition_k",
            "humidity_bias",
            "variation_strength",
            "seed",
        ],
        _ => return Err(invalid(format!("unsupported node kind {}", n.kind))),
    };
    if n.params.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid(format!("node {} has unsupported parameter", n.id)));
    }
    let empty = || n.params.is_empty();
    match n.kind.as_str() {
        "constant" => {
            let u = unit(text(n, "unit")?)?;
            let v = num(n, "value")?;
            if !(-1.0e9..=1.0e9).contains(&v) {
                return Err(invalid("constant is outside safe bounds"));
            };
            Ok(Type {
                unit: u,
                lo: v,
                hi: v,
            })
        }
        "noise" => {
            let u = unit(text(n, "unit")?)?;
            let amp = num(n, "amplitude")?;
            let off = num(n, "offset")?;
            let wave = num(n, "wavelength_m")?;
            let oct = uint(n, "octaves")?;
            let persistence = num(n, "persistence")?;
            let ns = text(n, "seed_namespace")?;
            let _ = uint(n, "seed")?;
            if !matches!(ns, "geometry" | "climate" | "material")
                || wave < 0.01
                || wave > 1.0e15
                || amp < 0.0
                || amp > 1.0e9
                || off.abs() + amp > 1.0e12
                || !(1..=8).contains(&oct)
                || !(0.0..=1.0).contains(&persistence)
            {
                return Err(invalid(format!(
                    "node {} noise controls exceed safe limits",
                    n.id
                )));
            };
            Ok(Type {
                unit: u,
                lo: off - amp,
                hi: off + amp,
            })
        }
        "remap" => {
            let a = num(n, "in_min")?;
            let b = num(n, "in_max")?;
            let c = num(n, "out_min")?;
            let d = num(n, "out_max")?;
            if a >= b || c.abs() > 1.0e12 || d.abs() > 1.0e12 {
                return Err(invalid("remap ranges must be ordered and bounded"));
            };
            Ok(Type {
                unit: Unit::Normalized,
                lo: c.min(d),
                hi: c.max(d),
            })
        }
        "blend" => {
            if !empty() {
                return Err(invalid(format!("node {} has unknown parameters", n.id)));
            };
            Ok(Type {
                unit: Unit::Meters,
                lo: -1.0e12,
                hi: 1.0e12,
            })
        }
        "add" => {
            if !empty() {
                return Err(invalid(format!("node {} has unknown parameters", n.id)));
            };
            Ok(Type {
                unit: Unit::Meters,
                lo: -1.0e12,
                hi: 1.0e12,
            })
        }
        "crater_height" | "crater_support" => {
            let center = vec3(n, "center_direction")?;
            let len =
                (center[0] * center[0] + center[1] * center[1] + center[2] * center[2]).sqrt();
            let radius = num(n, "radius_m")?;
            let depth = num(n, "depth_m")?;
            let rim = num(n, "rim_m")?;
            if !len.is_finite()
                || len <= 0.0
                || !(1.0..=1.0e9).contains(&radius)
                || depth < 0.0
                || depth > 1.0e6
                || rim < 0.0
                || rim > 1.0e6
            {
                return Err(invalid(
                    "crater center/radius/depth/rim outside safe bounds",
                ));
            };
            if n.params.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "center_direction" | "radius_m" | "depth_m" | "rim_m"
                )
            }) {
                return Err(invalid("unknown crater parameter"));
            };
            Ok(if n.kind == "crater_height" {
                Type {
                    unit: Unit::Meters,
                    lo: -(depth + rim),
                    hi: rim,
                }
            } else {
                Type {
                    unit: Unit::Normalized,
                    lo: 0.0,
                    hi: 1.0,
                }
            })
        }
        "material" => {
            let names = n
                .params
                .get("material_names")
                .and_then(Value::as_array)
                .filter(|a| a.len() == 3)
                .ok_or_else(|| invalid("material_names must have three names"))?;
            let mut ns = BTreeSet::new();
            for v in names {
                let s = v
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 64)
                    .ok_or_else(|| invalid("material names must be short text"))?;
                if !ns.insert(s) {
                    return Err(invalid("material names must be unique"));
                }
            }
            let _ = triplets(n, "palette")?;
            let ht = num(n, "height_transition_m")?;
            let tt = num(n, "temperature_transition_k")?;
            let bias = vec3(n, "humidity_bias")?;
            let v = num(n, "variation_strength")?;
            if ht <= 0.0
                || ht > 1.0e9
                || tt <= 0.0
                || tt > 1.0e6
                || v < 0.0
                || v > 2.0
                || bias.iter().any(|b| !(-8.0..=8.0).contains(b))
            {
                return Err(invalid("material response parameters outside safe range"));
            };
            Ok(Type {
                unit: Unit::Material,
                lo: 0.,
                hi: 1.,
            })
        }
        _ => Err(invalid(format!("unsupported node kind {}", n.kind))),
    }
}
fn unit(s: &str) -> Result<Unit, Error> {
    match s {
        "m" => Ok(Unit::Meters),
        "1" => Ok(Unit::Normalized),
        "K" => Ok(Unit::Kelvin),
        _ => Err(invalid("unit must be m, 1, or K")),
    }
}
fn expected_ports(n: &Node) -> &'static [(&'static str, Unit)] {
    match n.kind.as_str() {
        "constant" | "noise" | "crater_height" | "crater_support" => &[],
        "remap" => &[("input", Unit::Meters)],
        "blend" => &[
            ("a", Unit::Meters),
            ("b", Unit::Meters),
            ("mask", Unit::Normalized),
        ],
        "add" => &[("a", Unit::Meters), ("b", Unit::Meters)],
        "material" => &[
            ("height", Unit::Meters),
            ("humidity", Unit::Normalized),
            ("temperature", Unit::Kelvin),
        ],
        _ => &[],
    }
}
fn input_slot(kind: &str, port: &str) -> Option<usize> {
    match (kind, port) {
        ("remap", "input") => Some(0),
        ("add", "a") | ("blend", "a") => Some(0),
        ("add", "b") | ("blend", "b") => Some(1),
        ("blend", "mask") => Some(2),
        ("material", "height") => Some(0),
        ("material", "humidity") => Some(1),
        ("material", "temperature") => Some(2),
        _ => None,
    }
}
impl CompiledGraph {
    pub fn compile(graph: Graph) -> Result<Self, Error> {
        validate_serialized_size(&graph)?;
        if graph.schema_version != GRAPH_SCHEMA_VERSION {
            return Err(invalid("unsupported graph schema_version"));
        };
        if graph.recipe_id.is_empty()
            || graph.recipe_id.len() > 128
            || !graph.reference_radius_m.is_finite()
            || !(1.0..=1.0e12).contains(&graph.reference_radius_m)
        {
            return Err(invalid("recipe identity or reference radius invalid"));
        };
        if [
            graph.seeds.geometry,
            graph.seeds.climate,
            graph.seeds.material,
        ]
        .iter()
        .any(|s| *s > MAX_JS_SAFE_SEED)
        {
            return Err(invalid(
                "schema 2 seeds must be JavaScript-safe nonnegative integers",
            ));
        }
        if graph.nodes.is_empty() || graph.nodes.len() > MAX_GRAPH_NODES {
            return Err(invalid(format!(
                "graph must contain 1..={MAX_GRAPH_NODES} nodes"
            )));
        };
        let mut raw = HashMap::new();
        for n in &graph.nodes {
            if n.id.is_empty() || n.id.len() > 64 || raw.insert(n.id.clone(), n).is_some() {
                return Err(invalid(
                    "node ids must be unique, nonempty, and at most 64 bytes",
                ));
            };
            if n.label.len() > 128 {
                return Err(invalid("node label too long"));
            };
            let _ = validate_params(n)?;
        }
        fn visit<'a>(
            id: &'a str,
            raw: &HashMap<String, &'a Node>,
            marks: &mut HashMap<&'a str, u8>,
            order: &mut Vec<&'a Node>,
        ) -> Result<(), Error> {
            match marks.get(id).copied() {
                Some(1) => return Err(invalid(format!("cycle detected at node {id}"))),
                Some(2) => return Ok(()),
                _ => (),
            };
            let n = *raw
                .get(id)
                .ok_or_else(|| invalid(format!("missing node reference {id}")))?;
            marks.insert(n.id.as_str(), 1);
            for source in n.inputs.values() {
                visit(source, raw, marks, order)?
            }
            marks.insert(n.id.as_str(), 2);
            order.push(n);
            Ok(())
        }
        let mut marks = HashMap::new();
        let mut order = Vec::new();
        for n in &graph.nodes {
            visit(&n.id, &raw, &mut marks, &mut order)?
        }
        let mut types: HashMap<String, Type> = HashMap::new();
        let mut compiled = Vec::new();
        for n in order {
            let mut ty = validate_params(n)?;
            let ports = expected_ports(n);
            for (key, _) in ports {
                if !n.inputs.contains_key(*key) {
                    return Err(invalid(format!("node {} missing input {key}", n.id)));
                }
            }
            if n.inputs.keys().any(|k| !ports.iter().any(|(p, _)| p == k)) {
                return Err(invalid(format!("node {} has an unsupported input", n.id)));
            };
            let mut source_types = HashMap::new();
            for (port, unit) in ports {
                let id = n
                    .inputs
                    .get(*port)
                    .ok_or_else(|| invalid("missing input"))?;
                let st = *types.get(id).ok_or_else(|| {
                    invalid(format!("node {} input {port} is not compiled", n.id))
                })?;
                let polymorphic = n.kind == "remap"
                    || ((n.kind == "add" || n.kind == "blend") && (*port == "a" || *port == "b"));
                if !polymorphic && st.unit != *unit {
                    return Err(invalid(format!(
                        "node {} input {port} expects unit {}, received {}",
                        n.id,
                        unit.code(),
                        st.unit.code()
                    )));
                };
                source_types.insert(*port, st);
            }
            match n.kind.as_str() {
                "remap" => {
                    let i = num(n, "in_min")?;
                    let j = num(n, "in_max")?;
                    let s = source_types["input"];
                    if i < s.lo || j > s.hi {
                        return Err(invalid(format!(
                            "node {} remap input range exceeds its upstream envelope",
                            n.id
                        )));
                    };
                    ty.unit = s.unit;
                    ty.lo = num(n, "out_min")?.min(num(n, "out_max")?);
                    ty.hi = num(n, "out_min")?.max(num(n, "out_max")?);
                }
                "blend" => {
                    let a = source_types["a"];
                    let b = source_types["b"];
                    let m = source_types["mask"];
                    if a.unit != b.unit || m.unit != Unit::Normalized || m.lo < 0.0 || m.hi > 1.0 {
                        return Err(invalid(format!(
                            "node {} blend units or mask envelope invalid",
                            n.id
                        )));
                    };
                    ty = Type {
                        unit: a.unit,
                        lo: a.lo.min(b.lo),
                        hi: a.hi.max(b.hi),
                    }
                }
                "add" => {
                    let a = source_types["a"];
                    let b = source_types["b"];
                    if a.unit != b.unit {
                        return Err(invalid(format!(
                            "node {} add inputs must have matching units",
                            n.id
                        )));
                    }
                    ty = Type {
                        unit: a.unit,
                        lo: a.lo + b.lo,
                        hi: a.hi + b.hi,
                    };
                    if !ty.lo.is_finite()
                        || !ty.hi.is_finite()
                        || ty.lo.abs().max(ty.hi.abs()) > 1.0e12
                    {
                        return Err(invalid("add output envelope exceeds safe bounds"));
                    }
                }
                "material" => {
                    let humidity = source_types["humidity"];
                    let temperature = source_types["temperature"];
                    if humidity.lo < 0.0
                        || humidity.hi > 1.0
                        || temperature.lo < 0.0
                        || temperature.hi > 5000.0
                    {
                        return Err(invalid(format!(
                            "node {} material climate input envelope is outside physical bounds",
                            n.id
                        )));
                    }
                }
                _ => (),
            };
            types.insert(n.id.clone(), ty);
            compiled.push(CompiledNode {
                node: n.clone(),
                output: ty,
                input_indexes: [None; 3],
            });
        }
        let required = [
            (&graph.outputs.height, Unit::Meters, "height"),
            (&graph.outputs.humidity, Unit::Normalized, "humidity"),
            (&graph.outputs.temperature, Unit::Kelvin, "temperature"),
            (&graph.outputs.material, Unit::Material, "material"),
        ];
        for (id, u, name) in required {
            let t = types
                .get(id)
                .ok_or_else(|| invalid(format!("output {name} references missing node {id}")))?;
            if t.unit != u {
                return Err(invalid(format!(
                    "output {name} requires unit {}, received {}",
                    u.code(),
                    t.unit.code()
                )));
            }
        }
        let height_type = types[&graph.outputs.height];
        let humidity_type = types[&graph.outputs.humidity];
        let temperature_type = types[&graph.outputs.temperature];
        if height_type.lo.abs().max(height_type.hi.abs()) > graph.reference_radius_m * 0.1
            || humidity_type.lo < 0.0
            || humidity_type.hi > 1.0
            || temperature_type.lo < 0.0
            || temperature_type.hi > 5000.0
        {
            return Err(invalid(
                "declared output envelope violates height, humidity, or temperature bounds",
            ));
        }
        if let Some(id) = &graph.outputs.support {
            let t = types
                .get(id)
                .ok_or_else(|| invalid("support output references missing node"))?;
            if t.unit != Unit::Normalized || t.lo < 0. || t.hi > 1. {
                return Err(invalid("support output must be normalized to [0,1]"));
            }
            let nodes_by_id: HashMap<&str, &Node> =
                graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            let mut height_deps = BTreeSet::new();
            fn dependencies(id: &str, nodes: &HashMap<&str, &Node>, out: &mut BTreeSet<String>) {
                if !out.insert(id.to_owned()) {
                    return;
                };
                if let Some(n) = nodes.get(id) {
                    for source in n.inputs.values() {
                        dependencies(source, nodes, out)
                    }
                }
            }
            dependencies(&graph.outputs.height, &nodes_by_id, &mut height_deps);
            if let (Some(support), Some(feature)) = (
                nodes_by_id.get(id.as_str()),
                height_deps
                    .iter()
                    .filter_map(|dep| nodes_by_id.get(dep.as_str()))
                    .find(|n| n.kind == "crater_height"),
            ) && support.kind == "crater_support"
                && support.params != feature.params
            {
                return Err(invalid(
                    "crater support and height nodes must share identical feature parameters",
                ));
            }
        }
        let indexes: HashMap<String, usize> = compiled
            .iter()
            .enumerate()
            .map(|(i, n)| (n.node.id.clone(), i))
            .collect();
        for item in &mut compiled {
            for (port, source) in &item.node.inputs {
                let slot = input_slot(&item.node.kind, port)
                    .ok_or_else(|| invalid("compiled graph contains an unknown input port"))?;
                item.input_indexes[slot] = Some(
                    *indexes
                        .get(source)
                        .ok_or_else(|| invalid("compiled graph input is missing"))?,
                );
            }
        }
        let output_indexes = [
            *indexes
                .get(&graph.outputs.height)
                .ok_or_else(|| invalid("missing height output"))?,
            *indexes
                .get(&graph.outputs.humidity)
                .ok_or_else(|| invalid("missing humidity output"))?,
            *indexes
                .get(&graph.outputs.temperature)
                .ok_or_else(|| invalid("missing temperature output"))?,
            *indexes
                .get(&graph.outputs.material)
                .ok_or_else(|| invalid("missing material output"))?,
        ];
        let support_index = graph
            .outputs
            .support
            .as_ref()
            .map(|id| {
                indexes
                    .get(id)
                    .copied()
                    .ok_or_else(|| invalid("missing support output"))
            })
            .transpose()?;
        let palette = triplets(&compiled[output_indexes[3]].node, "palette")?;
        let height_type = types[&graph.outputs.height];
        let height_bounds_m = [height_type.lo, height_type.hi];
        let (differential_step_rad, minimum_feature_angular_ratio) =
            differential_plan(&graph, &compiled)?;
        let identities = identities(
            &graph,
            &compiled,
            differential_step_rad,
            minimum_feature_angular_ratio,
        )?;
        let mut work_units_per_query = 0u64;
        for node in &compiled {
            work_units_per_query =
                work_units_per_query.saturating_add(if node.node.kind == "noise" {
                    uint(&node.node, "octaves")?.saturating_mul(8)
                } else {
                    1
                });
        }
        Ok(Self {
            graph,
            order: compiled,
            indexes,
            identities,
            work_units_per_query,
            output_indexes,
            support_index,
            palette,
            height_bounds_m,
            differential_step_rad,
            minimum_feature_angular_ratio,
        })
    }
    pub fn graph(&self) -> &Graph {
        &self.graph
    }
    pub fn identities(&self) -> &Identities {
        &self.identities
    }
    pub fn palette(&self) -> [[f64; 3]; 3] {
        self.palette
    }
    pub fn height_bounds_m(&self) -> [f64; 2] {
        self.height_bounds_m
    }
    pub fn differential_step_rad(&self) -> f64 {
        self.differential_step_rad
    }
    pub fn minimum_feature_angular_ratio(&self) -> Option<f64> {
        self.minimum_feature_angular_ratio
    }
    pub fn evaluator_version(&self) -> &'static str {
        GRAPH_EVALUATOR_VERSION
    }
    pub fn differential_method_version(&self) -> &'static str {
        DIFFERENTIAL_METHOD_VERSION
    }
    /// Canonical semantic graph bytes, generated only after the input has passed bounded validation.
    pub fn semantic_definition_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut graph = self.graph.clone();
        graph.layout.clear();
        graph.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        for node in &mut graph.nodes {
            node.label.clear();
        }
        serde_json::to_vec(&graph).map_err(Error::from)
    }
    /// Fixed stack scratch used by the allocation-free native scalar evaluator.
    pub fn query_stack_bytes(&self) -> usize {
        MAX_GRAPH_NODES * std::mem::size_of::<ValueOut>()
    }
    pub fn export_work(&self, resolution: u32) -> u64 {
        (resolution as u64)
            .saturating_mul(resolution as u64)
            .saturating_mul(6)
            .saturating_mul(self.work_units_per_query)
            .saturating_mul(5)
            .saturating_mul(2)
    }
    pub fn normal_rgb(&self, direction: [f64; 3]) -> Result<[f64; 3], Error> {
        let n = self.surface_differential(direction)?.outward_normal;
        Ok([0.5 + 0.5 * n[0], 0.5 + 0.5 * n[1], 0.5 + 0.5 * n[2]])
    }
    /// Derive body-space slope and normal from the same compiled height authority.
    pub fn surface_differential(&self, direction: [f64; 3]) -> Result<SurfaceDifferential, Error> {
        self.evaluate_with_differential(direction).map(|(_, d)| d)
    }
    pub fn evaluate_surface_with_differential(
        &self,
        direction: [f64; 3],
    ) -> Result<(SurfaceEvaluation, SurfaceDifferential), Error> {
        let n = normalize_direction(direction)?;
        let sample = self.evaluate_surface(n)?;
        let differential = differential_at_height(self, n, sample.height_m)?;
        Ok((sample, differential))
    }
    /// Evaluate all output fields and their body-space height differential in one center query.
    pub fn evaluate_with_differential(
        &self,
        direction: [f64; 3],
    ) -> Result<(Evaluated, SurfaceDifferential), Error> {
        let n = normalize_direction(direction)?;
        let sample = self.evaluate(n)?;
        let differential = differential_at_height(self, n, sample.height_m)?;
        let color = std::array::from_fn(|channel| {
            (0..3)
                .map(|i| sample.weights[i] * self.palette[i][channel])
                .sum()
        });
        Ok((Evaluated { color, ..sample }, differential))
    }
    pub fn inspect_descriptor(&self, id: &str) -> Result<Value, Error> {
        let i = *self
            .indexes
            .get(id)
            .ok_or_else(|| invalid(format!("unknown inspect_node {id}")))?;
        let n = &self.order[i];
        if n.output.unit == Unit::Material {
            return Err(invalid("material node scalar inspection is unsupported"));
        };
        Ok(
            json!({"id":id,"kind":n.node.kind,"unit":n.output.unit.code(),"range":[n.output.lo,n.output.hi]}),
        )
    }
    pub fn ranges(&self) -> Value {
        let node = |id: &str| self.order[*self.indexes.get(id).unwrap()].output;
        let h = node(&self.graph.outputs.height);
        let q = node(&self.graph.outputs.humidity);
        let t = node(&self.graph.outputs.temperature);
        json!({"height":[h.lo,h.hi],"humidity":[q.lo,q.hi],"temperature":[t.lo,t.hi],"support":[0.0,1.0]})
    }
    pub fn evaluate(&self, direction: [f64; 3]) -> Result<Evaluated, Error> {
        let (sample, node_values) = self.evaluate_inner(direction, true)?;
        let color = std::array::from_fn(|channel| {
            (0..3)
                .map(|i| sample.weights[i] * self.palette[i][channel])
                .sum()
        });
        Ok(Evaluated {
            height_m: sample.height_m,
            humidity: sample.humidity,
            temperature_k: sample.temperature_k,
            material: sample.weights,
            weights: sample.weights,
            color,
            support: sample.support,
            node_values,
        })
    }

    /// Allocation-free scalar output path for native world queries.
    pub fn evaluate_surface(&self, direction: [f64; 3]) -> Result<SurfaceEvaluation, Error> {
        self.evaluate_inner(direction, false)
            .map(|(sample, _)| sample)
    }

    fn evaluate_inner(
        &self,
        direction: [f64; 3],
        inspect_nodes: bool,
    ) -> Result<(SurfaceEvaluation, BTreeMap<String, f64>), Error> {
        let p = normalize_direction(direction)?;
        let mut cache = [ValueOut::Scalar(0.0); MAX_GRAPH_NODES];
        let mut node_values = BTreeMap::new();
        for (index, c) in self.order.iter().enumerate() {
            let n = &c.node;
            let input = |slot: usize| -> Result<ValueOut, Error> {
                let source = c.input_indexes[slot]
                    .ok_or_else(|| invalid("compiled graph input was not resolved"))?;
                if source >= index {
                    return Err(invalid("compiled graph dependency order is invalid"));
                }
                Ok(cache[source])
            };
            let out = match n.kind.as_str() {
                "constant" => ValueOut::Scalar(num(n, "value")?),
                "noise" => {
                    let ns = text(n, "seed_namespace")?;
                    let seed = match ns {
                        "geometry" => self.graph.seeds.geometry,
                        "climate" => self.graph.seeds.climate,
                        "material" => self.graph.seeds.material,
                        _ => 0,
                    };
                    let seed = seed.wrapping_add(uint(n, "seed")?);
                    ValueOut::Scalar(
                        num(n, "offset")?
                            + num(n, "amplitude")?
                                * fbm(
                                    p,
                                    self.graph.reference_radius_m / num(n, "wavelength_m")?,
                                    uint(n, "octaves")? as u8,
                                    num(n, "persistence")?,
                                    seed,
                                ),
                    )
                }
                "remap" => {
                    let ValueOut::Scalar(v) = input(0)? else {
                        return Err(invalid("remap input is not scalar"));
                    };
                    let x = ((v - num(n, "in_min")?) / (num(n, "in_max")? - num(n, "in_min")?))
                        .clamp(0., 1.);
                    ValueOut::Scalar(
                        num(n, "out_min")? + x * (num(n, "out_max")? - num(n, "out_min")?),
                    )
                }
                "add" => {
                    let (ValueOut::Scalar(a), ValueOut::Scalar(b)) = (input(0)?, input(1)?) else {
                        return Err(invalid("add inputs are not scalar"));
                    };
                    ValueOut::Scalar(a + b)
                }
                "blend" => {
                    let (ValueOut::Scalar(a), ValueOut::Scalar(b), ValueOut::Scalar(m)) =
                        (input(0)?, input(1)?, input(2)?)
                    else {
                        return Err(invalid("blend inputs are not scalar"));
                    };
                    ValueOut::Scalar(a * (1. - m.clamp(0., 1.)) + b * m.clamp(0., 1.))
                }
                "crater_support" | "crater_height" => {
                    let center = normalize_direction(vec3(n, "center_direction")?)?;
                    let dot =
                        (center[0] * p[0] + center[1] * p[1] + center[2] * p[2]).clamp(-1., 1.);
                    let cross = [
                        center[1] * p[2] - center[2] * p[1],
                        center[2] * p[0] - center[0] * p[2],
                        center[0] * p[1] - center[1] * p[0],
                    ];
                    let sine = cross[0].hypot(cross[1]).hypot(cross[2]);
                    let distance = self.graph.reference_radius_m * sine.atan2(dot);
                    let radius = num(n, "radius_m")?;
                    let x = distance / radius;
                    let support = 1.0 - smoothstep(0.82, 1.0, x);
                    if n.kind == "crater_support" {
                        ValueOut::Scalar(support)
                    } else {
                        let depth = num(n, "depth_m")?;
                        let rim = num(n, "rim_m")?;
                        let basin = -depth * (1. - x.min(1.).powi(2));
                        let rim_profile = rim * (-((x - 0.82) / 0.09).powi(2)).exp();
                        ValueOut::Scalar((basin + rim_profile) * support)
                    }
                }
                "material" => {
                    let (ValueOut::Scalar(h), ValueOut::Scalar(q), ValueOut::Scalar(t)) =
                        (input(0)?, input(1)?, input(2)?)
                    else {
                        return Err(invalid("material inputs are not scalar"));
                    };
                    let names = n.params["material_names"]
                        .as_array()
                        .ok_or_else(|| invalid("invalid material names"))?;
                    let _ = names;
                    let bias = vec3(n, "humidity_bias")?;
                    let ht = num(n, "height_transition_m")?;
                    let tt = num(n, "temperature_transition_k")?;
                    let altitude = (h / ht).clamp(-1., 1.);
                    let thermal = ((t - 273.15) / tt).clamp(-1., 1.);
                    let variation = num(n, "variation_strength")?;
                    let seed = self.graph.seeds.material.wrapping_add(uint(n, "seed")?);
                    let noise =
                        fbm(p, self.graph.reference_radius_m / 5000.0, 2, 0.5, seed) * variation;
                    let logits = [
                        bias[0] + q * 2.0 - altitude * 0.4 + noise,
                        bias[1] + altitude * 0.8 - thermal * 0.2 - noise * 0.5,
                        bias[2] - q + thermal * 0.6 + altitude * 0.2 - noise * 0.5,
                    ];
                    let mx = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let mut w = logits.map(|v| (v - mx).exp());
                    let sum = w.iter().sum::<f64>();
                    for x in &mut w {
                        *x /= sum
                    }
                    ValueOut::Material(w)
                }
                _ => return Err(invalid("unknown compiled node")),
            };
            if let ValueOut::Scalar(v) = out
                && (!v.is_finite() || v < c.output.lo - 1e-8 || v > c.output.hi + 1e-8)
            {
                return Err(invalid(format!(
                    "node {} escaped declared output envelope",
                    n.id
                )));
            };
            if inspect_nodes && let ValueOut::Scalar(v) = out {
                node_values.insert(n.id.clone(), v);
            }
            cache[index] = out;
        }
        let scalar = |index: usize| -> Result<f64, Error> {
            match cache[index] {
                ValueOut::Scalar(v) => Ok(v),
                ValueOut::Material(_) => Err(invalid("expected scalar output")),
            }
        };
        let m = match cache[self.output_indexes[3]] {
            ValueOut::Material(v) => v,
            _ => return Err(invalid("expected material output")),
        };
        let support = self.support_index.map(scalar).transpose()?.unwrap_or(0.0);
        let result = SurfaceEvaluation {
            height_m: scalar(self.output_indexes[0])?,
            humidity: scalar(self.output_indexes[1])?,
            temperature_k: scalar(self.output_indexes[2])?,
            weights: m,
            support,
        };
        if !result.humidity.is_finite()
            || !(0.0..=1.0).contains(&result.humidity)
            || result.temperature_k < 0.0
            || result
                .weights
                .iter()
                .any(|v| !v.is_finite() || *v < 0. || *v > 1.)
            || (result.weights.iter().sum::<f64>() - 1.).abs() > 1e-10
        {
            return Err(invalid("evaluated outputs violate physical ranges"));
        };
        Ok((result, node_values))
    }
    pub fn sample_map(
        &self,
        resolution: u32,
        face: usize,
        inspect_node: Option<&str>,
    ) -> Result<SampleMap, Error> {
        if !(1..=256).contains(&resolution) || face >= 6 || resolution > 64 {
            return Err(invalid(
                "preview resolution must be 1..=64 and face must be 0..6",
            ));
        };
        let work = (resolution as u64)
            .saturating_mul(resolution as u64)
            .saturating_mul(self.work_units_per_query)
            .saturating_mul(5);
        if work > MAX_PREVIEW_WORK {
            return Err(invalid("preview graph exceeds the bounded work budget"));
        }
        if let Some(id) = inspect_node {
            let _ = self.inspect_descriptor(id)?;
        }
        let count = (resolution * resolution) as usize;
        let mut map = SampleMap {
            width: resolution,
            height: resolution,
            height_m: Vec::with_capacity(count),
            humidity: Vec::with_capacity(count),
            temperature_k: Vec::with_capacity(count),
            material: Vec::with_capacity(count * 3),
            weight_0: Vec::with_capacity(count),
            weight_1: Vec::with_capacity(count),
            weight_2: Vec::with_capacity(count),
            support: Vec::with_capacity(count),
            normal: Vec::with_capacity(count * 3),
            node_values: inspect_node.map(|_| Vec::with_capacity(count)),
            stats: Stats {
                height_min_m: f64::INFINITY,
                height_max_m: f64::NEG_INFINITY,
                humidity_min: f64::INFINITY,
                humidity_max: f64::NEG_INFINITY,
                temperature_min_k: f64::INFINITY,
                temperature_max_k: f64::NEG_INFINITY,
            },
        };
        for y in 0..resolution {
            for x in 0..resolution {
                let u = 2.0 * (x as f64 + 0.5) / resolution as f64 - 1.;
                let v = 2.0 * (y as f64 + 0.5) / resolution as f64 - 1.;
                let d = cube_direction(face, u, v)?;
                let (s, differential) = self.evaluate_with_differential(d)?;
                map.stats.height_min_m = map.stats.height_min_m.min(s.height_m);
                map.stats.height_max_m = map.stats.height_max_m.max(s.height_m);
                map.stats.humidity_min = map.stats.humidity_min.min(s.humidity);
                map.stats.humidity_max = map.stats.humidity_max.max(s.humidity);
                map.stats.temperature_min_k = map.stats.temperature_min_k.min(s.temperature_k);
                map.stats.temperature_max_k = map.stats.temperature_max_k.max(s.temperature_k);
                map.height_m.push(s.height_m);
                map.humidity.push(s.humidity);
                map.temperature_k.push(s.temperature_k);
                map.material.extend(s.color);
                map.weight_0.push(s.weights[0]);
                map.weight_1.push(s.weights[1]);
                map.weight_2.push(s.weights[2]);
                map.support.push(s.support);
                if let (Some(id), Some(values)) = (inspect_node, map.node_values.as_mut()) {
                    values.push(*s.node_values.get(id).ok_or_else(|| {
                        invalid(format!("node {id} has no scalar inspection output"))
                    })?);
                }
                map.normal
                    .extend(differential.outward_normal.map(|v| 0.5 + 0.5 * v));
            }
        }
        Ok(map)
    }
}

pub fn vary(graph: &Graph, seed: u64, locks: &VariationLocks) -> Result<Graph, Error> {
    let mut out = graph.clone();
    if !locks.geometry {
        out.seeds.geometry = mix(seed ^ 0x47454f4d45545259) % MAX_JS_SAFE_SEED;
    }
    if !locks.climate {
        out.seeds.climate = mix(seed ^ 0x434c494d41544531) % MAX_JS_SAFE_SEED;
    }
    if !locks.palette {
        let palette = out
            .nodes
            .iter_mut()
            .find(|n| n.kind == "material")
            .ok_or_else(|| invalid("graph has no material node"))?;
        let mut colors = triplets(palette, "palette")?;
        for (i, row) in colors.iter_mut().enumerate() {
            for (c, value) in row.iter_mut().enumerate() {
                let bit = mix(seed.wrapping_add((i * 3 + c) as u64)) as f64 / u64::MAX as f64;
                *value = (*value + (bit - 0.5) * 0.16).clamp(0.0, 1.0)
            }
        }
        palette
            .params
            .insert("palette".into(), serde_json::to_value(colors)?);
    }
    let _ = CompiledGraph::compile(out.clone())?;
    Ok(out)
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VariationLocks {
    pub geometry: bool,
    pub climate: bool,
    pub palette: bool,
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn differential_at_height(
    g: &CompiledGraph,
    d: [f64; 3],
    height_m: f64,
) -> Result<SurfaceDifferential, Error> {
    let eps = g.differential_step_rad;
    let axis = if d[2].abs() < 0.8 {
        [0., 0., 1.]
    } else {
        [0., 1., 0.]
    };
    let tangent = [
        d[1] * axis[2] - d[2] * axis[1],
        d[2] * axis[0] - d[0] * axis[2],
        d[0] * axis[1] - d[1] * axis[0],
    ];
    let t = normalize_direction(tangent)?;
    let u = [
        d[1] * t[2] - d[2] * t[1],
        d[2] * t[0] - d[0] * t[2],
        d[0] * t[1] - d[1] * t[0],
    ];
    let sample_height =
        |v: [f64; 3]| -> Result<f64, Error> { g.evaluate_surface(v).map(|s| s.height_m) };
    let plus_t = sample_height([d[0] + eps * t[0], d[1] + eps * t[1], d[2] + eps * t[2]])?;
    let minus_t = sample_height([d[0] - eps * t[0], d[1] - eps * t[1], d[2] - eps * t[2]])?;
    let plus_u = sample_height([d[0] + eps * u[0], d[1] + eps * u[1], d[2] + eps * u[2]])?;
    let minus_u = sample_height([d[0] - eps * u[0], d[1] - eps * u[1], d[2] - eps * u[2]])?;
    let slope_t = (plus_t - minus_t) / (2.0 * eps);
    let slope_u = (plus_u - minus_u) / (2.0 * eps);
    let gradient = std::array::from_fn(|i| slope_t * t[i] + slope_u * u[i]);
    let radial = g.graph.reference_radius_m + height_m;
    if !radial.is_finite() || radial <= 0.0 || gradient.iter().any(|v| !v.is_finite()) {
        return Err(invalid(
            "surface differential is outside finite radial bounds",
        ));
    }
    let mut normal = std::array::from_fn(|i| d[i] - gradient[i] / radial);
    normal = normalize_direction(normal)?;
    Ok(SurfaceDifferential {
        height_gradient_tangent_m_per_radian: gradient,
        outward_normal: normal,
    })
}

fn differential_plan(graph: &Graph, nodes: &[CompiledNode]) -> Result<(f64, Option<f64>), Error> {
    let by_id: HashMap<&str, &Node> = nodes
        .iter()
        .map(|node| (node.node.id.as_str(), &node.node))
        .collect();
    let compiled_by_id: HashMap<&str, &CompiledNode> = nodes
        .iter()
        .map(|node| (node.node.id.as_str(), node))
        .collect();
    let mut pending = vec![graph.outputs.height.as_str()];
    let mut closure = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !closure.insert(id.to_owned()) {
            continue;
        }
        if let Some(node) = by_id.get(id) {
            pending.extend(node.inputs.values().map(String::as_str));
        }
    }

    // Propagate the finest physical feature scale through the height DAG. A
    // remap's clamped input window can sharpen its source transition, so shrink
    // the inherited scale by the fraction of the upstream declared envelope
    // covered by that window.
    let mut scales_by_id: HashMap<&str, Option<f64>> = HashMap::new();
    for compiled in nodes {
        let node = &compiled.node;
        if !closure.contains(&node.id) {
            continue;
        }
        let inherited = node
            .inputs
            .values()
            .filter_map(|source| scales_by_id.get(source.as_str()).copied().flatten())
            .reduce(f64::min);
        let own = match node.kind.as_str() {
            "noise" if num(node, "amplitude")? > 0.0 => {
                let octave_count = uint(node, "octaves")?;
                let effective_octaves = if num(node, "persistence")? == 0.0 {
                    1
                } else {
                    octave_count
                };
                Some(num(node, "wavelength_m")? / 2.0_f64.powi((effective_octaves - 1) as i32))
            }
            "crater_height" => {
                let depth = num(node, "depth_m")?;
                let rim = num(node, "rim_m")?;
                if depth == 0.0 && rim == 0.0 {
                    None
                } else {
                    let radius = num(node, "radius_m")?;
                    Some(radius * if rim > 0.0 { 0.09 } else { 0.18 })
                }
            }
            "crater_support" => Some(num(node, "radius_m")? * 0.18),
            _ => None,
        };
        let propagated = if node.kind == "remap" {
            match inherited {
                Some(scale) if num(node, "out_min")? != num(node, "out_max")? => {
                    let source_id = node
                        .inputs
                        .get("input")
                        .ok_or_else(|| invalid(format!("node {} missing remap input", node.id)))?;
                    let source = compiled_by_id
                        .get(source_id.as_str())
                        .ok_or_else(|| invalid("remap source is missing from compiled graph"))?;
                    let source_span = source.output.hi - source.output.lo;
                    let window = num(node, "in_max")? - num(node, "in_min")?;
                    if source_span > 0.0 {
                        Some(scale * (window / source_span).min(1.0))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            match (own, inherited) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                (None, None) => None,
            }
        };
        scales_by_id.insert(node.id.as_str(), propagated);
    }

    let minimum_scale_m = scales_by_id
        .get(graph.outputs.height.as_str())
        .copied()
        .flatten();

    let Some(scale_m) = minimum_scale_m else {
        return Ok((MAX_DIFFERENTIAL_STEP_RAD, None));
    };
    let ratio = scale_m / graph.reference_radius_m;
    if !ratio.is_finite() || ratio < MIN_FEATURE_ANGULAR_RATIO {
        return Err(invalid(format!(
            "height feature angular scale {ratio:e} is below the supported minimum {MIN_FEATURE_ANGULAR_RATIO:e}"
        )));
    }
    let step = MAX_DIFFERENTIAL_STEP_RAD.min(ratio * 1.0e-3);
    if step < MIN_DIFFERENTIAL_STEP_RAD {
        return Err(invalid(format!(
            "height feature requires differential step {step:e} below supported minimum {MIN_DIFFERENTIAL_STEP_RAD:e}"
        )));
    }
    Ok((step, Some(ratio)))
}
#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub height_min_m: f64,
    pub height_max_m: f64,
    pub humidity_min: f64,
    pub humidity_max: f64,
    pub temperature_min_k: f64,
    pub temperature_max_k: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct SampleMap {
    pub width: u32,
    pub height: u32,
    pub height_m: Vec<f64>,
    pub humidity: Vec<f64>,
    pub temperature_k: Vec<f64>,
    pub material: Vec<f64>,
    pub weight_0: Vec<f64>,
    pub weight_1: Vec<f64>,
    pub weight_2: Vec<f64>,
    pub support: Vec<f64>,
    pub normal: Vec<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_values: Option<Vec<f64>>,
    pub stats: Stats,
}

fn hash<T: Serialize>(v: &T) -> Result<String, Error> {
    let bytes = serde_json::to_vec(v)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn identities(
    g: &Graph,
    nodes: &[CompiledNode],
    differential_step_rad: f64,
    minimum_feature_angular_ratio: Option<f64>,
) -> Result<Identities, Error> {
    let by_id: HashMap<&str, &Node> = nodes
        .iter()
        .map(|n| (n.node.id.as_str(), &n.node))
        .collect();
    fn closure(id: &str, by_id: &HashMap<&str, &Node>, found: &mut BTreeSet<String>) {
        if !found.insert(id.to_owned()) {
            return;
        }
        if let Some(node) = by_id.get(id) {
            for source in node.inputs.values() {
                closure(source, by_id, found);
            }
        }
    }
    fn records<'a>(
        roots: &[&str],
        by_id: &HashMap<&'a str, &'a Node>,
        strip_palette: bool,
    ) -> Vec<Value> {
        let mut found = BTreeSet::new();
        for root in roots {
            closure(root, by_id, &mut found);
        }
        found
            .into_iter()
            .filter_map(|id| {
                by_id.get(id.as_str()).map(|n| {
                    let mut params = n.params.clone();
                    if strip_palette {
                        params.remove("palette");
                    }
                    json!({"id":n.id,"kind":n.kind,"params":params,"inputs":n.inputs})
                })
            })
            .collect()
    }
    fn namespaces(roots: &[&str], by_id: &HashMap<&str, &Node>) -> BTreeSet<&'static str> {
        let mut found = BTreeSet::new();
        for root in roots {
            closure(root, by_id, &mut found)
        }
        found
            .into_iter()
            .filter_map(|id| {
                by_id.get(id.as_str()).and_then(|n| {
                    if n.kind == "material" {
                        Some("material")
                    } else {
                        n.params.get("seed_namespace").and_then(Value::as_str)
                    }
                })
            })
            .filter_map(|v| match v {
                "geometry" => Some("geometry"),
                "climate" => Some("climate"),
                "material" => Some("material"),
                _ => None,
            })
            .collect()
    }
    let mut geometry_roots = vec![g.outputs.height.as_str()];
    if let Some(support) = g.outputs.support.as_deref() {
        geometry_roots.push(support);
    }
    let geo_records = records(&geometry_roots, &by_id, false);
    let climate_records = records(
        &[&g.outputs.humidity, &g.outputs.temperature],
        &by_id,
        false,
    );
    let material_records = records(&[&g.outputs.material], &by_id, true);
    let geo_ns = namespaces(&geometry_roots, &by_id);
    let climate_ns = namespaces(&[&g.outputs.humidity, &g.outputs.temperature], &by_id);
    let mat_ns = namespaces(&[&g.outputs.material], &by_id);
    let uses_radius = |records: &[Value]| {
        records.iter().any(|n| {
            matches!(
                n["kind"].as_str(),
                Some("noise" | "crater_height" | "crater_support")
            )
        })
    };
    let seed_value = |ns: &str| match ns {
        "geometry" => g.seeds.geometry,
        "climate" => g.seeds.climate,
        _ => g.seeds.material,
    };
    let geometry = hash(&(
        GRAPH_EVALUATOR_VERSION,
        DIFFERENTIAL_METHOD_VERSION,
        differential_step_rad.to_bits(),
        minimum_feature_angular_ratio.map(f64::to_bits),
        uses_radius(&geo_records).then_some(g.reference_radius_m.to_bits()),
        geo_ns
            .iter()
            .map(|ns| (*ns, seed_value(ns)))
            .collect::<Vec<_>>(),
        geo_records,
    ))?;
    let climate = hash(&(
        GRAPH_EVALUATOR_VERSION,
        uses_radius(&climate_records).then_some(g.reference_radius_m.to_bits()),
        climate_ns
            .iter()
            .map(|ns| (*ns, seed_value(ns)))
            .collect::<Vec<_>>(),
        climate_records,
    ))?;
    let material = hash(&(
        GRAPH_EVALUATOR_VERSION,
        geometry.clone(),
        climate.clone(),
        mat_ns
            .iter()
            .map(|ns| (*ns, seed_value(ns)))
            .collect::<Vec<_>>(),
        material_records,
    ))?;
    let palette = hash(
        &records(&[&g.outputs.material], &by_id, false)
            .iter()
            .filter_map(|n| n["params"].get("palette").cloned())
            .collect::<Vec<_>>(),
    )?;
    let graph = hash(&(
        GRAPH_SCHEMA_VERSION,
        crate::ALGORITHM_VERSION,
        GRAPH_EVALUATOR_VERSION,
        DIFFERENTIAL_METHOD_VERSION,
        &g.recipe_id,
        &geometry,
        &climate,
        &material,
        &palette,
    ))?;
    Ok(Identities {
        geometry,
        climate,
        material,
        palette,
        graph,
    })
}

pub fn catalog() -> Value {
    json!({"templates":templates(),"node_kinds":[
    {"kind":"constant","label":"Constant","output_type":"scalar","inputs":[],"params":[{"name":"value","label":"Value","type":"number","default":0.0,"min":-1000000.0,"max":1000000.0,"step":1.0},{"name":"unit","label":"Unit","type":"text","default":"m","choices":["m","1","K"]}]},
    {"kind":"noise","label":"Coherent noise","output_type":"scalar","inputs":[],"params":[{"name":"unit","label":"Unit","type":"text","default":"m","choices":["m","1","K"]},{"name":"wavelength_m","label":"Wavelength (m)","type":"number","default":10000.0,"min":0.01,"max":1000000000.0,"step":10.0},{"name":"amplitude","label":"Amplitude","type":"number","default":100.0,"min":0.0,"max":1000000.0,"step":1.0},{"name":"offset","label":"Offset","type":"number","default":0.0,"min":-1000000.0,"max":1000000.0,"step":1.0},{"name":"octaves","label":"Octaves","type":"integer","default":4,"min":1,"max":8,"step":1},{"name":"persistence","label":"Persistence","type":"number","default":0.5,"min":0.0,"max":1.0,"step":0.05},{"name":"seed_namespace","label":"Seed namespace","type":"text","default":"geometry","choices":["geometry","climate","material"]},{"name":"seed","label":"Node seed","type":"integer","default":0,"min":0,"max":2147483647,"step":1}]},
    {"kind":"remap","label":"Remap","output_type":"scalar","inputs":[{"name":"input","type":"scalar","required":true}],"params":[{"name":"in_min","label":"Input min","type":"number","default":-1.0,"min":-1000000000.0,"max":1000000000.0,"step":1.0},{"name":"in_max","label":"Input max","type":"number","default":1.0,"min":-1000000000.0,"max":1000000000.0,"step":1.0},{"name":"out_min","label":"Output min","type":"number","default":0.0,"min":-1000000.0,"max":1000000.0,"step":1.0},{"name":"out_max","label":"Output max","type":"number","default":1.0,"min":-1000000.0,"max":1000000.0,"step":1.0}]},
    {"kind":"blend","label":"Blend","output_type":"scalar","inputs":[{"name":"a","type":"scalar","required":true},{"name":"b","type":"scalar","required":true},{"name":"mask","type":"scalar[1]","required":true}],"params":[]},{"kind":"add","label":"Add","output_type":"scalar","inputs":[{"name":"a","type":"scalar","required":true},{"name":"b","type":"scalar","required":true}],"params":[]},
    {"kind":"crater_height","label":"Crater relief","output_type":"scalar[m]","inputs":[],"params":[{"name":"center_direction","label":"Center direction","type":"text","default":[0.0,0.0,1.0]},{"name":"radius_m","label":"Radius (m)","type":"number","default":1000.0,"min":1.0,"max":100000000.0,"step":1.0},{"name":"depth_m","label":"Depth (m)","type":"number","default":120.0,"min":0.0,"max":1000000.0,"step":1.0},{"name":"rim_m","label":"Rim (m)","type":"number","default":30.0,"min":0.0,"max":1000000.0,"step":1.0}]},{"kind":"crater_support","label":"Crater support","output_type":"scalar[1]","inputs":[],"params":[{"name":"center_direction","label":"Center direction","type":"text","default":[0.0,0.0,1.0]},{"name":"radius_m","label":"Radius (m)","type":"number","default":1000.0,"min":1.0,"max":100000000.0,"step":1.0},{"name":"depth_m","label":"Depth (m)","type":"number","default":120.0,"min":0.0,"max":1000000.0,"step":1.0},{"name":"rim_m","label":"Rim (m)","type":"number","default":30.0,"min":0.0,"max":1000000.0,"step":1.0}]},
    {"kind":"material","label":"Material response","output_type":"material","inputs":[{"name":"height","type":"scalar[m]","required":true},{"name":"humidity","type":"scalar[1]","required":true},{"name":"temperature","type":"scalar[K]","required":true}],"params":[{"name":"material_names","label":"Material names","type":"text","default":["regolith","rock","basalt"]},{"name":"palette","label":"Palette","type":"color_palette","default":[[0.36,0.30,0.24],[0.50,0.46,0.40],[0.18,0.22,0.28]]},{"name":"height_transition_m","label":"Height response scale (m)","type":"number","default":500.0,"min":0.01,"max":1000000.0,"step":1.0},{"name":"temperature_transition_k","label":"Temperature response scale (K)","type":"number","default":100.0,"min":0.01,"max":3000.0,"step":1.0},{"name":"humidity_bias","label":"Humidity response","type":"text","default":[0.4,0.0,-0.2]},{"name":"variation_strength","label":"Response variation","type":"number","default":0.25,"min":0.0,"max":2.0,"step":0.01},{"name":"seed","label":"Node seed","type":"integer","default":0,"min":0,"max":2147483647,"step":1}]}
    ]})
}

pub fn templates() -> Value {
    json!([
    {"id":"rocky-feature","name":"Rocky impact feature","graph":{"schema_version":2,"recipe_id":"rocky-feature","reference_radius_m":1737400.0,"seeds":{"geometry":41,"climate":53,"material":67},"nodes":[
    {"id":"base","kind":"noise","label":"Regional relief","params":{"unit":"m","wavelength_m":18000.0,"amplitude":260.0,"offset":0.0,"octaves":5,"persistence":0.52,"seed_namespace":"geometry","seed":1},"inputs":{}},
    {"id":"crater","kind":"crater_height","label":"Impact bowl and rim","params":{"center_direction":[0.0,0.0,1.0],"radius_m":42000.0,"depth_m":360.0,"rim_m":95.0},"inputs":{}},
    {"id":"height","kind":"add","label":"Composed relief","params":{},"inputs":{"a":"base","b":"crater"}},
    {"id":"humidity","kind":"noise","label":"Moisture","params":{"unit":"1","wavelength_m":90000.0,"amplitude":0.32,"offset":0.5,"octaves":4,"persistence":0.55,"seed_namespace":"climate","seed":2},"inputs":{}},
    {"id":"temperature","kind":"noise","label":"Temperature","params":{"unit":"K","wavelength_m":130000.0,"amplitude":35.0,"offset":235.0,"octaves":3,"persistence":0.5,"seed_namespace":"climate","seed":3},"inputs":{}},
    {"id":"materials","kind":"material","label":"Rock and regolith","params":{"material_names":["regolith","rock","basalt"],"palette":[[0.36,0.30,0.24],[0.50,0.46,0.40],[0.18,0.22,0.28]],"height_transition_m":500.0,"temperature_transition_k":100.0,"humidity_bias":[0.4,0.0,-0.2],"variation_strength":0.25,"seed":4},"inputs":{"height":"height","humidity":"humidity","temperature":"temperature"}},
    {"id":"feature_mask","kind":"crater_support","label":"Feature support","params":{"center_direction":[0.0,0.0,1.0],"radius_m":42000.0,"depth_m":360.0,"rim_m":95.0},"inputs":{}}],"outputs":{"height":"height","humidity":"humidity","temperature":"temperature","material":"materials","support":"feature_mask"},"layout":{}}},
    {"id":"cold-highlands","name":"Cold fractured highlands","graph":{"schema_version":2,"recipe_id":"cold-highlands","reference_radius_m":1560000.0,"seeds":{"geometry":101,"climate":202,"material":303},"nodes":[
    {"id":"broad","kind":"noise","label":"Highland backbone","params":{"unit":"m","wavelength_m":85000.0,"amplitude":520.0,"offset":150.0,"octaves":4,"persistence":0.48,"seed_namespace":"geometry","seed":0},"inputs":{}},
    {"id":"ridge","kind":"noise","label":"Fracture relief","params":{"unit":"m","wavelength_m":12000.0,"amplitude":130.0,"offset":0.0,"octaves":5,"persistence":0.58,"seed_namespace":"geometry","seed":7},"inputs":{}},
    {"id":"height","kind":"add","label":"Layered highlands","params":{},"inputs":{"a":"broad","b":"ridge"}},
    {"id":"humidity","kind":"noise","label":"Moisture field","params":{"unit":"1","wavelength_m":115000.0,"amplitude":0.27,"offset":0.27,"octaves":4,"persistence":0.55,"seed_namespace":"climate","seed":2},"inputs":{}},
    {"id":"temperature","kind":"noise","label":"Cold climate","params":{"unit":"K","wavelength_m":70000.0,"amplitude":42.0,"offset":188.0,"octaves":4,"persistence":0.5,"seed_namespace":"climate","seed":3},"inputs":{}},
    {"id":"materials","kind":"material","label":"Ice, stone, dust","params":{"material_names":["ice","stone","dust"],"palette":[[0.58,0.68,0.74],[0.38,0.40,0.42],[0.68,0.57,0.44]],"height_transition_m":750.0,"temperature_transition_k":70.0,"humidity_bias":[0.1,-0.1,0.25],"variation_strength":0.35,"seed":9},"inputs":{"height":"height","humidity":"humidity","temperature":"temperature"}}],"outputs":{"height":"height","humidity":"humidity","temperature":"temperature","material":"materials"},"layout":{}}}
    ])
}

fn fbm(n: [f64; 3], frequency: f64, octaves: u8, persistence: f64, seed: u64) -> f64 {
    let mut total = 0.;
    let mut norm = 0.;
    let mut amp = 1.;
    for octave in 0..octaves {
        let s = seed.wrapping_add((octave as u64).wrapping_mul(0x9e3779b97f4a7c15));
        total += noise(
            [
                n[0] * frequency * 2f64.powi(octave as i32),
                n[1] * frequency * 2f64.powi(octave as i32),
                n[2] * frequency * 2f64.powi(octave as i32),
            ],
            s,
        ) * amp;
        norm += amp;
        amp *= persistence;
    }
    if norm > 0. { total / norm } else { 0. }
}
fn noise(p: [f64; 3], seed: u64) -> f64 {
    let b = p.map(f64::floor);
    let f = [p[0] - b[0], p[1] - b[1], p[2] - b[2]];
    let bi = b.map(|x| x as i64);
    let w = f.map(|x| {
        let t = x * x * x;
        t * (x * (x * 6. - 15.) + 10.)
    });
    let mut out = 0.;
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let h = mix(seed
                    ^ (bi[0] + x) as u64
                    ^ ((bi[1] + y) as u64).rotate_left(21)
                    ^ ((bi[2] + z) as u64).rotate_left(43));
                let zz = ((h >> 11) as f64) / ((1u64 << 53) as f64) * 2. - 1.;
                let aa = (h.rotate_left(17) >> 11) as f64 / ((1u64 << 53) as f64)
                    * std::f64::consts::TAU;
                let g = [
                    (1. - zz * zz).max(0.).sqrt() * aa.cos(),
                    (1. - zz * zz).max(0.).sqrt() * aa.sin(),
                    zz,
                ];
                let d = [f[0] - x as f64, f[1] - y as f64, f[2] - z as f64];
                let wx = if x == 0 { 1. - w[0] } else { w[0] };
                let wy = if y == 0 { 1. - w[1] } else { w[1] };
                let wz = if z == 0 { 1. - w[2] } else { w[2] };
                out += (g[0] * d[0] + g[1] * d[1] + g[2] * d[2]) * wx * wy * wz;
            }
        }
    }
    (out * 1.8).clamp(-1., 1.)
}
fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Graph {
        serde_json::from_value(templates()[0]["graph"].clone()).unwrap()
    }
    fn small_crater_graph() -> Graph {
        let mut graph = sample();
        graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "base")
            .unwrap()
            .params
            .insert("amplitude".into(), json!(0.0));
        for id in ["crater", "feature_mask"] {
            let node = graph.nodes.iter_mut().find(|node| node.id == id).unwrap();
            node.params.insert("radius_m".into(), json!(1.0));
            node.params.insert("depth_m".into(), json!(2.0));
            node.params.insert("rim_m".into(), json!(0.0));
        }
        graph
    }
    fn support_mask_height_graph() -> Graph {
        let mut graph = small_crater_graph();
        graph.nodes.push(
            serde_json::from_value(json!({
                "id":"flat_height","kind":"constant","label":"",
                "params":{"value":0.0,"unit":"m"},"inputs":{}
            }))
            .unwrap(),
        );
        graph.nodes.push(
            serde_json::from_value(json!({
                "id":"masked_height","kind":"constant","label":"",
                "params":{"value":100.0,"unit":"m"},"inputs":{}
            }))
            .unwrap(),
        );
        let height = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "height")
            .unwrap();
        height.kind = "blend".into();
        height.inputs = BTreeMap::from([
            ("a".into(), "flat_height".into()),
            ("b".into(), "masked_height".into()),
            ("mask".into(), "feature_mask".into()),
        ]);
        graph
    }
    #[test]
    fn catalog_has_real_templates_and_nodes() {
        assert_eq!(templates().as_array().unwrap().len(), 2);
        assert!(catalog()["node_kinds"].as_array().unwrap().len() >= 7);
    }
    #[test]
    fn typed_graph_evaluates_and_is_order_independent() {
        let g = sample();
        let a = CompiledGraph::compile(g.clone()).unwrap();
        let mut shuffled = g;
        shuffled.nodes.reverse();
        let b = CompiledGraph::compile(shuffled).unwrap();
        assert_eq!(
            a.evaluate([0., 0., 1.]).unwrap().height_m,
            b.evaluate([0., 0., 1.]).unwrap().height_m
        );
        let s = a.evaluate([0., 0., 1.]).unwrap();
        assert!(s.height_m.is_finite());
        assert!((s.material.iter().sum::<f64>() - 1.).abs() < 1e-12);
    }
    #[test]
    fn rejects_dangling_cycles_units_and_output_mismatch() {
        let mut g = sample();
        g.nodes[0].inputs.insert("a".into(), "gone".into());
        assert!(CompiledGraph::compile(g).is_err());
        let mut g = sample();
        g.nodes
            .iter_mut()
            .find(|n| n.id == "height")
            .unwrap()
            .inputs
            .insert("a".into(), "height".into());
        assert!(CompiledGraph::compile(g).is_err());
        let mut g = sample();
        g.nodes
            .iter_mut()
            .find(|n| n.id == "height")
            .unwrap()
            .inputs
            .insert("b".into(), "humidity".into());
        assert!(CompiledGraph::compile(g).is_err());
        let mut g = sample();
        g.outputs.height = "humidity".into();
        assert!(CompiledGraph::compile(g).is_err());
    }
    #[test]
    fn palette_identity_isolated_and_geometry_seed_flows() {
        let g = sample();
        let a = CompiledGraph::compile(g.clone()).unwrap();
        let mut p = g.clone();
        let palette = p.nodes.iter_mut().find(|n| n.kind == "material").unwrap();
        palette.params.insert(
            "palette".into(),
            json!([[0.1, 0.2, 0.3], [0.4, 0.5, 0.6], [0.7, 0.8, 0.9]]),
        );
        let b = CompiledGraph::compile(p).unwrap();
        assert_eq!(a.identities.geometry, b.identities.geometry);
        assert_ne!(a.identities.palette, b.identities.palette);
        let mut s = g;
        s.seeds.geometry += 1;
        let c = CompiledGraph::compile(s).unwrap();
        assert_ne!(a.identities.geometry, c.identities.geometry);
    }
    #[test]
    fn crater_support_reaches_zero_continuously() {
        let mut g = sample();
        g.nodes
            .iter_mut()
            .find(|n| n.id == "feature_mask")
            .unwrap()
            .params
            .insert("radius_m".into(), json!(100000.));
        g.nodes
            .iter_mut()
            .find(|n| n.id == "crater")
            .unwrap()
            .params
            .insert("radius_m".into(), json!(100000.));
        let c = CompiledGraph::compile(g).unwrap();
        let angle: f64 = 100000. / 1737400.;
        let edge = c.evaluate([angle.sin(), 0., angle.cos()]).unwrap().support;
        let near = c
            .evaluate([(angle * 0.999999).sin(), 0., (angle * 0.999999).cos()])
            .unwrap()
            .support;
        assert_eq!(edge, 0.0);
        assert!((0.0..1.0e-8).contains(&near));
        let edge_height = c
            .evaluate([angle.sin(), 0., angle.cos()])
            .unwrap()
            .node_values["crater"];
        let near_height = c
            .evaluate([(angle * 0.999999).sin(), 0., (angle * 0.999999).cos()])
            .unwrap()
            .node_values["crater"];
        assert_eq!(edge_height, 0.0);
        assert!(near_height.abs() < 1.0e-5);
    }
    #[test]
    fn small_crater_differential_retains_analytic_wall_slope_and_normal() {
        let graph = small_crater_graph();
        let compiled = CompiledGraph::compile(graph).unwrap();
        assert!(compiled.differential_step_rad() < 1.1e-10);
        let radius = compiled.graph().reference_radius_m;
        let theta = 0.5 / radius;
        let direction = [theta.sin(), 0.0, theta.cos()];
        let (_, differential) = compiled
            .evaluate_surface_with_differential(direction)
            .unwrap();
        let tangent_toward_center = [-direction[2], 0.0, direction[0]];
        let measured = differential
            .height_gradient_tangent_m_per_radian
            .iter()
            .zip(tangent_toward_center)
            .map(|(gradient, tangent)| gradient * tangent)
            .sum::<f64>();
        let expected = -2.0 * radius;
        assert!((measured - expected).abs() < expected.abs() * 0.03);
        let expected_normal = normalize_direction([
            direction[0] - expected * tangent_toward_center[0] / radius,
            direction[1],
            direction[2] - expected * tangent_toward_center[2] / radius,
        ])
        .unwrap();
        let normal_error = differential
            .outward_normal
            .iter()
            .zip(expected_normal)
            .map(|(actual, expected)| (actual - expected).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(normal_error < 0.03);
    }
    #[test]
    fn rejects_height_features_below_supported_angular_scale() {
        let mut graph = small_crater_graph();
        graph.reference_radius_m = 1.0e12;
        assert!(CompiledGraph::compile(graph).is_err());
    }
    #[test]
    fn support_mask_in_height_blend_plans_and_measures_its_slope_and_normal() {
        let compiled = CompiledGraph::compile(support_mask_height_graph()).unwrap();
        let radius = compiled.graph().reference_radius_m;
        let expected_step = 0.18 / radius * 1.0e-3;
        assert!((compiled.differential_step_rad() - expected_step).abs() < expected_step * 1e-12);

        let theta = 0.91 / radius;
        let direction = [theta.sin(), 0.0, theta.cos()];
        let (sample, differential) = compiled
            .evaluate_surface_with_differential(direction)
            .unwrap();
        assert!((sample.height_m - 50.0).abs() < 1e-5);
        let tangent_toward_center = [-direction[2], 0.0, direction[0]];
        let measured = differential
            .height_gradient_tangent_m_per_radian
            .iter()
            .zip(tangent_toward_center)
            .map(|(gradient, tangent)| gradient * tangent)
            .sum::<f64>();
        let expected_slope = 100.0 * (1.5 / 0.18) * radius;
        assert!((measured - expected_slope).abs() < expected_slope * 0.03);
        let expected_normal = normalize_direction([
            direction[0] - expected_slope * tangent_toward_center[0] / (radius + sample.height_m),
            direction[1],
            direction[2] - expected_slope * tangent_toward_center[2] / (radius + sample.height_m),
        ])
        .unwrap();
        let normal_error = differential
            .outward_normal
            .iter()
            .zip(expected_normal)
            .map(|(actual, expected)| (actual - expected).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(normal_error < 0.03);
    }
    #[test]
    fn rejects_small_support_mask_height_features_below_angular_limit() {
        let mut graph = support_mask_height_graph();
        graph.reference_radius_m = 1.0e12;
        assert!(CompiledGraph::compile(graph).is_err());
    }
    #[test]
    fn finest_active_noise_octave_sets_differential_step() {
        let mut graph = sample();
        let base = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "base")
            .unwrap();
        base.params.insert("amplitude".into(), json!(1.0));
        base.params.insert("wavelength_m".into(), json!(12800.0));
        base.params.insert("octaves".into(), json!(8));
        base.params.insert("persistence".into(), json!(0.5));
        let compiled = CompiledGraph::compile(graph).unwrap();
        let expected = (12800.0 / 128.0) / compiled.graph().reference_radius_m * 1.0e-3;
        assert!((compiled.differential_step_rad() - expected).abs() < expected * 1e-12);
    }
    #[test]
    fn narrow_remap_windows_reduce_scale_and_reject_unsupported_height_relief() {
        let mut graph = sample();
        let base = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "base")
            .unwrap();
        base.params.insert("amplitude".into(), json!(1.0));
        base.params.insert("wavelength_m".into(), json!(1000.0));
        let height = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "height")
            .unwrap();
        height.kind = "remap".into();
        height.inputs = BTreeMap::from([("input".into(), "base".into())]);
        height.params = Map::from_iter([
            ("in_min".into(), json!(-1e-4)),
            ("in_max".into(), json!(1e-4)),
            ("out_min".into(), json!(-100.0)),
            ("out_max".into(), json!(100.0)),
        ]);
        assert!(CompiledGraph::compile(graph).is_err());
    }
    #[test]
    fn rejects_oversized_serialized_graph_before_semantic_canonicalization() {
        let mut graph = sample();
        graph
            .layout
            .insert("oversized".into(), LayoutPoint { x: 0.0, y: 0.0 });
        graph.recipe_id = "x".repeat(MAX_GRAPH_BYTES);
        assert!(CompiledGraph::compile(graph).is_err());
    }
    #[test]
    fn shipped_graphs_and_seed_corpus_stay_finite_and_bounded() {
        for template in templates().as_array().unwrap() {
            let base: Graph = serde_json::from_value(template["graph"].clone()).unwrap();
            for seed in 0..8u64 {
                let mut g = base.clone();
                g.seeds.geometry += seed;
                g.seeds.climate += seed * 3;
                g.seeds.material += seed * 5;
                let c = CompiledGraph::compile(g).unwrap();
                for d in [
                    [0., 0., 1.],
                    [1., 0., 0.],
                    [0., 1., 0.],
                    [-1., 0., 1.],
                    [1., 1., 1.],
                    [-1., -1., -1.],
                ] {
                    let s = c.evaluate(d).unwrap();
                    assert!(s.height_m.is_finite());
                    assert!((0.0..=1.0).contains(&s.humidity));
                    assert!((0.0..=5000.0).contains(&s.temperature_k));
                    assert!(
                        s.weights
                            .iter()
                            .all(|v| v.is_finite() && *v >= 0. && *v <= 1.)
                    );
                    assert!((s.weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
                    assert!((0.0..=1.0).contains(&s.support));
                    let normal = c.normal_rgb(d).unwrap();
                    assert!(
                        normal
                            .iter()
                            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                    );
                    let differential = c.surface_differential(d).unwrap();
                    assert!(
                        differential
                            .height_gradient_tangent_m_per_radian
                            .iter()
                            .all(|v| v.is_finite())
                    );
                    let unit = d.iter().map(|v| v * v).sum::<f64>().sqrt();
                    assert!(
                        differential
                            .outward_normal
                            .iter()
                            .zip(d)
                            .map(|(a, b)| a * b / unit)
                            .sum::<f64>()
                            > 0.0
                    );
                }
            }
        }
    }
    #[test]
    fn variation_is_reproducible_respects_locks_and_survives_json_roundtrip() {
        let g = sample();
        let locks = VariationLocks {
            geometry: true,
            climate: true,
            palette: false,
        };
        let a = vary(&g, 781, &locks).unwrap();
        let b = vary(&g, 781, &locks).unwrap();
        assert_eq!(a.seeds.geometry, g.seeds.geometry);
        assert_eq!(a, b);
        assert_eq!(a.seeds.climate, g.seeds.climate);
        assert_eq!(a.seeds.material, g.seeds.material);
        let original = CompiledGraph::compile(g.clone()).unwrap();
        let changed = CompiledGraph::compile(a.clone()).unwrap();
        let sample_dir = [0.3, 0.7, 0.2];
        assert_eq!(
            original.evaluate(sample_dir).unwrap().weights,
            changed.evaluate(sample_dir).unwrap().weights
        );
        assert_ne!(original.identities.palette, changed.identities.palette);
        let restored: Graph = serde_json::from_slice(&serde_json::to_vec(&a).unwrap()).unwrap();
        let before = changed;
        let after = CompiledGraph::compile(restored).unwrap();
        assert_eq!(before.identities(), after.identities());
        assert_eq!(
            before.evaluate([0.3, 0.7, 0.2]).unwrap(),
            after.evaluate([0.3, 0.7, 0.2]).unwrap()
        );
        let all = VariationLocks {
            geometry: true,
            climate: true,
            palette: true,
        };
        assert_eq!(vary(&g, 17, &all).unwrap(), g);
    }
    #[test]
    fn disconnected_nodes_and_palette_do_not_invalidate_physical_fields() {
        let g = sample();
        let baseline = CompiledGraph::compile(g.clone()).unwrap();
        let mut disconnected = g.clone();
        disconnected.nodes.push(Node{id:"unused".into(),kind:"noise".into(),label:"unused".into(),params:serde_json::from_value(json!({"unit":"m","wavelength_m":9000.0,"amplitude":20.0,"offset":0.0,"octaves":2,"persistence":0.5,"seed_namespace":"geometry","seed":100})).unwrap(),inputs:BTreeMap::new()});
        let other = CompiledGraph::compile(disconnected).unwrap();
        assert_eq!(baseline.identities.geometry, other.identities.geometry);
        assert_eq!(baseline.identities.climate, other.identities.climate);
        let mut palette = g;
        palette
            .nodes
            .iter_mut()
            .find(|n| n.kind == "material")
            .unwrap()
            .params
            .insert(
                "palette".into(),
                json!([[0.0, 0.1, 0.2], [0.3, 0.4, 0.5], [0.6, 0.7, 0.8]]),
            );
        let recolored = CompiledGraph::compile(palette).unwrap();
        let a = baseline.evaluate([0.4, 0.5, 0.6]).unwrap();
        let b = recolored.evaluate([0.4, 0.5, 0.6]).unwrap();
        assert_eq!(a.weights, b.weights);
        assert_eq!(baseline.identities.material, recolored.identities.material);
        assert_ne!(baseline.identities.palette, recolored.identities.palette);
    }
    #[test]
    fn layout_does_not_change_semantics_and_work_is_preflighted() {
        let mut g = sample();
        let first = CompiledGraph::compile(g.clone()).unwrap();
        g.layout
            .insert("base".into(), LayoutPoint { x: 42.0, y: 17.0 });
        let second = CompiledGraph::compile(g.clone()).unwrap();
        assert_eq!(first.identities(), second.identities());
        for i in 0..120 {
            g.nodes.push(Node{id:format!("extra-{i}"),kind:"noise".into(),label:String::new(),params:serde_json::from_value(json!({"unit":"m","wavelength_m":5000.0,"amplitude":1.0,"offset":0.0,"octaves":8,"persistence":0.5,"seed_namespace":"geometry","seed":i})).unwrap(),inputs:BTreeMap::new()})
        }
        let heavy = CompiledGraph::compile(g).unwrap();
        assert!(heavy.sample_map(64, 0, None).is_err());
    }
}
