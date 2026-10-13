//! A node standing for a network: a macro (a saved sub-network with a few
//! exposed parameters) and a multi-kernel module (a network declared in
//! code). Both expand into inner steps when a render is planned: each with
//! its own key, region and cache entry, an inner node read twice computed
//! once over the union of its reads, unread inner nodes never computed,
//! and a tiled render equal to one render.

use std::sync::Arc;

use helios_core::graph::SOURCE_PORT;
use helios_core::*;

fn manifest(name: &str, inputs: Vec<PortSpec>, params: Vec<ParamSpec>) -> Manifest {
    Manifest {
        name: name.into(),
        version: 1,
        description: String::new(),
        params,
        inputs,
        outputs: vec![PortSpec::image("output", 1)],
        context: vec![],
        needs: Needs::default(),
        kind: ModuleKind::Process,
        family: Family::Other,
        role: Role::General,
    }
}

/// The mean over a square of `radius` pixels around each pixel.
struct BoxBlur;

impl Module for BoxBlur {
    type Plan = u32;

    fn manifest(&self) -> Manifest {
        manifest(
            "box",
            vec![PortSpec::image("input", 1)],
            vec![ParamSpec {
                name: "radius".into(),
                description: String::new(),
                kind: ParamKind::Int {
                    min: 0,
                    max: 8,
                    default: 1,
                },
            }],
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<u32, String> {
        Ok(params.get("radius").and_then(Value::as_i64).unwrap_or(0) as u32)
    }

    fn input_region(&self, radius: &u32, output: Region, _: u32) -> Region {
        output.grow(*radius)
    }

    fn process(
        &self,
        radius: &u32,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let r = i64::from(*radius);
        let n = ((2 * r + 1) * (2 * r + 1)) as f32;
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let mut sum = 0.0;
            for dy in -r..=r {
                for dx in -r..=r {
                    sum += inputs[0].pixel(x + dx, y + dy)[0];
                }
            }
            sum / n
        })])
    }
}

/// Half the resolution: each pixel the mean of the 2x2 it covers.
struct Down;

impl Module for Down {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        manifest("down", vec![PortSpec::image("input", 1)], vec![])
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn input_region(&self, (): &(), r: Region, _: u32) -> Region {
        Region::new(2 * r.x, 2 * r.y, 2 * r.width, 2 * r.height)
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let i = inputs[0];
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            (i.pixel(2 * x, 2 * y)[0]
                + i.pixel(2 * x + 1, 2 * y)[0]
                + i.pixel(2 * x, 2 * y + 1)[0]
                + i.pixel(2 * x + 1, 2 * y + 1)[0])
                / 4.0
        })])
    }
}

/// Twice the resolution, nearest.
struct Up;

impl Module for Up {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        manifest("up", vec![PortSpec::image("input", 1)], vec![])
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn input_region(&self, (): &(), r: Region, _: u32) -> Region {
        let (x0, y0) = (r.x.div_euclid(2), r.y.div_euclid(2));
        let (x1, y1) = (
            (r.right() - 1).div_euclid(2),
            (r.bottom() - 1).div_euclid(2),
        );
        Region::new(x0, y0, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32)
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            inputs[0].pixel(x.div_euclid(2), y.div_euclid(2))[0]
        })])
    }
}

/// `a - b`.
struct Diff;

impl Module for Diff {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        manifest(
            "diff",
            vec![PortSpec::image("a", 1), PortSpec::image("b", 1)],
            vec![],
        )
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            inputs[0].pixel(x, y)[0] - inputs[1].pixel(x, y)[0]
        })])
    }
}

/// A multi-kernel module: the detail of its input, `input - up(blur(down(input)))`,
/// the blur's radius its parameter. Its `unused` inner node is wired but
/// no output reads it.
struct Detail;

