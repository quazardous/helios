//! A macro's interface: how it shows each exposed parameter (a label, a
//! unit, a group, a narrower range, a soft range, the options kept, a
//! default), the inner parameter's hints forwarded under the exposed name,
//! and linked parameters accepting what all their inner ones accept.

use helios_core::graph::SOURCE_PORT;
use helios_core::*;

fn manifest(name: &str, params: Vec<ParamSpec>) -> Manifest {
    Manifest {
        name: name.into(),
        version: 1,
        description: String::new(),
        params,
        inputs: vec![PortSpec::image("input", 1)],
        outputs: vec![PortSpec::image("output", 1)],
        context: vec![],
        needs: Needs::default(),
        kind: ModuleKind::Process,
        family: Family::Other,
        role: Role::General,
    }
}

/// Scales its input by `gain`; `mode`, `radius` and `curve` only change
/// its key.
struct Gain {
    radius_max: i64,
}

impl Module for Gain {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        manifest(
            "gain",
            vec![
                ParamSpec::float("gain", "How much brighter", 0.0, 4.0, 1.0),
                ParamSpec {
                    name: "mode".into(),
                    description: String::new(),
                    kind: ParamKind::Choice {
                        options: vec!["linear".into(), "log".into(), "film".into()],
                        default: "linear".into(),
                    },
                },
                ParamSpec {
                    name: "radius".into(),
                    description: String::new(),
                    kind: ParamKind::Int {
                        min: 0,
                        max: self.radius_max,
                        default: 2,
                    },
                },
                ParamSpec {
                    name: "flag".into(),
                    description: String::new(),
                    kind: ParamKind::Bool { default: false },
                },
            ],
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![
            ParamHint::new("gain")
                .soft(0.5, 2.0)
                .unit("x")
                .label("Gain")
                .steps(0.01, 0.1),
        ]
    }

    #[allow(clippy::cast_possible_truncation)]
    fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
        Ok(params.get("gain").and_then(Value::as_f64).unwrap_or(1.0) as f32)
    }

    fn input_region(&self, _: &f32, output: Region, _: u32) -> Region {
        output
    }

    fn process(&self, gain: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            inputs[0].pixel(x, y)[0] * gain
        })])
    }
}

fn picture() -> Image {
    Image::from_fn(Region::new(0, 0, 32, 32), 1, |x, y, _| {
        #[allow(clippy::cast_precision_loss)]
        let v = (x * 7 + y * 3) as f32 / 256.0;
        v
    })
}

/// Two gains in a row, the second one's radius up to `radius_max`.
fn two(radius_max: i64) -> (Network, Inner, Inner) {
    let mut n = Network::with_ports(
        vec![PortSpec::image("input", 1)],
        vec![PortSpec::image("output", 1)],
    );
    let input = n.input("input").unwrap();
    let saved = Params::new().with("gain", Value::Float(1.5));
    let a = n
        .chain("a", ModuleRef::new(Gain { radius_max: 8 }), &saved, input)
        .unwrap();
    let a_out = n.output(a, "output").unwrap();
    let b = n
        .chain("b", ModuleRef::new(Gain { radius_max }), &saved, a_out)
        .unwrap();
    n.exit(n.output(b, "output").unwrap(), "output").unwrap();
    (n, a, b)
}

fn macro_of(network: Network) -> Macro {
    Macro::new(manifest("look", vec![]), network).unwrap()
}

fn param<'a>(m: &'a Manifest, name: &str) -> &'a ParamSpec {
    m.params.iter().find(|p| p.name == name).unwrap()
}

#[test]
fn an_exposed_parameter_keeps_the_inner_hints_renamed() {
    let (n, a, b) = two(8);
    let m = macro_of(n)
        .expose("gain", a, "gain")
        .unwrap()
        .expose("strength", b, "gain")
        .unwrap();
    let hints = m.param_hints();
    let gain = hints.iter().find(|h| h.name == "gain").unwrap();
    assert_eq!(gain.soft, Some((0.5, 2.0)));
    assert_eq!(gain.unit.as_deref(), Some("x"));
    assert_eq!(
        gain.label.as_deref(),
        Some("Gain"),
        "same name: the label too"
    );
    assert_eq!(gain.step, Some(0.01));
    let strength = hints.iter().find(|h| h.name == "strength").unwrap();
    assert_eq!(strength.unit.as_deref(), Some("x"));
    assert_eq!(strength.label, None, "renamed: the inner label is dropped");
    // The saved value is the default.
    assert_eq!(
        param(&m.manifest(), "strength").kind,
        ParamKind::Float {
            min: 0.0,
            max: 4.0,
            default: 1.5
        }
    );
}

