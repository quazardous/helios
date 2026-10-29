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

pub mod masks;
pub mod vkdt;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use helios_core::{Image, Region};
pub use helios_photo::compare::{Difference, Thresholds, delta_e_2000, lab};
use helios_photo::compare::{Pixels, difference};
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
    /// The module's instance (`multi_priority`), 0 for the first.
    pub instance: u32,
}

impl Item {
    pub fn new(module: &str, values: Value) -> Item {
        Item {
            module: module.into(),
            values,
            instance: 0,
        }
    }

    /// The same item for another instance of its module.
    #[must_use]
    pub fn instance(mut self, instance: u32) -> Item {
        self.instance = instance;
        self
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
    let (colorin, colorout) = linear_ends(input);
    let mut history = vec![colorin];
    for item in items {
        history.push(Item::new(&item.module, item.values.clone()).instance(item.instance));
    }
    history.push(colorout);
    let mut lines = Vec::new();
    for (i, item) in history.iter().enumerate() {
        lines.push(history_line(i, item, layouts)?);
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

/// colorin reading `input` into linear Rec.2020, and colorout writing
/// linear Rec.2020: around a history, a pipe whose output is its working
/// space, as Helios's colorin gives it.
pub fn linear_ends(input: Input) -> (Item, Item) {
    let profile = match input {
        Input::LinearRec2020 => LIN_REC2020,
        Input::Camera => STANDARD_MATRIX,
    };
    (
        Item::new(
            "colorin",
            serde_json::json!({"type": profile, "type_work": LIN_REC2020}),
        ),
        Item::new("colorout", serde_json::json!({"type": LIN_REC2020})),
    )
}

/// A history item of a sidecar, number `num`, at the newest version
/// `layouts` knows for its module.
pub fn history_line(num: usize, item: &Item, layouts: &Layouts) -> Result<String, String> {
    let version = *layouts
        .versions(&item.module)
        .iter()
        .max()
        .ok_or_else(|| format!("no layout for `{}`", item.module))?;
    let layout = layouts.get(&item.module, version).expect("listed");
    let params = encode(layout, &item.values)?
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        });
    let name = if item.instance == 0 {
        String::new()
    } else {
        item.instance.to_string()
    };
    Ok(format!(
        r#"<rdf:li darktable:num="{num}" darktable:operation="{}" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{params}" darktable:multi_name="{name}" darktable:multi_priority="{}"/>"#,
        item.module, item.instance
    ))
}

/// Writes the RGB channels of an image as a little-endian PFM (rows bottom up).
pub fn write_pfm(image: &Image, path: &Path) -> Result<(), String> {
    let r = image.region();
    let mut out = format!("PF\n{} {}\n-1.0\n", r.width, r.height).into_bytes();
    for y in (0..i64::from(r.height)).rev() {
        for x in 0..i64::from(r.width) {
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
            .map_or_else(|| PathBuf::from("darktable-cli"), PathBuf::from);
        Darktable::at(&cli)
    }

    /// The darktable-cli at `cli`, or why it does not run.
    pub fn at(cli: &Path) -> Result<Darktable, String> {
        let cli = cli.to_path_buf();
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
        self.render_sidecar(input, &sidecar(items, Input::LinearRec2020, layouts)?, dir)
    }

    /// Renders a linear Rec.2020 image with a sidecar's text, as is.
    pub fn render_sidecar(&self, input: &Image, xmp: &str, dir: &Path) -> Result<Image, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let src = dir.join("input.pfm");
        write_pfm(input, &src)?;
        self.run(&src, xmp, dir)
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
        self.render_raw_sidecar(raw, &sidecar(items, Input::Camera, layouts)?, dir)
    }

    /// The modules darktable runs for a raw with a sidecar's text, as is,
    /// in the order it runs them: the `process` lines of its pipe debug
    /// output for a small export, as `operation` then `.N` for instance N
    /// (`dt_iop_get_instance_id`: `.x` from 7 on).
    pub fn processed(&self, raw: &Path, xmp: &str, dir: &Path) -> Result<Vec<String>, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = raw.file_name().ok_or("a raw path without a name")?;
        let src = dir.join(name);
        std::fs::copy(raw, &src).map_err(|e| format!("{}: {e}", raw.display()))?;
        let mut sidecar = src.as_os_str().to_owned();
        sidecar.push(".xmp");
        std::fs::write(&sidecar, xmp).map_err(|e| format!("{}: {e}", dir.display()))?;
        let out = dir.join("small.jpg");
        let _ = std::fs::remove_file(&out);
        let run = Command::new(&self.cli)
            .arg(&src)
            .arg(&sidecar)
            .arg(&out)
            // Small, but not tiny: darktable 5.6.1 crashes in highlights at 64 px.
            .args(["--width", "512", "--height", "512"])
            .args(["--apply-custom-presets", "false", "--core", "--configdir"])
            .arg(dir.join("config"))
            .args(["--library", ":memory:"])
            .args(["--conf", "plugins/darkroom/workflow=none"])
            .args(["--conf", "plugins/darkroom/chromatic-adaptation=legacy"])
            .args(["-d", "pipe"])
            .output()
            .map_err(|e| format!("darktable-cli: {e}"))?;
        if !run.status.success() {
            return Err(format!(
                "darktable-cli failed ({}): {}",
                run.status,
                String::from_utf8_lossy(&run.stderr)
            ));
        }
        let text = String::from_utf8_lossy(&run.stdout).into_owned()
            + &String::from_utf8_lossy(&run.stderr);
        Ok(text
            .lines()
            .filter_map(|line| {
                let words: Vec<&str> = line.split_whitespace().collect();
                let at = words.iter().position(|w| *w == "[export]")?;
                if !words[..at].contains(&"process") {
                    return None;
                }
                words.get(at + 1).map(|w| (*w).to_string())
            })
            .collect())
    }