impl Composite for Detail {
    fn manifest(&self) -> Manifest {
        manifest(
            "detail",
            vec![PortSpec::image("input", 1)],
            vec![ParamSpec {
                name: "radius".into(),
                description: String::new(),
                kind: ParamKind::Int {
                    min: 0,
                    max: 4,
                    default: 1,
                },
            }],
        )
    }

    fn expand(&self, params: &Params, _: &Context, _: &Expansion) -> Result<Network, String> {
        let build = || -> Result<Network, Error> {
            let mut n = Network::new(&self.manifest());
            let input = n.input("input")?;
            let down = n.chain("down", ModuleRef::new(Down), &Params::new(), input)?;
            let radius = Params::new().with(
                "radius",
                Value::Int(params.get("radius").and_then(Value::as_i64).unwrap_or(0)),
            );
            let blur = n.chain(
                "blur",
                ModuleRef::new(BoxBlur),
                &radius,
                n.output(down, "output")?,
            )?;
            let up = n.chain(
                "up",
                ModuleRef::new(Up),
                &Params::new(),
                n.output(blur, "output")?,
            )?;
            n.chain("unused", ModuleRef::new(BoxBlur), &Params::new(), input)?;
            let diff = n.add("diff", ModuleRef::new(Diff), &Params::new())?;
            n.connect(input, diff, "a")?;
            n.connect(n.output(up, "output")?, diff, "b")?;
            n.exit(n.output(diff, "output")?, "output")?;
            Ok(n)
        };
        build().map_err(|e| e.to_string())
    }
}

fn picture() -> Image {
    Image::from_fn(Region::sized(64, 48), 1, |x, y, _| {
        ((x * 7 + y * 13) % 31) as f32 + (x as f32 * 0.37).sin()
    })
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

/// `src -> module` in a fresh graph.
fn one(module: ModuleRef, params: &Params) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source("src", picture());
    let n = g.add_module("m", module, params).unwrap();
    g.connect(src, SOURCE_PORT, n, "input").unwrap();
    (g, n)
}

fn blur_twice() -> (Network, Inner, Inner) {
    let mut n = Network::with_ports(
        vec![PortSpec::image("input", 1)],
        vec![PortSpec::image("output", 1)],
    );
    let input = n.input("input").unwrap();
    let r2 = Params::new().with("radius", Value::Int(2));
    let a = n.chain("a", ModuleRef::new(BoxBlur), &r2, input).unwrap();
    let b = n
        .chain(
            "b",
            ModuleRef::new(BoxBlur),
            &r2,
            n.output(a, "output").unwrap(),
        )
        .unwrap();
    n.exit(n.output(b, "output").unwrap(), "output").unwrap();
    (n, a, b)
}

fn macro_of(network: Network) -> Macro {
    Macro::new(manifest("blur_twice", vec![], vec![]), network).unwrap()
}

#[test]
fn a_macro_renders_as_its_flat_network_and_shares_its_content() {
    let (network, a, b) = blur_twice();
    let m = macro_of(network)
        .expose("radius", a, "radius")
        .unwrap()
        .expose("radius", b, "radius")
        .unwrap();
    assert_eq!(m.manifest().params.len(), 1, "one name links both");
    let module = ModuleRef::composite(m);
    let (g, node) = one(module, &Params::new());

    // The same network, flat.
    let mut flat = Graph::new();
    let src = flat.add_source("src", picture());
    let r2 = Params::new().with("radius", Value::Int(2));
    let fa = flat.add_module("a", ModuleRef::new(BoxBlur), &r2).unwrap();
    let fb = flat.add_module("b", ModuleRef::new(BoxBlur), &r2).unwrap();
    flat.connect(src, SOURCE_PORT, fa, "input").unwrap();
    flat.connect(fa, "output", fb, "input").unwrap();

    let e = engine();
    let region = Region::new(8, 8, 32, 24);
    let packed = e.render(&g, node, "output", region).unwrap();
    assert_eq!(e.counters().computed, 3, "the source and two inner steps");
    let before = e.counters();
    let unpacked = e.render(&flat, fb, "output", region).unwrap();
    assert_eq!(packed.data(), unpacked.data());
    assert_eq!(
        e.counters().since(&before).computed,
        0,
        "the flat network's steps are the macro's inner steps"
    );
    assert_eq!(
        e.key(&g, node, region).unwrap(),
        e.key(&flat, fb, region).unwrap()
    );
}