#[test]
fn the_interface_narrows_renames_and_groups() {
    let (n, a, _) = two(8);
    let how = Exposure {
        label: Some("Strength".into()),
        description: Some("How strong the look is".into()),
        unit: Some("%".into()),
        group: Some("Look".into()),
        min: Some(1.0),
        max: Some(3.0),
        soft: Some((1.2, 2.5)),
        default: Some(Value::Float(2.0)),
        ..Exposure::default()
    };
    let m = macro_of(n)
        .expose_as("strength", a, "gain", &how)
        .unwrap()
        .expose_as(
            "mode",
            a,
            "mode",
            &Exposure {
                options: Some(vec!["film".into(), "log".into()]),
                group: Some("Look".into()),
                ..Exposure::default()
            },
        )
        .unwrap()
        .expose_as(
            "radius",
            a,
            "radius",
            &Exposure {
                min: Some(1.0),
                max: Some(1.0),
                ..Exposure::default()
            },
        )
        .unwrap();
    let manifest = m.manifest();
    let order: Vec<&str> = manifest.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(order, ["strength", "mode", "radius"], "the order exposed");
    let strength = param(&manifest, "strength");
    assert_eq!(strength.description, "How strong the look is");
    assert_eq!(
        strength.kind,
        ParamKind::Float {
            min: 1.0,
            max: 3.0,
            default: 2.0
        }
    );
    assert_eq!(
        param(&manifest, "mode").kind,
        ParamKind::Choice {
            options: vec!["film".into(), "log".into()],
            default: "film".into(),
        },
        "a subset in the order given; the inner default dropped, the first kept"
    );
    assert_eq!(
        param(&manifest, "radius").kind,
        ParamKind::Int {
            min: 1,
            max: 1,
            default: 1
        },
        "the inner default clamped into the narrowed range"
    );
    let hints = m.param_hints();
    let h = &hints[0];
    assert_eq!(h.label.as_deref(), Some("Strength"));
    assert_eq!(h.unit.as_deref(), Some("%"));
    assert_eq!(h.group.as_deref(), Some("Look"));
    assert_eq!(h.soft, Some((1.2, 2.5)));
    assert_eq!(hints[1].group.as_deref(), Some("Look"));

    // In a graph: the narrowed range is the macro's, a value outside is
    // refused like any parameter's, and the value reaches the inner node.
    let mut g = Graph::new();
    let src = g.add_source("src", picture());
    let node = g
        .add_module("look", ModuleRef::composite(m), &Params::new())
        .unwrap();
    g.connect(src, SOURCE_PORT, node, "input").unwrap();
    assert!(g.set_param(node, "strength", Value::Float(3.5)).is_err());
    assert!(
        g.set_param(node, "mode", Value::Choice("linear".into()))
            .is_err()
    );
    g.set_param(node, "strength", Value::Float(3.0)).unwrap();
    let e = Engine::new(std::sync::Arc::new(Bus::new()));
    let region = Region::new(0, 0, 8, 8);
    let out = e.render(&g, node, "output", region).unwrap();
    let want = picture().sample(region);
    for (o, w) in out.data().iter().zip(want.data()) {
        // a's gain 3.0, b's saved 1.5.
        assert!((o - w * 4.5).abs() < 1e-5, "{o} vs {w}");
    }
}

