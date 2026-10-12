//! A large mask network, for stress tests and the masks bench.
//!
//! [`build`] chains masked modules: each one reads the previous one's
//! output, and its mask is a tree of three groups over 18 mask nodes:
//!
//! ```text
//! input -> parametric (luminance) -> feather (guide: input) -> in0 \
//! input -> parametric (red) -> blur ---------------------------> in1  > group A -> blur -> levels -> in0 \
//! input -> parametric (chroma) --------------------------------> in2 /                               |
//! input -> mask_from_luma -> blur -----------------------------> in0 \                               |
//! previous module's mask -> raster (first module: green) ------> in1  > group B -> levels ---------> in1  > root -> levels -> blend.mask
//! input -> parametric (blue, inverted) ------------------------> in2 /                               |
//! input -> parametric (lightness) ------------------------------------------------------------------> in2 /
//! input -> blend.input, input -> the processed module -> blend.processed
//! ```
//!
//! With shapes ([`build_with_shapes`]), each module's root gets a fourth
//! member, a group of four drawn shapes (circle, ellipse, gradient, path)
//! placed across the image:
//!
//! ```text
//! circle, ellipse, gradient (inverted), path -> group S -> root.in3
//! ```
//!
//! The nine groups of three modules use all seven operators ([`OPERATORS`]).
//! Every module after the first reuses the previous module's mask through
//! [`crate::MaskRaster`], so the chain is wired both through its images
//! and through its masks, as darktable's raster masks are.

use helios_core::{Error, Graph, Image, ModuleRef, NodeId, Params, Value};

use crate::shape::path_values;
use crate::{
    MAX_MEMBERS, MaskBlend, MaskBlur, MaskCircle, MaskCombine, MaskEllipse, MaskFeather,
    MaskFromLuma, MaskGradient, MaskLevels, MaskParametric, MaskPath, MaskRaster, PathPoint,
};

/// Mask nodes per masked module (the processed module is not one).
pub const MASK_NODES_PER_MODULE: usize = 18;

/// Mask nodes the shapes add to each masked module: four shapes and their
/// group.
pub const SHAPE_NODES_PER_MODULE: usize = 5;

/// The operator of each module's shape group, by module (cycling).
pub const SHAPE_OPERATORS: [&str; 3] = ["maximum", "difference", "minimum"];

/// The operators of groups A, B and the root, by module (cycling): all
/// seven, with the root's keeping the mask's range wide.
pub const OPERATORS: [[&str; 3]; 3] = [
    ["maximum", "minimum", "screen"],
    ["product", "sum", "maximum"],
    ["exclusion", "difference", "screen"],
];

/// A built stress network.
pub struct Stress {
    pub graph: Graph,
    /// The image every chain starts from.
    pub source: NodeId,
    /// Each masked module's blend (its output, port `output`), in chain
    /// order: the last one is the network's result.
    pub outputs: Vec<NodeId>,
    /// Each masked module's finished mask (port `mask`).
    pub masks: Vec<NodeId>,
    /// Mask nodes in the network.
    pub mask_nodes: usize,
}

impl Stress {
    /// The network's result: the last module's blend.
    pub fn output(&self) -> NodeId {
        *self.outputs.last().expect("at least one module")
    }
}

/// Builds `modules` masked modules over `image` (four channels). The
/// processed module of each comes from `processed`, given the module's
/// index: a module with an `input` and an `output` port of four channels.
/// Nodes are named `m{index}.{part}`.
pub fn build(
    image: Image,
    modules: usize,
    processed: impl Fn(usize) -> (ModuleRef, Params),
) -> Result<Stress, Error> {
    build_inner(image, modules, processed, false)
}

/// [`build`], each module's mask also folding a group of four drawn
/// shapes into its root.
pub fn build_with_shapes(
    image: Image,
    modules: usize,
    processed: impl Fn(usize) -> (ModuleRef, Params),
) -> Result<Stress, Error> {
    build_inner(image, modules, processed, true)
}

