//! Parity with darktable: a module ported from darktable is checked against
//! darktable's own render (`docs/module-contract.md`, "How a module is
//! tested", level 3).
//!
//! The comparison isolates one module. Both sides get the same linear
//! Rec.2020 float image. darktable reads it as a PFM whose history holds
//! only that module between colorin and colorout, both linear Rec.2020, so
//! its pipe is the module and two conversions that cancel. The results
//! are compared in CIE Lab with Delta E 2000, against darktable's own
//! integration thresholds.
//!
//! darktable is found as `HELIOS_DARKTABLE_CLI`, else `darktable-cli` on
//! the PATH; [`Darktable::find`] says when there is none, so tests can
//! skip and say why.

use std::path::{Path, PathBuf};
use std::process::Command;

use helios_core::{Image, Region};
use helios_photo::color::{REC2020_TO_XYZ, apply};
use helios_xmp::{Layouts, encode};
use serde_json::Value;

/// darktable's thresholds for its integration tests.
pub const MAX_DELTA_E: f64 = 2.3;
pub const MEAN_DELTA_E: f64 = 0.77;

/// A history item: a darktable module and the values of its params that
/// differ from darktable's defaults.
pub struct Item {
    pub module: String,
    pub values: Value,
}

impl Item {
    pub fn new(module: &str, values: Value) -> Item {
        Item {
            module: module.into(),
            values,
        }
    }
}

/// darktable's colorspace number for linear Rec.2020 (`DT_COLORSPACE_LIN_REC2020`).
const LIN_REC2020: i64 = 4;
/// darktable's colorspace number for a camera's standard matrix.
const STANDARD_MATRIX: i64 = 11;

/// What darktable's colorin reads the input as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Already linear Rec.2020 (a float image): colorin changes nothing.
    LinearRec2020,
    /// A raw file: the camera's standard matrix, as Helios's colorin uses.
    Camera,
}

/// A sidecar whose history is `items` between colorin (reading `input`,
/// into linear Rec.2020) and colorout (linear Rec.2020), at the newest
/// versions `layouts` knows, with darktable's automatic presets marked as
/// applied so it adds nothing. Modules darktable enables by default and
/// `items` leave out (rawprepare, white balance) keep their defaults.
pub fn sidecar(items: &[Item], input: Input, layouts: &Layouts) -> Result<String, String> {
    let profile = match input {
        Input::LinearRec2020 => LIN_REC2020,
        Input::Camera => STANDARD_MATRIX,
    };
    let mut history = vec![Item::new(
        "colorin",
        serde_json::json!({"type": profile, "type_work": LIN_REC2020}),
    )];
    for item in items {
        history.push(Item::new(&item.module, item.values.clone()));
    }
    history.push(Item::new(
        "colorout",
        serde_json::json!({"type": LIN_REC2020}),
    ));
    let mut lines = Vec::new();
    for (i, item) in history.iter().enumerate() {
        let version = *layouts
            .versions(&item.module)
            .iter()
            .max()
            .ok_or_else(|| format!("no layout for `{}`", item.module))?;
        let layout = layouts.get(&item.module, version).expect("listed");
        let params: String = encode(layout, &item.values)?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        lines.push(format!(
            r#"<rdf:li darktable:num="{i}" darktable:operation="{}" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{params}" darktable:multi_name="" darktable:multi_priority="0"/>"#,
            item.module
        ));
    }
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:raw_params="0"
   darktable:auto_presets_applied="1" darktable:history_end="{}">
   <darktable:history>
    <rdf:Seq>
     {}
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#,
        history.len(),
        lines.join("\n     ")
    ))
}