    /// Renders a raw file with a sidecar's text, as is.
    pub fn render_raw_sidecar(&self, raw: &Path, xmp: &str, dir: &Path) -> Result<Image, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = raw.file_name().ok_or("a raw path without a name")?;
        let src = dir.join(name);
        std::fs::copy(raw, &src).map_err(|e| format!("{}: {e}", raw.display()))?;
        self.run(&src, xmp, dir)
    }

    fn run(&self, src: &Path, sidecar: &str, dir: &Path) -> Result<Image, String> {
        let mut xmp = src.as_os_str().to_owned();
        xmp.push(".xmp");
        let xmp = PathBuf::from(xmp);
        let out = dir.join("output.pfm");
        let _ = std::fs::remove_file(&out);
        std::fs::write(&xmp, sidecar).map_err(|e| format!("{}: {e}", xmp.display()))?;
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

/// A difference, judged by darktable's integration thresholds: a max of
/// at most [`MAX_DELTA_E`] and a mean of at most [`MEAN_DELTA_E`].
pub const DARKTABLE: Thresholds = Thresholds {
    mean: MEAN_DELTA_E,
    max: MAX_DELTA_E,
    ..Thresholds::NONE
};

/// darktable's thresholds with the max taken at the 99.9th percentile:
/// for a port rather than darktable against itself. In near-black noise a
/// rounding difference flips a demosaic's choice of direction, and Lab
/// turns a few thousandths into a large Delta E (on mire1.cr2, 0.005 % of
/// pixels, all below 0.02).
pub const AS_A_PORT: Thresholds = Thresholds {
    mean: MEAN_DELTA_E,
    p999: MAX_DELTA_E,
    ..Thresholds::NONE
};

/// The `mask-parity` job's thresholds: [`AS_A_PORT`], with the 99.99th
/// percentile and the max capped too, so a fault on a few hundred pixels
/// (an edge drawn off by one) fails even when the rest matches. The caps
/// sit above the worst fixture's values when they were set, with room for
/// noise: see docs/masks.md, "Parity with darktable".
pub const MASK_PARITY: Thresholds = Thresholds {
    mean: MEAN_DELTA_E,
    p999: MAX_DELTA_E,
    p9999: MASK_P9999,
    max: MASK_MAX,
};

/// The `mask-parity` bound on the 99.99th percentile.
pub const MASK_P9999: f64 = 1.5;
/// The `mask-parity` cap on the max: darktable's own, on every pixel.
pub const MASK_MAX: f64 = MAX_DELTA_E;

/// Compares the RGB channels of two images of the same size, wherever
/// they sit, as linear Rec.2020 (see [`helios_photo::compare`]).
pub fn compare(ours: &Image, theirs: &Image) -> Result<Difference, String> {
    let (a, b) = (ours.region(), theirs.region());
    if (a.width, a.height) != (b.width, b.height) {
        return Err(format!("sizes differ: {a} against {b}"));
    }
    difference(&pixels(ours)?, &pixels(theirs)?)
}

/// An image's pixels, as the comparison reads them.
fn pixels(img: &Image) -> Result<Pixels<'_>, String> {
    let r = img.region();
    Pixels::new(
        img.data(),
        img.channels() as usize,
        r.width as usize,
        r.height as usize,
    )
}

/// The mean of each `factor` × `factor` block: what is left of an image
/// once detail is gone, to tell colour differences from demosaicing ones.
pub fn reduce(image: &Image, factor: u32) -> Image {
    let r = image.region();
    let (w, h) = (r.width / factor, r.height / factor);
    let n = (factor * factor) as f32;
    Image::from_fn(Region::sized(w, h), 3, |x, y, c| {
        let mut sum = 0.0;
        for dy in 0..i64::from(factor) {
            for dx in 0..i64::from(factor) {
                sum += image.pixel(
                    r.x + x * i64::from(factor) + dx,
                    r.y + y * i64::from(factor) + dy,
                )[c as usize];
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
        let x1 = i64::from(b.width).min(i64::from(a.width) - dx);
        let y1 = i64::from(b.height).min(i64::from(a.height) - dy);
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