fn build_inner(
    image: Image,
    modules: usize,
    processed: impl Fn(usize) -> (ModuleRef, Params),
    shapes: bool,
) -> Result<Stress, Error> {
    assert!(modules > 0, "a stress network has at least one module");
    let size = image.region();
    let mut g = Graph::new();
    let source = g.add_source("image", image);
    let mut input = (source, "image");
    let mut outputs = Vec::with_capacity(modules);
    let mut masks: Vec<NodeId> = Vec::with_capacity(modules);
    for k in 0..modules {
        let mut b = Builder {
            g: &mut g,
            k,
            nodes: 0,
        };
        let operator = |group: usize| {
            let name = OPERATORS[k % OPERATORS.len()][group];
            Params::new().with("operator", Value::Choice(name.into()))
        };

        // Group A: three members from ranges of the input, refined.
        let luminance = b.parametric("luminance", input, &[0.15, 0.7, 0.1, 0.2])?;
        let feather = b.add(
            "feather",
            ModuleRef::new(MaskFeather),
            radius(4.0),
            &[((luminance, "mask"), "mask"), (input, "guide")],
        )?;
        let red = b.parametric("red", input, &[0.2, 0.7, 0.1, 0.1])?;
        let red_blur = b.add(
            "red.blur",
            ModuleRef::new(MaskBlur),
            radius(2.0),
            &[((red, "mask"), "mask")],
        )?;
        let chroma = b.parametric("chroma", input, &[0.0, 0.4, 0.0, 0.1])?;
        let a = b.add(
            "a",
            ModuleRef::new(MaskCombine),
            operator(0).with("opacity", members(&[1.0, 0.8, 0.6])),
            &[
                ((feather, "mask"), "in0"),
                ((red_blur, "mask"), "in1"),
                ((chroma, "mask"), "in2"),
            ],
        )?;
        let a_blur = b.add(
            "a.blur",
            ModuleRef::new(MaskBlur),
            radius(1.5),
            &[((a, "mask"), "mask")],
        )?;
        let a_levels = b.add(
            "a.levels",
            ModuleRef::new(MaskLevels),
            Params::new().with("contrast", Value::Float(0.3)),
            &[((a_blur, "mask"), "mask")],
        )?;

        // Group B: a luminance member, the previous module's mask, an
        // inverted blue range.
        let luma = b.add(
            "luma",
            ModuleRef::new(MaskFromLuma),
            Params::new()
                .with("low", Value::Float(0.1))
                .with("high", Value::Float(0.6)),
            &[(input, "input")],
        )?;
        let luma_blur = b.add(
            "luma.blur",
            ModuleRef::new(MaskBlur),
            radius(3.0),
            &[((luma, "mask"), "mask")],
        )?;
        let reused = match masks.last() {
            Some(&previous) => b.add(
                "raster",
                ModuleRef::new(MaskRaster),
                Params::new().with("opacity", Value::Float(0.7)),
                &[((previous, "mask"), "mask")],
            )?,
            None => b.parametric("green", input, &[0.3, 0.9, 0.2, 0.0])?,
        };
        let blue = b.add(
            "blue",
            ModuleRef::new(MaskParametric),
            Params::new()
                .with("blue", Value::List(vec![0.25, 0.35, 0.05, 0.05]))
                .with("blue_invert", Value::Bool(true)),
            &[(input, "input")],
        )?;
        let group_b = b.add(
            "b",
            ModuleRef::new(MaskCombine),
            operator(1).with("invert", Value::Ints(vec![0, 1, 0])),
            &[
                ((luma_blur, "mask"), "in0"),
                ((reused, "mask"), "in1"),
                ((blue, "mask"), "in2"),
            ],
        )?;
        let b_levels = b.add(
            "b.levels",
            ModuleRef::new(MaskLevels),
            Params::new()
                .with("brightness", Value::Float(0.1))
                .with("opacity", Value::Float(0.9)),
            &[((group_b, "mask"), "mask")],
        )?;

        // The root: both groups and a lightness member (and the shapes).
        let lightness = b.parametric("lightness", input, &[0.0, 0.5, 0.0, 0.2])?;
        let mut members = vec![
            ((a_levels, "mask"), "in0"),
            ((b_levels, "mask"), "in1"),
            ((lightness, "mask"), "in2"),
        ];
        if shapes {
            let group = b.shapes(size, SHAPE_OPERATORS[k % SHAPE_OPERATORS.len()])?;
            members.push(((group, "mask"), "in3"));
        }
        let root = b.add("root", ModuleRef::new(MaskCombine), operator(2), &members)?;
        let mask = b.add(
            "root.levels",
            ModuleRef::new(MaskLevels),
            Params::new().with("contrast", Value::Float(0.2)),
            &[((root, "mask"), "mask")],
        )?;

        // The module itself, blended through its mask.
        let (module, params) = processed(k);
        let name = format!("m{k}.module");
        let module = b.g.add_module(&name, module, &params)?;
        b.g.connect(input.0, input.1, module, "input")?;
        let blend = b.add(
            "blend",
            ModuleRef::new(MaskBlend),
            Params::new(),
            &[
                (input, "input"),
                ((module, "output"), "processed"),
                ((mask, "mask"), "mask"),
            ],
        )?;
        debug_assert_eq!(
            b.nodes,
            MASK_NODES_PER_MODULE + if shapes { SHAPE_NODES_PER_MODULE } else { 0 }
        );
        outputs.push(blend);
        masks.push(mask);
        input = (blend, "output");
    }
    Ok(Stress {
        graph: g,
        source,
        outputs,
        masks,
        mask_nodes: modules
            * (MASK_NODES_PER_MODULE + if shapes { SHAPE_NODES_PER_MODULE } else { 0 }),
    })
}