/// Writes the RGB channels of an image as a little-endian PFM (rows bottom up).
pub fn write_pfm(image: &Image, path: &Path) -> Result<(), String> {
    let r = image.region();
    let mut out = format!("PF\n{} {}\n-1.0\n", r.width, r.height).into_bytes();
    for y in (0..r.height as i64).rev() {
        for x in 0..r.width as i64 {
            let p = image.pixel(r.x + x, r.y + y);
            for c in 0..3 {
                out.extend_from_slice(&p[c.min(p.len() - 1)].to_le_bytes());
            }
        }
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads a colour PFM as a 3-channel image at the origin.
pub fn read_pfm(path: &Path) -> Result<Image, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut header = data.splitn(4, |b| *b == b'\n');
    let (Some(magic), Some(size), Some(scale), Some(body)) =
        (header.next(), header.next(), header.next(), header.next())
    else {
        return Err("PFM: short header".into());
    };
    if magic != b"PF" {
        return Err("PFM: not a colour PFM".into());
    }
    let size = String::from_utf8_lossy(size);
    let mut wh = size.split_whitespace().map(str::parse::<u32>);
    let (Some(Ok(w)), Some(Ok(h))) = (wh.next(), wh.next()) else {
        return Err(format!("PFM: bad size `{size}`"));
    };
    let little = String::from_utf8_lossy(scale)
        .trim()
        .parse::<f32>()
        .map_err(|e| format!("PFM scale: {e}"))?
        < 0.0;
    let n = (w * h * 3) as usize;
    if body.len() < n * 4 {
        return Err("PFM: short data".into());
    }
    let value = |i: usize| {
        let b: [u8; 4] = body[i * 4..i * 4 + 4].try_into().expect("4 bytes");
        if little {
            f32::from_le_bytes(b)
        } else {
            f32::from_be_bytes(b)
        }
    };
    Ok(Image::from_fn(Region::sized(w, h), 3, |x, y, c| {
        let row = h as usize - 1 - y as usize;
        value((row * w as usize + x as usize) * 3 + c as usize)
    }))
}

/// A darktable command line found on this machine.
pub struct Darktable {
    pub cli: PathBuf,
    pub version: String,
}

impl Darktable {
    /// darktable-cli, or why there is none.
    pub fn find() -> Result<Darktable, String> {
        let cli = std::env::var_os("HELIOS_DARKTABLE_CLI")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("darktable-cli"));
        let out = Command::new(&cli)
            .arg("--version")
            .output()
            .map_err(|e| format!("no darktable-cli ({}): {e}", cli.display()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let version = text.lines().next().unwrap_or("").trim().to_string();
        Ok(Darktable { cli, version })
    }

    /// The params layouts of this darktable's modules: a release's own
    /// (`layouts/`, from `tools/dt_param_layouts.sh` run on its plugins),
    /// else those Helios was built against, which may be newer.
    pub fn layouts(&self) -> Layouts {
        let release = self
            .version
            .strip_prefix("darktable ")
            .and_then(|v| v.rsplit_once('.'))
            .map(|(major_minor, _)| major_minor);
        let json = match release {
            Some("5.6") => Some(include_str!("../layouts/darktable-5.6.json")),
            _ => None,
        };
        json.and_then(|j| Layouts::from_json(j).ok())
            .unwrap_or_else(Layouts::built_in)
    }

    /// Renders a linear Rec.2020 image with a history of `items` (see
    /// [`sidecar`]) in a throwaway configuration under `dir`, and reads
    /// the result back.
    pub fn render(
        &self,
        input: &Image,
        items: &[Item],
        layouts: &Layouts,
        dir: &Path,
    ) -> Result<Image, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let src = dir.join("input.pfm");
        write_pfm(input, &src)?;
        self.run(&src, items, Input::LinearRec2020, layouts, dir)
    }

    /// Renders a raw file with a history of `items` after darktable's
    /// default raw modules, read through the camera's standard matrix.
    pub fn render_raw(
        &self,
        raw: &Path,
        items: &[Item],
        layouts: &Layouts,
        dir: &Path,
    ) -> Result<Image, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = raw.file_name().ok_or("a raw path without a name")?;
        let src = dir.join(name);
        std::fs::copy(raw, &src).map_err(|e| format!("{}: {e}", raw.display()))?;
        self.run(&src, items, Input::Camera, layouts, dir)
    }

    fn run(
        &self,
        src: &Path,
        items: &[Item],
        input: Input,
        layouts: &Layouts,
        dir: &Path,
    ) -> Result<Image, String> {
        let mut xmp = src.as_os_str().to_owned();
        xmp.push(".xmp");
        let xmp = PathBuf::from(xmp);
        let out = dir.join("output.pfm");
        let _ = std::fs::remove_file(&out);
        std::fs::write(&xmp, sidecar(items, input, layouts)?)
            .map_err(|e| format!("{}: {e}", xmp.display()))?;
        let run = Command::new(&self.cli)
            .arg(src)
            .arg(&xmp)
            .arg(&out)
            .args(["--apply-custom-presets", "false", "--core", "--configdir"])
            .arg(dir.join("config"))
            .args(["--library", ":memory:"])
            .args(["--conf", "plugins/darkroom/workflow=none"])
            // White balance as shot in the temperature module, as Helios's.
            .args(["--conf", "plugins/darkroom/chromatic-adaptation=legacy"])
            .output()
            .map_err(|e| format!("darktable-cli: {e}"))?;
        if !run.status.success() || !out.exists() {
            return Err(format!(
                "darktable-cli failed ({}): {}",
                run.status,
                String::from_utf8_lossy(&run.stderr)
            ));
        }
        read_pfm(&out)
    }
}

