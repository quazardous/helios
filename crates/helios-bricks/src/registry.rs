//! The brick registry: every registered brick by `id@major`, checked when
//! registered, and read as plain JSON by people and agents
//! ([`BrickRegistry::catalogue`], [`BrickRegistry::describe`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use helios_core::{BrickSpec, Contract, ModuleRef, PortType, Shape};
use serde_json::{Value as Json, json};

use crate::values::ValueBrick;

/// A registered brick: an image brick (a module offering the brick
/// contract) or a value brick.
#[derive(Clone)]
pub enum Brick {
    Image(ModuleRef),
    Value(Arc<dyn ValueBrick>),
}

impl Brick {
    pub fn spec(&self) -> BrickSpec {
        match self {
            Brick::Image(m) => m.brick().cloned().expect("checked when registered"),
            Brick::Value(v) => v.spec(),
        }
    }
}

/// Bricks by `id@major`.
#[derive(Clone, Default)]
pub struct BrickRegistry {
    bricks: BTreeMap<String, Brick>,
}

impl BrickRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Helios's core bricks (`core.*`).
    pub fn core() -> Self {
        let mut registry = Self::new();
        registry
            .register(ModuleRef::new(crate::affine::Affine))
            .expect("core.affine is a valid brick");
        registry
            .register(ModuleRef::new(crate::matrix3::Matrix3))
            .expect("core.matrix3 is a valid brick");
        for brick in crate::values::core() {
            registry
                .register_value(Arc::from(brick))
                .expect("the core value bricks are valid");
        }
        registry
    }

    /// Registers an image brick: a module that declares itself a brick,
    /// named `id` at version `major` in its manifest (so two majors never
    /// share a cache entry), with every port described and typed as the
    /// manifest's.
    pub fn register(&mut self, module: ModuleRef) -> Result<(), Vec<String>> {
        let Some(spec) = module.brick().cloned() else {
            return Err(vec![format!(
                "`{}` does not declare itself a brick",
                module.manifest().name
            )]);
        };
        let mut problems = spec.problems();
        let manifest = module.manifest();
        if !module.provides().contains(&Contract::Brick) {
            problems.push(format!("`{}` does not provide the brick contract", spec.id));
        }
        if manifest.name != spec.id || manifest.version != spec.major {
            problems.push(format!(
                "`{}`: its manifest is `{}` version {}, not the brick's id and major",
                spec.id, manifest.name, manifest.version
            ));
        }
        if spec.shape == Shape::Value {
            problems.push(format!("`{}`: an image brick has an image shape", spec.id));
        }
        for (side, described, declared) in [
            ("input", &spec.inputs, &manifest.inputs),
            ("output", &spec.outputs, &manifest.outputs),
        ] {
            let names = |ports: Vec<&String>| ports.into_iter().cloned().collect::<Vec<_>>();
            if names(described.iter().map(|p| &p.name).collect())
                != names(declared.iter().map(|p| &p.name).collect())
            {
                problems.push(format!(
                    "`{}`: its {side} ports are not described as declared",
                    spec.id
                ));
                continue;
            }
            for (d, m) in described.iter().zip(declared) {
                let channels = match d.kind {
                    PortType::Image { channels, .. } => Some(channels),
                    PortType::Mask => Some(1),
                    PortType::Scalar | PortType::Bool => None,
                };
                if channels != m.channels() {
                    problems.push(format!(
                        "`{}`: {side} `{}` is described with {channels:?} channels, \
                         declared with {:?}",
                        spec.id,
                        d.name,
                        m.channels()
                    ));
                }
            }
        }
        self.insert(spec, Brick::Image(module), problems)
    }

    /// Registers a value brick: shape [`Shape::Value`], scalar or flag
    /// ports, one output.
    pub fn register_value(&mut self, brick: Arc<dyn ValueBrick>) -> Result<(), Vec<String>> {
        let spec = brick.spec();
        let mut problems = spec.problems();
        if spec.shape != Shape::Value {
            problems.push(format!("`{}`: a value brick has the value shape", spec.id));
        }
        if spec.outputs.len() != 1 {
            problems.push(format!("`{}`: a value brick has one output", spec.id));
        }
        if spec
            .inputs
            .iter()
            .chain(&spec.outputs)
            .any(|p| !matches!(p.kind, PortType::Scalar | PortType::Bool))
        {
            problems.push(format!(
                "`{}`: a value brick reads and gives values",
                spec.id
            ));
        }
        self.insert(spec, Brick::Value(brick), problems)
    }

    fn insert(
        &mut self,
        spec: BrickSpec,
        brick: Brick,
        mut problems: Vec<String>,
    ) -> Result<(), Vec<String>> {
        let key = spec.versioned_id();
        if self.bricks.contains_key(&key) {
            problems.push(format!("`{key}` is registered already"));
        }
        if problems.is_empty() {
            self.bricks.insert(key, brick);
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// A brick by `id@major`.
    pub fn get(&self, versioned_id: &str) -> Option<&Brick> {
        self.bricks.get(versioned_id)
    }

    /// Every brick, by `id@major`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Brick)> {
        self.bricks.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Every brick in a line each: id, family, shape, purpose, ports and
    /// forms. Plain JSON, the same wherever it is asked.
    pub fn catalogue(&self) -> Json {
        Json::Array(
            self.bricks
                .iter()
                .map(|(key, brick)| {
                    let spec = brick.spec();
                    let ports = |ports: &[helios_core::BrickPort]| {
                        ports
                            .iter()
                            .map(|p| format!("{}: {}", p.name, port_word(&p.kind)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    json!({
                        "id": key,
                        "family": spec.family,
                        "shape": spec.shape,
                        "purpose": spec.purpose,
                        "ports": format!("({}) -> ({})", ports(&spec.inputs), ports(&spec.outputs)),
                        "forms": forms(brick),
                    })
                })
                .collect(),
        )
    }

    /// Everything about one brick, by `id@major`, or by `id` for its
    /// newest major: its declaration, parameters with their ranges, the
    /// forms it is available in and the majors registered.
    pub fn describe(&self, id: &str) -> Option<Json> {
        let (key, brick) = match self.bricks.get_key_value(id) {
            Some(found) => found,
            None => self
                .bricks
                .iter()
                .filter(|(_, b)| b.spec().id == id)
                .max_by_key(|(_, b)| b.spec().major)?,
        };
        let spec = brick.spec();
        let majors: Vec<u32> = self
            .bricks
            .values()
            .map(Brick::spec)
            .filter(|s| s.id == spec.id)
            .map(|s| s.major)
            .collect();
        let params = match brick {
            Brick::Image(m) => serde_json::to_value(&m.manifest().params).unwrap_or(Json::Null),
            Brick::Value(_) => json!([]),
        };
        let mut out = serde_json::to_value(&spec).unwrap_or(Json::Null);
        if let Json::Object(map) = &mut out {
            map.insert("versioned_id".into(), json!(key));
            map.insert("params".into(), params);
            map.insert("forms".into(), forms(brick));
            map.insert("majors".into(), json!(majors));
        }
        Some(out)
    }
}

fn port_word(kind: &PortType) -> String {
    match kind {
        PortType::Image { channels, .. } => format!("image({channels})"),
        PortType::Mask => "mask".into(),
        PortType::Scalar => "scalar".into(),
        PortType::Bool => "bool".into(),
    }
}

fn forms(brick: &Brick) -> Json {
    match brick {
        Brick::Image(m) => json!(m.forms()),
        Brick::Value(_) => json!([helios_core::Form::Cpu]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::{
        Context, Image, Manifest, Module, ModuleKind, Needs, Params, PortSpec, Region,
    };

    #[test]
    fn the_core_catalogue_lists_every_brick_in_a_line() {
        let registry = BrickRegistry::core();
        let catalogue = registry.catalogue();
        let ids: Vec<&str> = catalogue
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"core.affine@1"), "{ids:?}");
        assert!(ids.contains(&"core.value.exp2@1"), "{ids:?}");
        let affine = &catalogue[ids.iter().position(|i| *i == "core.affine@1").unwrap()];
        assert_eq!(affine["ports"], "(input: image(4)) -> (output: image(4))");
        assert_eq!(affine["forms"], json!(["cpu", "gpu"]));
        assert_eq!(affine["shape"], "pointwise");
    }

    #[test]
    fn describe_finds_a_brick_with_or_without_its_major() {
        let registry = BrickRegistry::core();
        let full = registry.describe("core.affine").unwrap();
        assert_eq!(full, registry.describe("core.affine@1").unwrap());
        assert_eq!(full["majors"], json!([1]));
        assert_eq!(full["params"][1]["name"], "scale");
        assert!(registry.describe("core.nothing").is_none());
    }

    /// A module that is not a brick, and one whose manifest is not named
    /// as the brick.
    struct Plain;

    impl Module for Plain {
        type Plan = ();
        fn manifest(&self) -> Manifest {
            Manifest {
                name: "plain".into(),
                version: 1,
                description: String::new(),
                params: vec![],
                inputs: vec![PortSpec::image("input", 4)],
                outputs: vec![PortSpec::image("output", 4)],
                context: vec![],
                needs: Needs::default(),
                kind: ModuleKind::Process,
                family: helios_core::Family::default(),
                role: helios_core::Role::default(),
            }
        }
        fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
            Ok(())
        }
        fn process(&self, (): &(), i: &[&Image], _: Region) -> Result<Vec<Image>, String> {
            Ok(vec![i[0].clone()])
        }
    }

    struct Misnamed;

    impl Module for Misnamed {
        type Plan = ();
        fn manifest(&self) -> Manifest {
            Plain.manifest()
        }
        fn brick(&self) -> Option<BrickSpec> {
            crate::affine::Affine.brick()
        }
        fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
            Ok(())
        }
        fn process(&self, (): &(), i: &[&Image], _: Region) -> Result<Vec<Image>, String> {
            Ok(vec![i[0].clone()])
        }
    }

    #[test]
    fn registering_checks_the_declaration() {
        let mut registry = BrickRegistry::core();
        let err = registry.register(ModuleRef::new(Plain)).unwrap_err();
        assert!(
            err[0].contains("does not declare itself a brick"),
            "{err:?}"
        );
        let err = registry.register(ModuleRef::new(Misnamed)).unwrap_err();
        assert!(
            err.iter()
                .any(|e| e.contains("not the brick's id and major")),
            "{err:?}"
        );
        let err = registry
            .register(ModuleRef::new(crate::affine::Affine))
            .unwrap_err();
        assert!(
            err.iter().any(|e| e.contains("registered already")),
            "{err:?}"
        );
        // A module is a node and a process by default; a brick says so.
        assert_eq!(
            ModuleRef::new(Plain).provides(),
            [Contract::Node, Contract::Process]
        );
        assert_eq!(
            ModuleRef::new(Misnamed).provides(),
            [Contract::Brick, Contract::Node, Contract::Process]
        );
    }
}