/// Adds one module's mask nodes, named after it, counting them.
struct Builder<'a> {
    g: &'a mut Graph,
    k: usize,
    nodes: usize,
}

impl Builder<'_> {
    /// Adds a node and connects each `((source, port), input)`.
    fn add(
        &mut self,
        part: &str,
        module: ModuleRef,
        params: Params,
        inputs: &[((NodeId, &str), &str)],
    ) -> Result<NodeId, Error> {
        let id = self
            .g
            .add_module(&format!("m{}.{part}", self.k), module, &params)?;
        for ((from, port), to) in inputs {
            self.g.connect(*from, port, id, to)?;
        }
        self.nodes += 1;
        Ok(id)
    }

    /// A parametric member over one channel's range.
    fn parametric(
        &mut self,
        channel: &str,
        input: (NodeId, &str),
        range: &[f64],
    ) -> Result<NodeId, Error> {
        self.add(
            channel,
            ModuleRef::new(MaskParametric),
            Params::new().with(channel, Value::List(range.to_vec())),
            &[(input, "input")],
        )
    }
}

fn radius(r: f64) -> Params {
    Params::new().with("radius", Value::Float(r))
}

impl Builder<'_> {
    /// Four shapes over an image of `size`, placed apart by module, and
    /// their group.
    fn shapes(&mut self, size: helios_core::Region, operator: &str) -> Result<NodeId, Error> {
        let (w, h) = (f64::from(size.width), f64::from(size.height));
        let shift = 0.1 * self.k as f64;
        let f = |v: f64| Value::Float(v);
        let circle = self.add(
            "circle",
            ModuleRef::new(MaskCircle),
            Params::new()
                .with("x", f(w * (0.25 + shift)))
                .with("y", f(h * 0.3))
                .with("radius", f(h * 0.12))
                .with("feather", f(h * 0.05)),
            &[],
        )?;
        let ellipse = self.add(
            "ellipse",
            ModuleRef::new(MaskEllipse),
            Params::new()
                .with("x", f(w * 0.6))
                .with("y", f(h * (0.6 - shift)))
                .with("radius_x", f(w * 0.15))
                .with("radius_y", f(h * 0.08))
                .with("rotation", f(25.0 + 30.0 * self.k as f64))
                .with("feather", f(h * 0.04)),
            &[],
        )?;
        let gradient = self.add(
            "gradient",
            ModuleRef::new(MaskGradient),
            Params::new()
                .with("x", f(w * 0.5))
                .with("y", f(h * 0.5))
                .with("rotation", f(60.0 * self.k as f64))
                .with("feather", f(h * 0.3))
                .with("profile", Value::Choice("sigmoid".into()))
                .with("invert", Value::Bool(true))
                .with("opacity", f(0.7)),
            &[],
        )?;
        let corner = |x: f64, y: f64| PathPoint::corner(w * x, h * y);
        let path = self.add(
            "path",
            ModuleRef::new(MaskPath),
            Params::new()
                .with(
                    "points",
                    path_values(&[
                        corner(0.1, 0.7),
                        PathPoint {
                            before: [w * 0.25, h * 0.55],
                            after: [w * 0.4, h * 0.85],
                            ..corner(0.3, 0.65)
                        },
                        corner(0.35, 0.95),
                        corner(0.15, 0.9),
                    ]),
                )
                .with("feather", f(h * 0.03)),
            &[],
        )?;
        self.add(
            "s",
            ModuleRef::new(MaskCombine),
            Params::new().with("operator", Value::Choice(operator.into())),
            &[
                ((circle, "mask"), "in0"),
                ((ellipse, "mask"), "in1"),
                ((gradient, "mask"), "in2"),
                ((path, "mask"), "in3"),
            ],
        )
    }
}

/// Per-member values for a group, the rest left at 1.
fn members(values: &[f64]) -> Value {
    let mut all = vec![1.0; MAX_MEMBERS];
    all[..values.len()].copy_from_slice(values);
    Value::List(all)
}

/// A deterministic picture with gradients and edges across every range the
/// network selects, for the stress network to read.
pub fn picture(region: helios_core::Region) -> Image {
    let (w, h) = (region.width.max(1) as f32, region.height.max(1) as f32);
    Image::from_fn(region, 4, |x, y, c| {
        let (u, v) = (x as f32 / w, y as f32 / h);
        let stripe = if (x / 7 + y / 5) % 2 == 0 { 0.15 } else { 0.0 };
        match c {
            0 => u + stripe,
            1 => v,
            2 => 0.5 * (u * 9.0).sin().abs() + 0.2 * v,
            _ => 1.0,
        }
    })
}
