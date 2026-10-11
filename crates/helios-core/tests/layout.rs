//! Layout hints: ranks, principal nodes and the stacks under them, on the
//! landscape network of the GUI mockups.

use helios_core::*;

/// A module with `inputs` image inputs, one output, of a given kind.
struct Fake {
    inputs: usize,
    kind: ModuleKind,
}

impl Module for Fake {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "fake".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: (0..self.inputs)
                .map(|i| PortSpec::image(&format!("in{i}"), 1))
                .collect(),
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: self.kind,
            family: Family::Other,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, _: &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![inputs[0].sample(output)])
    }
}

fn add(g: &mut Graph, name: &str, inputs: usize, kind: ModuleKind) -> NodeId {
    g.add_module(name, ModuleRef::new(Fake { inputs, kind }), &Params::new())
        .unwrap()
}

#[test]
fn the_landscape_network_folds_into_six_principal_nodes() {
    use ModuleKind::*;
    let mut g = Graph::new();
    let img = || Image::from_fn(Region::sized(4, 4), 1, |_, _, _| 0.0);
    let brackets = g.add_source("brackets", img());
    let panorama = g.add_source("panorama", img());
    let hdr = add(&mut g, "hdr", 1, Process);
    let stitch = add(&mut g, "stitch", 2, Process);
    let base = add(&mut g, "base", 1, Process);
    let denoise = add(&mut g, "denoise", 1, Process);
    let sky = add(&mut g, "sky", 1, Decision);
    let local = add(&mut g, "local", 1, Process);
    let color = add(&mut g, "color", 1, Process);
    let print = add(&mut g, "print", 1, Sink);
    let web = add(&mut g, "web", 1, Sink);
    for (from, to, port) in [
        (brackets, hdr, "in0"),
        (hdr, stitch, "in0"),
        (panorama, stitch, "in1"),
        (stitch, base, "in0"),
        (base, denoise, "in0"),
        (denoise, sky, "in0"),
        (sky, local, "in0"),
        (local, color, "in0"),
        (color, print, "in0"),
        (color, web, "in0"),
    ] {
        let out = if from == brackets || from == panorama {
            "image"
        } else {
            "output"
        };
        g.connect(from, out, to, port).unwrap();
    }

    let layout = g.layout();
    let hint = |id: NodeId| layout[id.0];
    let principal: Vec<NodeId> = layout
        .iter()
        .filter(|h| h.principal)
        .map(|h| h.node)
        .collect();
    assert_eq!(principal, [brackets, panorama, stitch, sky, print, web]);
    assert_eq!(
        hint(hdr).group,
        stitch,
        "a single-input step folds into the merge it feeds"
    );
    assert_eq!((hint(base).group, hint(denoise).group), (sky, sky));
    assert_eq!(
        (hint(local).group, hint(color).group),
        (print, print),
        "first consumer wins"
    );
    assert_eq!(hint(web).group, web);
    let ranks: Vec<usize> = [brackets, panorama, hdr, stitch, sky, print]
        .iter()
        .map(|&n| hint(n).rank)
        .collect();
    assert_eq!(ranks, [0, 0, 1, 2, 5, 8]);
    assert_eq!(g.describe()["layout"][3]["principal"], true);
}
