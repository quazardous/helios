//! Loaders: files read by the framework into graph sources.
//!
//! Data access is the framework's job, never a module's (see
//! `docs/architecture.md`, "Data access"). A file is read once, hashed
//! (BLAKE3, the vault's image identity), and decoded:
//!
//! - **raw files**, through rawler: the sensor mosaic as one channel in raw
//!   units (no black subtraction, no scaling: that is the job of a module,
//!   as darktable's rawprepare), cropped to the recommended area, with its
//!   filter pattern shifted to the crop and the camera description
//!   ([`CameraInfo`]) for the context of the modules that declare it;
//! - **pictures** (JPEG, PNG, TIFF): decoded, their sRGB curve removed and
//!   converted to the linear Rec.2020 working space, four channels (alpha
//!   last, 1 when the file has none);
//! - **embedded previews** of raw files ([`preview`]), for culling without
//!   decoding the mosaic, converted to the working space like a picture.

use std::path::Path;
use std::sync::Arc;

use helios_core::{Image, Region};
use helios_photo::color::{self, Mat3};
use helios_photo::{CameraInfo, Cfa};
use image::DynamicImage;
use rawler::decoders::{RawDecodeParams, RawMetadata};
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::{RawImage, RawImageData, RawPhotometricInterpretation};
use rawler::rawsource::RawSource;
use serde::Serialize;

/// What a file holds, once loaded.
pub struct Loaded {
    /// BLAKE3 of the file, hex: the vault's identity of the image.
    pub hash: String,
    pub image: Image,
    /// For raw files: the camera and its sensor, as context.
    pub camera: Option<CameraInfo>,
}

/// What kind of file a path is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Picture,
    Raw,
}

impl Kind {
    pub fn of(path: &Path) -> Kind {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("jpg" | "jpeg" | "png" | "tif" | "tiff") => Kind::Picture,
            _ => Kind::Raw,
        }
    }
}

fn read(path: &Path) -> Result<(String, Arc<Vec<u8>>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok((blake3::hash(&bytes).to_hex().to_string(), Arc::new(bytes)))
}

/// Loads a file: a raw mosaic with its camera, or a picture in the working space.
pub fn load(path: &Path) -> Result<Loaded, String> {
    let (hash, bytes) = read(path)?;
    let named = |e: String| format!("{}: {e}", path.display());
    let (image, camera) = match Kind::of(path) {
        Kind::Picture => (picture(&bytes).map_err(named)?, None),
        Kind::Raw => {
            let (image, camera) = raw(bytes).map_err(named)?;
            (image, Some(camera))
        }
    };
    Ok(Loaded {
        hash,
        image,
        camera,
    })
}

/// The preview a raw file carries (the largest one), in the working space;
/// `None` when it has none. A picture is its own preview: use [`load`].
pub fn preview(path: &Path) -> Result<Option<Image>, String> {
    let (_, bytes) = read(path)?;
    let source = RawSource::new_from_shared_vec(bytes);
    let decoder = rawler::get_decoder(&source).map_err(|e| format!("{}: {e}", path.display()))?;
    let params = RawDecodeParams::default();
    let found = [
        decoder.full_image(&source, &params),
        decoder.preview_image(&source, &params),
        decoder.thumbnail_image(&source, &params),
    ]
    .into_iter()
    .find_map(|r| r.ok().flatten());
    Ok(found.map(|img| working(&img)))
}

/// sRGB-encoded to the linear working space.
fn srgb_to_working() -> Mat3 {
    let xyz_to_rec2020 = color::inverse(&color::REC2020_TO_XYZ).expect("invertible primaries");
    color::mul(&xyz_to_rec2020, &color::SRGB_TO_XYZ)
}

fn working(img: &DynamicImage) -> Image {
    let rgba = img.to_rgba32f();
    let (w, h) = rgba.dimensions();
    let m = srgb_to_working();
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for p in rgba.pixels() {
        let linear = [0, 1, 2].map(|c| color::srgb_decode(f64::from(p.0[c])));
        for v in color::apply(&m, linear) {
            data.push(v as f32);
        }
        data.push(p.0[3]);
    }
    Image::new(Region::sized(w, h), 4, data)
}

fn picture(bytes: &[u8]) -> Result<Image, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    Ok(working(&img))
}

fn ratio(n: f64, d: f64) -> f64 {
    if d == 0.0 { 0.0 } else { n / d }
}

/// The black level of a photosite, in full-sensor coordinates.
fn black_at(raw: &RawImage, x: usize, y: usize) -> f64 {
    let b = &raw.blacklevel;
    if b.levels.is_empty() || b.width == 0 || b.height == 0 {
        return 0.0;
    }
    let i = ((y % b.height) * b.width + x % b.width) * b.cpp.max(1);
    let r = b.levels[i.min(b.levels.len() - 1)];
    ratio(f64::from(r.n), f64::from(r.d))
}