/// Linear Rec.2020 to CIE Lab (D65 white).
pub fn lab(rgb: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = apply(&REC2020_TO_XYZ, rgb);
    let white = apply(&REC2020_TO_XYZ, [1.0, 1.0, 1.0]);
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x / white[0]), f(y / white[1]), f(z / white[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE Delta E 2000 between two Lab colours.
pub fn delta_e_2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let [l1, a1, b1] = a;
    let [l2, a2, b2] = b;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).rem_euclid(2.0 * PI)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > PI {
            d - 2.0 * PI
        } else if d < -PI {
            d + 2.0 * PI
        } else {
            d
        }
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).sin();
    let lm = (l1 + l2) / 2.0;
    let cmp = (c1p + c2p) / 2.0;
    let hm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= PI {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 2.0 * PI {
        (h1p + h2p + 2.0 * PI) / 2.0
    } else {
        (h1p + h2p - 2.0 * PI) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - PI / 6.0).cos()
        + 0.24 * (2.0 * hm).cos()
        + 0.32 * (3.0 * hm + PI / 30.0).cos()
        - 0.20 * (4.0 * hm - 63.0 * PI / 180.0).cos();
    let dtheta = PI / 6.0 * (-((hm * 180.0 / PI - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cmp.powi(7) / (cmp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -(2.0 * dtheta).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

/// How far two renders are, in Delta E 2000 over every pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Difference {
    pub max: f64,
    pub mean: f64,
    /// The 99.9th percentile: the max of all but one pixel in a thousand.
    pub p999: f64,
    /// The largest difference of a channel value, for the record.
    pub max_abs: f64,
}

impl Difference {
    /// Within darktable's integration thresholds.
    pub fn passes(&self) -> bool {
        self.max <= MAX_DELTA_E && self.mean <= MEAN_DELTA_E
    }

    /// Within darktable's thresholds, the max taken at the 99.9th
    /// percentile. For a port rather than darktable against itself: in
    /// near-black noise a rounding difference flips a demosaic's choice of
    /// direction, and Lab turns a few thousandths into a large Delta E
    /// (on mire1.cr2, 0.005 % of pixels, all below 0.02).
    pub fn passes_as_a_port(&self) -> bool {
        self.p999 <= MAX_DELTA_E && self.mean <= MEAN_DELTA_E
    }
}

/// Compares the RGB channels of two images of the same size, wherever
/// they sit, as linear Rec.2020.
pub fn compare(ours: &Image, theirs: &Image) -> Result<Difference, String> {
    let (a, b) = (ours.region(), theirs.region());
    if (a.width, a.height) != (b.width, b.height) {
        return Err(format!("sizes differ: {a} against {b}"));
    }
    let (mut max, mut sum, mut max_abs) = (0.0f64, 0.0f64, 0.0f64);
    let mut all = Vec::with_capacity(a.area());
    for y in 0..a.height as i64 {
        for x in 0..a.width as i64 {
            let p = ours.pixel(a.x + x, a.y + y);
            let q = theirs.pixel(b.x + x, b.y + y);
            let rgb = |p: &[f32]| [p[0] as f64, p[1] as f64, p[2] as f64];
            for c in 0..3 {
                max_abs = max_abs.max((p[c] - q[c]).abs() as f64);
            }
            let d = delta_e_2000(lab(rgb(p)), lab(rgb(q)));
            max = max.max(d);
            sum += d;
            all.push(d);
        }
    }
    let rank = ((all.len() as f64 * 0.999) as usize).min(all.len().saturating_sub(1));
    let p999 = if all.is_empty() {
        0.0
    } else {
        *all.select_nth_unstable_by(rank, f64::total_cmp).1
    };
    Ok(Difference {
        max,
        mean: sum / a.area() as f64,
        p999,
        max_abs,
    })
}

/// The mean of each `factor` × `factor` block: what is left of an image
/// once detail is gone, to tell colour differences from demosaicing ones.
pub fn reduce(image: &Image, factor: u32) -> Image {
    let r = image.region();
    let (w, h) = (r.width / factor, r.height / factor);
    let n = (factor * factor) as f32;
    Image::from_fn(Region::sized(w, h), 3, |x, y, c| {
        let mut sum = 0.0;
        for dy in 0..factor as i64 {
            for dx in 0..factor as i64 {
                sum += image.pixel(r.x + x * factor as i64 + dx, r.y + y * factor as i64 + dy)
                    [c as usize];
            }
        }
        sum / n
    })
}

/// Where `theirs` sits on `ours` when the two were cropped differently
/// (raw decoders disagree on a sensor's usable area): the shift, within
/// `reach` pixels, that best matches their luminance, and the overlapping
/// parts at that shift.
pub fn align(ours: &Image, theirs: &Image, reach: i64) -> (i64, i64, Image, Image) {
    let (a, b) = (ours.region(), theirs.region());
    let luma = |p: &[f32]| p[0] * 0.26 + p[1] * 0.68 + p[2] * 0.06;
    let overlap = |dx: i64, dy: i64| {
        // theirs(x, y) is ours(x + dx, y + dy).
        let x0 = 0.max(-dx);
        let y0 = 0.max(-dy);
        let x1 = (b.width as i64).min(a.width as i64 - dx);
        let y1 = (b.height as i64).min(a.height as i64 - dy);
        (x0, y0, x1, y1)
    };
    let mut best = (f64::INFINITY, 0, 0);
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (x0, y0, x1, y1) = overlap(dx, dy);
            if x1 - x0 < 16 || y1 - y0 < 16 {
                continue;
            }
            // A sparse grid is enough to find the shift.
            let (mut sum, mut count) = (0.0f64, 0u32);
            for y in (y0..y1).step_by(((y1 - y0) / 64).max(1) as usize) {
                for x in (x0..x1).step_by(((x1 - x0) / 64).max(1) as usize) {
                    let p = luma(ours.pixel(a.x + x + dx, a.y + y + dy));
                    let q = luma(theirs.pixel(b.x + x, b.y + y));
                    sum += f64::from((p - q).abs());
                    count += 1;
                }
            }
            let score = sum / f64::from(count);
            if score < best.0 {
                best = (score, dx, dy);
            }
        }
    }
    let (_, dx, dy) = best;
    let (x0, y0, x1, y1) = overlap(dx, dy);
    let size = Region::sized((x1 - x0) as u32, (y1 - y0) as u32);
    let cut = |img: &Image, ox: i64, oy: i64| {
        let r = img.region();
        Image::from_fn(size, 3, |x, y, c| {
            img.pixel(r.x + x + ox, r.y + y + oy)[c as usize]
        })
    };
    (dx, dy, cut(ours, x0 + dx, y0 + dy), cut(theirs, x0, y0))
}

/// An image without its outer `margin` pixels. Near the image's edges
/// darktable's demosaic methods leave pixels to another method, which
/// Helios does not reproduce: parity is judged inside.
pub fn inset(image: &Image, margin: u32) -> Image {
    let r = image.region();
    let inner = Region::new(
        r.x + i64::from(margin),
        r.y + i64::from(margin),
        r.width.saturating_sub(2 * margin),
        r.height.saturating_sub(2 * margin),
    );
    image.sample(inner)
}

/// How far from the image's edges demosaic parity is judged.
pub const DEMOSAIC_MARGIN: u32 = 12;