#[test]
fn a_soft_range_inherited_is_kept_within_the_narrowed_range() {
    let (n, a, _) = two(8);
    let m = macro_of(n)
        .expose_as(
            "gain",
            a,
            "gain",
            &Exposure {
                min: Some(1.0),
                max: Some(4.0),
                ..Exposure::default()
            },
        )
        .unwrap();
    assert_eq!(m.param_hints()[0].soft, Some((1.0, 2.0)));
    let (n, a, _) = two(8);
    let m = macro_of(n)
        .expose_as(
            "gain",
            a,
            "gain",
            &Exposure {
                min: Some(3.0),
                ..Exposure::default()
            },
        )
        .unwrap();
    assert_eq!(m.param_hints()[0].soft, None, "nothing of it left");
}

#[test]
fn an_interface_that_does_not_fit_is_refused_with_its_reason() {
    let refused = |param: &str, how: Exposure| {
        let (n, a, _) = two(8);
        macro_of(n)
            .expose_as("x", a, param, &how)
            .err()
            .unwrap_or_else(|| panic!("{param} {how:?} was accepted"))
            .to_string()
    };
    let e = refused(
        "gain",
        Exposure {
            max: Some(5.0),
            ..Exposure::default()
        },
    );
    assert!(e.contains("not within the inner [0, 4]"), "{e}");
    let e = refused(
        "gain",
        Exposure {
            min: Some(2.0),
            max: Some(1.0),
            ..Exposure::default()
        },
    );
    assert!(e.contains("not within"), "{e}");
    let e = refused(
        "radius",
        Exposure {
            max: Some(2.5),
            ..Exposure::default()
        },
    );
    assert!(e.contains("not a whole number"), "{e}");
    let e = refused(
        "flag",
        Exposure {
            min: Some(0.0),
            ..Exposure::default()
        },
    );
    assert!(e.contains("for numbers"), "{e}");
    let e = refused(
        "gain",
        Exposure {
            options: Some(vec!["a".into()]),
            ..Exposure::default()
        },
    );
    assert!(e.contains("for a choice"), "{e}");
    let e = refused(
        "mode",
        Exposure {
            options: Some(vec!["film".into(), "sepia".into()]),
            ..Exposure::default()
        },
    );
    assert!(e.contains("`sepia` is not one of"), "{e}");
    let e = refused(
        "mode",
        Exposure {
            options: Some(vec![]),
            ..Exposure::default()
        },
    );
    assert!(e.contains("no option kept"), "{e}");
    let e = refused(
        "gain",
        Exposure {
            max: Some(2.0),
            default: Some(Value::Float(3.0)),
            ..Exposure::default()
        },
    );
    assert!(e.contains("default"), "{e}");
    let e = refused(
        "gain",
        Exposure {
            default: Some(Value::Int(1)),
            ..Exposure::default()
        },
    );
    assert!(e.contains("default"), "{e}");
    let e = refused(
        "gain",
        Exposure {
            max: Some(2.0),
            soft: Some((0.5, 3.0)),
            ..Exposure::default()
        },
    );
    assert!(e.contains("soft range"), "{e}");
    let e = refused(
        "flag",
        Exposure {
            soft: Some((0.0, 1.0)),
            ..Exposure::default()
        },
    );
    assert!(e.contains("soft range is for numbers"), "{e}");
}

#[test]
fn linked_parameters_accept_what_all_their_inner_ones_accept() {
    // a's radius 0..8, b's 0..4: the link is 0..4.
    let (n, a, b) = two(4);
    let m = macro_of(n)
        .expose("radius", a, "radius")
        .unwrap()
        .expose("radius", b, "radius")
        .unwrap();
    assert_eq!(
        param(&m.manifest(), "radius").kind,
        ParamKind::Int {
            min: 0,
            max: 4,
            default: 2
        }
    );
    // The interface is given on the first entry only.
    let (n, a, b) = two(8);
    let e = macro_of(n)
        .expose("radius", a, "radius")
        .unwrap()
        .expose_as(
            "radius",
            b,
            "radius",
            &Exposure {
                label: Some("Radius".into()),
                ..Exposure::default()
            },
        )
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("first entry"), "{e}");
    // A narrowed range that misses the other's is refused.
    let (n, a, b) = two(2);
    let e = macro_of(n)
        .expose_as(
            "radius",
            a,
            "radius",
            &Exposure {
                min: Some(5.0),
                ..Exposure::default()
            },
        )
        .unwrap()
        .expose("radius", b, "radius")
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("misses"), "{e}");
}