/// The black level measured on the sensor's masked photosites, when the
/// camera has some: the median of each of the four 2 x 2 positions, by
/// full-sensor parity (row, column), as rawspeed and so darktable measure
/// it. rawler takes their mean, which a few hot masked photosites pull up
/// or down.
fn masked_black(raw: &RawImage) -> Option<[f64; 4]> {
    let RawImageData::Integer(data) = &raw.data else {
        return None;
    };
    if raw.blackareas.is_empty() || raw.cpp != 1 {
        return None;
    }
    let mut samples: [Vec<u16>; 4] = Default::default();
    for area in &raw.blackareas {
        for y in area.p.y..(area.p.y + area.d.h).min(raw.height) {
            for x in area.p.x..(area.p.x + area.d.w).min(raw.width) {
                samples[(y & 1) * 2 + (x & 1)].push(data[y * raw.width + x]);
            }
        }
    }
    if samples.iter().any(Vec::is_empty) {
        return None;
    }
    Some(samples.map(|mut s| {
        s.sort_unstable();
        f64::from(s[s.len() / 2])
    }))
}

fn camera(
    raw: &RawImage,
    meta: Option<&RawMetadata>,
    cfa: Cfa,
    (x0, y0): (usize, usize),
) -> CameraInfo {
    let exif = meta.map(|m| &m.exif);
    let rational = |r: Option<rawler::formats::tiff::Rational>| {
        r.map_or(0.0, |r| ratio(f64::from(r.n), f64::from(r.d)))
    };
    let iso = exif.map_or(0.0, |e| {
        e.iso_speed
            .or(e.recommended_exposure_index)
            .map(f64::from)
            .or(e.iso_speed_ratings.map(f64::from))
            .unwrap_or(0.0)
    });
    // The D65 matrix when the camera has one (as darktable uses), else the
    // first known; its first three rows are R, G, B.
    let matrix = raw
        .color_matrix
        .get(&Illuminant::D65)
        .or_else(|| raw.color_matrix.values().next());
    let masked = masked_black(raw);
    let mut xyz_to_camera = [0.0; 9];
    if let Some(m) = matrix.filter(|m| m.len() >= 9) {
        for (i, v) in m.iter().take(9).enumerate() {
            xyz_to_camera[i] = f64::from(*v);
        }
    }
    // The fourth coefficient is the second green: absent (NaN) on most
    // cameras, then the same as the first green.
    let wb = raw.wb_coeffs.map(f64::from);
    let white_balance = [
        wb[0],
        wb[1],
        wb[2],
        if wb[3].is_finite() { wb[3] } else { wb[1] },
    ];
    CameraInfo {
        // rawler passes the EXIF strings through for a camera it does not
        // know (a DNG from an unlisted body), padding included: trimmed, as
        // darktable and rawspeed do, so names compare.
        maker: raw.clean_make.trim().to_string(),
        model: raw.clean_model.trim().to_string(),
        iso,
        exposure_bias: exif
            .and_then(|e| e.exposure_bias)
            .map_or(0.0, |r| ratio(f64::from(r.n), f64::from(r.d))),
        exposure_time: rational(exif.and_then(|e| e.exposure_time)),
        aperture: rational(exif.and_then(|e| e.fnumber)),
        focal_length: rational(exif.and_then(|e| e.focal_length)),
        // The 2 x 2 photosites at the crop's corner, row by row.
        black: [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
            let (x, y) = (x0 + dx, y0 + dy);
            match masked {
                Some(m) => m[(y & 1) * 2 + (x & 1)],
                None => black_at(raw, x, y),
            }
        }),
        white: f64::from(
            raw.whitelevel
                .0
                .first()
                .copied()
                .unwrap_or(u32::from(u16::MAX)),
        ),
        white_balance,
        xyz_to_camera,
        cfa,
    }
}

fn raw(bytes: Arc<Vec<u8>>) -> Result<(Image, CameraInfo), String> {
    let source = RawSource::new_from_shared_vec(bytes);
    let decoder = rawler::get_decoder(&source).map_err(|e| e.to_string())?;
    let params = RawDecodeParams::default();
    let raw = decoder
        .raw_image(&source, &params, false)
        .map_err(|e| e.to_string())?;
    let meta = decoder.raw_metadata(&source, &params).ok();
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        return Err("not a mosaic (linear or monochrome raws are not supported yet)".into());
    };
    if raw.cpp != 1 {
        return Err(format!("{} components per photosite, expected 1", raw.cpp));
    }
    let area = raw
        .crop_area
        .or(raw.active_area)
        .map_or((0, 0, raw.width, raw.height), |r| {
            (r.p.x, r.p.y, r.d.w, r.d.h)
        });
    let (x0, y0, w, h) = area;
    if x0 + w > raw.width || y0 + h > raw.height || w == 0 || h == 0 {
        return Err(format!(
            "crop {w}x{h}+{x0}+{y0} outside the {}x{} sensor",
            raw.width, raw.height
        ));
    }
    // The pattern as seen from the crop's corner.
    if !config.cfa.is_rgb() {
        return Err(format!(
            "{} sensor ({}): only red, green and blue filters are supported",
            config.cfa.name,
            if config.cfa.is_cygm() {
                "CYGM"
            } else {
                "not RGB"
            }
        ));
    }
    let cfa = Cfa::parse(&config.cfa.shift(x0, y0).name)?;
    let value = |i: usize| -> f32 {
        match &raw.data {
            RawImageData::Integer(v) => f32::from(v[i]),
            RawImageData::Float(v) => v[i],
        }
    };
    let mut data = Vec::with_capacity(w * h);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            data.push(value(y * raw.width + x));
        }
    }
    let image = Image::new(Region::sized(w as u32, h as u32), 1, data);
    let camera = camera(&raw, meta.as_ref(), cfa, (x0, y0));
    Ok((image, camera))
}
