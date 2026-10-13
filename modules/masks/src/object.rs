//! An AI object member: a mask of an object (the subject, the sky, a
//! person, or what a prompt names) that a model finds, recorded as a
//! decision. darktable's AI object masks, through Helios's decision nodes
//! (see `docs/module-contract.md`, "Decision nodes", and `docs/masks.md`,
//! "AI object members").
//!
//! [`MaskObject`] is a module of the decision kind. Its **question** is
//! what to find (`kind`, `prompt` when the kind is `prompt`) and who to ask
//! (`resolver`, a name the application maps to a resolver; empty for its
//! default): set like any parameter, and a new question makes it pending
//! again. Its **answer** is the mask a resolver gave: an 8-bit grayscale
//! PNG at the resolution the model saw (at most [`MAX_SIDE`] a side),
//! base64 encoded in `mask`, the region of the image it covers in `area`
//! (full-resolution pixels) and the model's `confidence`. The answer lives
//! in the decision book with who gave it, so an edit replays it offline.
//!
//! The node upsamples the mask to the render: each output pixel takes the
//! mask bilinearly at its centre in image coordinates, so every scale,
//! crop and tile sees the same mask, and outside `area` it is 0. Pending
//! (never answered, or asked another question), it renders nothing: a
//! render through it waits like through any decision.

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamHint, ParamKind, ParamSpec,
    Params, PortSpec, Region, Role, Value,
};

use crate::mask_port;

/// The longest side of a mask answer, in mask pixels.
pub const MAX_SIDE: u32 = 1024;
/// The most bytes the encoded mask holds (base64 of a PNG of at most
/// [`MAX_SIDE`] × [`MAX_SIDE`] 8-bit pixels, with room for its framing).
pub const MASK_CAPACITY: u32 = 1 << 21;
/// What a member can be asked to find.
pub const KINDS: [&str; 4] = ["subject", "sky", "person", "prompt"];
/// The parameters that are the question.
pub const QUESTION: [&str; 3] = ["kind", "prompt", "resolver"];

/// The AI object member (see the module's documentation).
pub struct MaskObject;

/// A mask answer, decoded: one value from 0 to 1 per mask pixel, row by
/// row, over `area` of the image.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectPlan {
    pub width: usize,
    pub height: usize,
    pub values: Vec<f32>,
    /// x, y, width, height in full-resolution image pixels.
    pub area: [f64; 4],
    pub scale: f64,
}

impl Module for MaskObject {
    type Plan = ObjectPlan;

    fn manifest(&self) -> Manifest {
        let text = |name: &str, description: &str, capacity: u32| ParamSpec {
            name: name.into(),
            description: description.into(),
            kind: ParamKind::Text {
                capacity,
                default: String::new(),
            },
        };
        Manifest {
            name: "mask_object".into(),
            version: 1,
            description: "A member a model finds: a mask of the subject, the sky, a person or what a prompt names, recorded as a decision".into(),
            params: vec![
                ParamSpec {
                    name: "kind".into(),
                    description: "what to find: the main subject, the sky, a person, or what the prompt names".into(),
                    kind: ParamKind::Choice {
                        options: KINDS.iter().map(|k| (*k).into()).collect(),
                        default: "subject".into(),
                    },
                },
                text("prompt", "what to find, in words, when the kind is prompt", 200),
                text("resolver", "who finds it: a resolver's name, empty for the default one", 64),
                text("mask", "the mask found: an 8-bit grayscale PNG, base64", MASK_CAPACITY),
                ParamSpec {
                    name: "area".into(),
                    description: "the region of the image the mask covers: x, y, width, height in image pixels".into(),
                    kind: ParamKind::Ints {
                        count: 4,
                        min: 0,
                        max: i64::from(i32::MAX),
                        default: Vec::new(),
                    },
                },
                ParamSpec::float("confidence", "how sure the model is, from 0 to 1", 0.0, 1.0, 0.0),
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![mask_port("mask")],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Decision,
            family: Family::Mask,
            role: Role::General,
        }
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![ParamHint::new("confidence").unit("%").display(100.0, 0)]
    }

    fn question(&self) -> Vec<String> {
        QUESTION.iter().map(|q| (*q).into()).collect()
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<ObjectPlan, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, p: &Params, _: &Context, scale: f64) -> Result<ObjectPlan, String> {
        let area = match p.get("area") {
            Some(Value::Ints(a)) if a.len() == 4 => {
                [a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64]
            }
            _ => [0.0; 4],
        };
        let text = match p.get("mask") {
            Some(Value::Text(t)) => t.as_str(),
            _ => "",
        };
        let (width, height, values) = if text.is_empty() || area[2] <= 0.0 || area[3] <= 0.0 {
            (0, 0, Vec::new())
        } else {
            let (w, h, bytes) = decode_mask(text)?;
            (
                w as usize,
                h as usize,
                bytes.iter().map(|&b| f32::from(b) / 255.0).collect(),
            )
        };
        Ok(ObjectPlan {
            width,
            height,
            values,
            area,
            scale,
        })
    }

    fn process(
        &self,
        plan: &ObjectPlan,
        _: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 1, |x, y, _| plan.at(x, y))])
    }
}

