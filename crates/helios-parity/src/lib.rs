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

/// A sidecar whose history is `items` between colorin and colorout, both
/// linear Rec.2020, at the newest versions `layouts` knows, with darktable's
/// automatic presets marked as applied so it adds nothing.
pub fn sidecar(items: &[Item], layouts: &Layouts) -> Result<String, String> {
    let mut history = vec![Item::new(
        "colorin",
        serde_json::json!({"type": LIN_REC2020, "type_work": LIN_REC2020}),
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

    /// Renders `input` with a history of `items` (see [`sidecar`]) in a
    /// throwaway configuration under `dir`, and reads the result back.
    pub fn render(
        &self,
        input: &Image,
        items: &[Item],
        layouts: &Layouts,
        dir: &Path,
    ) -> Result<Image, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let src = dir.join("input.pfm");
        let xmp = dir.join("input.pfm.xmp");
        let out = dir.join("output.pfm");
        let _ = std::fs::remove_file(&out);
        write_pfm(input, &src)?;
        std::fs::write(&xmp, sidecar(items, layouts)?)
            .map_err(|e| format!("{}: {e}", xmp.display()))?;
        let run = Command::new(&self.cli)
            .arg(&src)
            .arg(&xmp)
            .arg(&out)
            .args(["--apply-custom-presets", "false", "--core", "--configdir"])
            .arg(dir.join("config"))
            .args(["--library", ":memory:"])
            .args(["--conf", "plugins/darkroom/workflow=none"])
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
    /// The largest difference of a channel value, for the record.
    pub max_abs: f64,
}

impl Difference {
    /// Within darktable's integration thresholds.
    pub fn passes(&self) -> bool {
        self.max <= MAX_DELTA_E && self.mean <= MEAN_DELTA_E
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
        }
    }
    Ok(Difference {
        max,
        mean: sum / a.area() as f64,
        max_abs,
    })
}