#[test]
fn an_exposed_parameter_reaches_its_inner_nodes() {
    let (network, a, b) = blur_twice();
    let m = ModuleRef::composite(
        macro_of(network)
            .expose("radius", a, "radius")
            .unwrap()
            .expose("radius", b, "radius")
            .unwrap(),
    );
    let (mut g, node) = one(m, &Params::new().with("radius", Value::Int(0)));
    let e = engine();
    let region = Region::new(4, 4, 16, 16);
    // Radius 0 on both: the identity.
    assert_eq!(
        e.render(&g, node, "output", region).unwrap().data(),
        picture().sample(region).data()
    );
    g.set_param(node, "radius", Value::Int(3)).unwrap();
    assert_ne!(
        e.render(&g, node, "output", region).unwrap().data(),
        picture().sample(region).data()
    );
    // Out of the inner parameter's range: refused like any parameter.
    assert!(g.set_param(node, "radius", Value::Int(9)).is_err());
    // A name linked to a parameter of another kind is refused.
    let (network, a, _) = blur_twice();
    let mut other = Network::with_ports(
        vec![PortSpec::image("input", 1)],
        vec![PortSpec::image("output", 1)],
    );
    let i = other.input("input").unwrap();
    let x = other
        .chain("x", ModuleRef::composite(Detail), &Params::new(), i)
        .unwrap();
    other
        .exit(other.output(x, "output").unwrap(), "output")
        .unwrap();
    let mut both = Network::with_ports(
        vec![PortSpec::image("input", 1)],
        vec![PortSpec::image("output", 1)],
    );
    let i = both.input("input").unwrap();
    let blur = both
        .chain(
            "blur",
            network.nodes()[a.0].module.clone(),
            &Params::new(),
            i,
        )
        .unwrap();
    both.exit(both.output(blur, "output").unwrap(), "output")
        .unwrap();
    assert!(macro_of(both).expose("radius", blur, "nope").is_err());
}

/// [`Detail`], declared to need full resolution.
struct FullDetail;

impl Composite for FullDetail {
    fn manifest(&self) -> Manifest {
        let mut m = Detail.manifest();
        m.name = "full_detail".into();
        m.needs.full_resolution = true;
        m
    }

    fn expand(
        &self,
        params: &Params,
        context: &Context,
        at: &Expansion,
    ) -> Result<Network, String> {
        Detail.expand(params, context, at)
    }
}

#[test]
fn a_composite_needing_full_resolution_is_reduced_at_a_scale() {
    let (g, node) = one(ModuleRef::composite(FullDetail), &Params::new());
    let e = engine();
    let full = e.render(&g, node, "output", Region::sized(64, 48)).unwrap();
    let half = e
        .render_at(&g, node, "output", Region::sized(32, 24), 0.5)
        .unwrap();
    assert_eq!(
        half.data(),
        full.sample_at(Region::sized(32, 24), 0.5).data()
    );
}

#[test]
fn a_multi_kernel_module_tiles_like_one_render_and_skips_unread_nodes() {
    let (g, node) = one(ModuleRef::composite(Detail), &Params::new());
    let region = Region::new(3, 5, 40, 30);
    let e = engine();
    let whole = e.render(&g, node, "output", region).unwrap();
    // source, down, blur, up, diff: the unused node is never planned.
    assert_eq!(e.counters().computed, 5);
    let names: Vec<String> = e
        .status(&g, node, region)
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["src", "m/down", "m/blur", "m/up", "m/diff"]);
    for tile in region.tiles(7) {
        let part = engine().render(&g, node, "output", tile).unwrap();
        assert_eq!(part.data(), whole.sample(tile).data(), "tile {tile}");
    }
    // What it reads of its input: its region, grown by what the half
    // resolution blur reaches.
    let reads = ModuleRef::composite(Detail)
        .reads(&Params::new(), &Context::new(), 1.0, region)
        .unwrap();
    assert!(reads.contains(&region) && reads != region, "{reads}");
}