impl ObjectPlan {
    /// The mask at render pixel `(x, y)`: bilinear at its centre.
    pub fn at(&self, x: i64, y: i64) -> f32 {
        if self.width == 0 || self.height == 0 {
            return 0.0;
        }
        let [ax, ay, aw, ah] = self.area;
        let ix = (x as f64 + 0.5) / self.scale - ax;
        let iy = (y as f64 + 0.5) / self.scale - ay;
        if ix < 0.0 || iy < 0.0 || ix >= aw || iy >= ah {
            return 0.0;
        }
        let (w, h) = (self.width, self.height);
        let u = (ix * w as f64 / aw - 0.5).clamp(0.0, (w - 1) as f64);
        let v = (iy * h as f64 / ah - 0.5).clamp(0.0, (h - 1) as f64);
        let (u0, v0) = (u.floor() as usize, v.floor() as usize);
        let (u1, v1) = ((u0 + 1).min(w - 1), (v0 + 1).min(h - 1));
        let (fu, fv) = ((u - u0 as f64) as f32, (v - v0 as f64) as f32);
        let m = |i: usize, j: usize| self.values[j * w + i];
        let top = m(u0, v0) * (1.0 - fu) + m(u1, v0) * fu;
        let bottom = m(u0, v1) * (1.0 - fu) + m(u1, v1) * fu;
        top * (1.0 - fv) + bottom * fv
    }
}

/// A mask as the answer holds it: `values` (one byte a pixel, row by row)
/// as an 8-bit grayscale PNG, base64 encoded.
pub fn encode_mask(width: u32, height: u32, values: &[u8]) -> Result<String, String> {
    check_size(width, height)?;
    if values.len() != width as usize * height as usize {
        return Err(format!(
            "a {width}×{height} mask has {} values",
            values.len()
        ));
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(values).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(STANDARD.encode(png))
}

/// A mask from its encoded answer: its size and its values.
pub fn decode_mask(text: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let png = STANDARD
        .decode(text.trim())
        .map_err(|e| format!("the mask is not base64: {e}"))?;
    decode_png(&png)
}

/// A PNG as a mask: its luminance, 8 bits a pixel, alpha left out. Any
/// colour type and depth is read; at most [`MAX_SIDE`] a side.
pub fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let bad = |e: png::DecodingError| format!("the mask is not a PNG: {e}");
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(bad)?;
    let (width, height) = (reader.info().width, reader.info().height);
    check_size(width, height)?;
    let size = reader
        .output_buffer_size()
        .ok_or("the mask is too large to decode")?;
    let mut buf = vec![0; size];
    let frame = reader.next_frame(&mut buf).map_err(bad)?;
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("the mask's palette was not expanded".into()),
    };
    let line = frame.line_size;
    let mut out = Vec::with_capacity(width as usize * height as usize);
    for row in buf.chunks(line).take(height as usize) {
        for px in row.chunks(channels).take(width as usize) {
            out.push(if channels >= 3 {
                // Rec.709 luminance of the encoded values.
                (0.2126 * f32::from(px[0]) + 0.7152 * f32::from(px[1]) + 0.0722 * f32::from(px[2]))
                    .round()
                    .clamp(0.0, 255.0) as u8
            } else {
                px[0]
            });
        }
    }
    Ok((width, height, out))
}

fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!(
            "a mask is 1 to {MAX_SIDE} pixels a side, not {width}×{height}"
        ));
    }
    Ok(())
}

/// The answer's parameters for a mask over `area` (x, y, width, height in
/// image pixels) with the model's confidence.
pub fn answer(
    width: u32,
    height: u32,
    values: &[u8],
    area: Region,
    confidence: f64,
) -> Result<Params, String> {
    Ok(Params::new()
        .with("mask", Value::Text(encode_mask(width, height, values)?))
        .with(
            "area",
            Value::Ints(vec![
                area.x,
                area.y,
                i64::from(area.width),
                i64::from(area.height),
            ]),
        )
        .with("confidence", Value::Float(confidence.clamp(0.0, 1.0))))
}