#[test]
fn an_inner_node_read_twice_is_computed_once_over_the_union() {
    // The source read by the composite's `diff` and `down`, the half
    // resolution steps once each, whatever the tiles.
    let (g, node) = one(ModuleRef::composite(Detail), &Params::new());
    let e = engine();
    e.render(&g, node, "output", Region::sized(32, 32)).unwrap();
    let c = e.counters();
    assert_eq!(c.source_reads, 1, "{c:?}");
    // A second render inside the first: cropped, nothing computed.
    e.render(&g, node, "output", Region::new(4, 4, 8, 8))
        .unwrap();
    assert_eq!(e.counters().since(&c).computed, 0);
}

#[test]
fn macros_nest_and_a_composite_runs_at_a_scale() {
    // A macro holding the multi-kernel module, then a blur.
    let mut n = Network::with_ports(
        vec![PortSpec::image("input", 1)],
        vec![PortSpec::image("output", 1)],
    );
    let input = n.input("input").unwrap();
    let d = n
        .chain(
            "detail",
            ModuleRef::composite(Detail),
            &Params::new(),
            input,
        )
        .unwrap();
    let b = n
        .chain(
            "soft",
            ModuleRef::new(BoxBlur),
            &Params::new(),
            n.output(d, "output").unwrap(),
        )
        .unwrap();
    n.exit(n.output(b, "output").unwrap(), "output").unwrap();
    let m = ModuleRef::composite(macro_of(n).expose("detail_radius", d, "radius").unwrap());
    let (g, node) = one(m, &Params::new());
    let e = engine();
    let names: Vec<String> = e
        .status(&g, node, Region::sized(16, 16))
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.contains(&"m/detail/blur".to_string()), "{names:?}");
    let region = Region::sized(24, 20);
    let half = e.render_at(&g, node, "output", region, 0.5).unwrap();
    assert!(half.data().iter().all(|v| v.is_finite()));
    for tile in region.tiles(9) {
        let part = engine().render_at(&g, node, "output", tile, 0.5).unwrap();
        assert_eq!(part.data(), half.sample(tile).data(), "tile {tile}");
    }
}

#[test]
fn a_network_is_checked() {
    let mut n = Network::with_ports(
        vec![PortSpec::image("input", 1)],
        vec![PortSpec::image("output", 1)],
    );
    let diff = n.add("diff", ModuleRef::new(Diff), &Params::new()).unwrap();
    n.connect(n.input("input").unwrap(), diff, "a").unwrap();
    // No exit yet.
    assert!(matches!(
        Macro::new(manifest("m", vec![], vec![]), n.clone()),
        Err(Error::Contract { .. })
    ));
    n.exit(n.output(diff, "output").unwrap(), "output").unwrap();
    // `b` is not wired.
    assert!(matches!(
        Macro::new(manifest("m", vec![], vec![]), n.clone()),
        Err(Error::Unconnected { .. })
    ));
    // A node reads only nodes added before it.
    let later = n.add("later", ModuleRef::new(Up), &Params::new()).unwrap();
    assert!(matches!(
        n.connect(n.output(later, "output").unwrap(), diff, "b"),
        Err(Error::Cycle { .. })
    ));
    // Channels must agree.
    let three = Network::with_ports(
        vec![PortSpec::image("input", 3)],
        vec![PortSpec::image("output", 1)],
    );
    let mut three = three;
    let up = three.add("up", ModuleRef::new(Up), &Params::new()).unwrap();
    assert!(matches!(
        three.connect(three.input("input").unwrap(), up, "input"),
        Err(Error::Contract { .. })
    ));
}
