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
//! - **pictures** (JPEG, PNG, TIFF): decoded and converted to the linear
//!   Rec.2020 working space through what their tags say of their colours
//!   (an ICC profile, cICP, PNG chunks, Exif; sRGB when there is nothing,
//!   and [`Colour`] says so), four channels (alpha last, 1 when the file
//!   has none);
//! - **embedded previews** of raw files ([`preview`]), for culling without
//!   decoding the mosaic, converted to the working space like a picture;
//! - **how a raw was shot** ([`Shot`]): its capture time, exposure and
//!   frame number from its EXIF, read with its preview ([`look`]) for an
//!   import's near-duplicate signals.

pub mod capture;
mod colour;

pub use colour::{Colour, ColourTag};

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use helios_colour::{InputSpace, OutputSpace};
use helios_core::{Image, Region};
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
    /// For raw files: where the image sits on the sensor.
    pub sensor: Option<Sensor>,
    /// For pictures: what their colours were read as.
    pub colour: Option<Colour>,
    /// The EXIF orientation tag, 1 to 8 (1 when the file has none): how
    /// the image is turned to be seen. The pixels are as stored: the
    /// `flip` step turns them (a raw's tag is also its camera's).
    pub orientation: u8,
}

/// Where a raw's image sits on its sensor: the full photosite grid (what
/// darktable's pipe starts from, and what its drawn masks are normalised
/// to) and the corner of the crop the loader keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Sensor {
    pub width: u32,
    pub height: u32,
    pub crop_x: u32,
    pub crop_y: u32,
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
    let (image, camera, sensor, colour, orientation) = match Kind::of(path) {
        Kind::Picture => {
            let (image, colour, orientation) = picture(&bytes).map_err(named)?;
            (image, None, None, Some(colour), orientation)
        }
        Kind::Raw => {
            let (image, camera, sensor) = raw(bytes).map_err(named)?;
            let orientation = camera.orientation;
            (image, Some(camera), Some(sensor), None, orientation)
        }
    };
    Ok(Loaded {
        hash,
        image,
        camera,
        sensor,
        colour,
        orientation,
    })
}

/// The preview a raw file carries (the largest one), in the working space,
/// as the file stores it; `None` when it has none. A picture is its own
/// preview: use [`load`].
pub fn preview(path: &Path) -> Result<Option<Image>, String> {
    preview_oriented(path).map(|(image, _)| image)
}

/// [`preview`] and the raw's EXIF orientation tag (1 to 8; 1 when it has
/// none). A raw's embedded previews are stored as its sensor is, not
/// turned: as darktable does, they are seen turned by the raw's tag
/// ([`upright`], or a `flip` step at its default given the tag).
///
/// # Errors
/// The file cannot be read, or its format is unknown.
pub fn preview_oriented(path: &Path) -> Result<(Option<Image>, u8), String> {
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
    let tag = raw_tag(decoder.as_ref(), &source);
    Ok((found.map(|img| working(&img)), tag))
}

/// A raw's EXIF orientation tag, 1 to 8 (1 when it has none), from its
/// metadata (no pixel decoded).
fn raw_tag(decoder: &dyn rawler::decoders::Decoder, source: &RawSource) -> u8 {
    decoder
        .raw_metadata(source, &RawDecodeParams::default())
        .ok()
        .and_then(|m| m.exif.orientation)
        .and_then(|o| u8::try_from(o).ok())
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1)
}

/// A raw's EXIF orientation tag from its bytes, 1 to 8 (1 when it has
/// none, or the bytes are no raw rawler reads): what turns its embedded
/// previews.
pub fn raw_orientation(bytes: Arc<Vec<u8>>) -> u8 {
    let source = RawSource::new_from_shared_vec(bytes);
    rawler::get_decoder(&source).map_or(1, |d| raw_tag(d.as_ref(), &source))
}

/// What [`glance`] found: a small picture and when it was taken.
pub struct Glance {
    /// The smallest preview the file carries (a raw's thumbnail or
    /// preview, a JPEG's Exif thumbnail), else the picture itself, in the
    /// working space; `None` when there is none.
    pub preview: Option<Image>,
    /// When it was taken, as the camera wrote it (`YYYY:MM:DD HH:MM:SS`).
    pub taken: Option<String>,
    /// The camera's model, as it wrote it.
    pub camera: Option<String>,
}

/// A file at a glance, for a grid of what a card holds: its smallest
/// embedded preview and its capture time, never a raw's mosaic decoded.
/// A picture without an Exif thumbnail is decoded whole.
///
/// # Errors
/// The file cannot be read, or a raw's format is unknown.
pub fn glance(path: &Path) -> Result<Glance, String> {
    glance_at(path, None)
}

/// [`glance`], its preview no larger than `max` pixels on its long side:
/// an embedded preview is scaled down in 8 bits before it is taken to the
/// working space, so a thumbnail never converts a full-size preview in
/// floating point; a picture without an Exif thumbnail likewise, once
/// decoded.
///
/// # Errors
/// The file cannot be read, or a raw's format is unknown.
pub fn glance_at(path: &Path, max: Option<u32>) -> Result<Glance, String> {
    let fit = |img: DynamicImage| match max {
        Some(m) if img.width().max(img.height()) > m => img.thumbnail(m, m),
        _ => img,
    };
    // Not hashed: a glance identifies nothing.
    let bytes =
        Arc::new(std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?);
    let named = |e: String| format!("{}: {e}", path.display());
    if Kind::of(path) == Kind::Raw {
        let source = RawSource::new_from_shared_vec(bytes);
        let decoder = rawler::get_decoder(&source).map_err(|e| named(e.to_string()))?;
        let params = RawDecodeParams::default();
        let preview = [
            decoder.thumbnail_image(&source, &params),
            decoder.preview_image(&source, &params),
        ]
        .into_iter()
        .find_map(|r| r.ok().flatten())
        .map(|img| working(&fit(img)));
        let metadata = decoder.raw_metadata(&source, &params).ok();
        let tag = metadata
            .as_ref()
            .and_then(|m| m.exif.orientation)
            .and_then(|o| u8::try_from(o).ok())
            .unwrap_or(1);
        let preview = preview.map(|p| upright(p, tag));
        let taken = metadata.as_ref().and_then(|m| {
            m.exif
                .date_time_original
                .clone()
                .or_else(|| m.exif.create_date.clone())
        });
        let camera = metadata
            .map(|m| m.model.trim().to_string())
            .filter(|m| !m.is_empty());
        return Ok(Glance {
            preview,
            taken,
            camera,
        });
    }
    let reader = image::ImageReader::new(Cursor::new(&bytes[..]))
        .with_guessed_format()
        .map_err(|e| named(e.to_string()))?;
    let tag = picture_tag(&bytes, reader.format());
    let (taken, camera, thumb) = if reader.format() == Some(image::ImageFormat::Jpeg) {
        use image::ImageDecoder as _;
        image::codecs::jpeg::JpegDecoder::new(Cursor::new(&bytes[..]))
            .ok()
            .and_then(|mut d| d.exif_metadata().ok().flatten())
            .map_or((None, None, None), |block| colour::exif_glance(&block))
    } else {
        (None, None, None)
    };
    let preview = match thumb.and_then(|t| image::load_from_memory(&t).ok()) {
        Some(img) => working(&fit(img)),
        None => picture_at(&bytes, max).map_err(named)?.0,
    };
    Ok(Glance {
        preview: Some(upright(preview, tag)),
        taken,
        camera,
    })
}

/// How a raw file was shot, as its EXIF says.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Shot {
    pub maker: String,
    pub model: String,
    /// When it was taken, as the camera wrote it (`YYYY:MM:DD HH:MM:SS`,
    /// the camera's local time).
    pub taken: Option<String>,
    /// The fraction of a second after `taken`, as digits (`25` is 0.25 s).
    pub subsec: Option<String>,
    /// Seconds.
    pub exposure_time: Option<f64>,
    /// f-number.
    pub aperture: Option<f64>,
    pub iso: Option<f64>,
    /// EV.
    pub exposure_bias: Option<f64>,
    /// The camera's frame counter, when it writes one.
    pub frame: Option<u32>,
    pub serial: Option<String>,
}

/// What [`look`] found in a file.
pub struct Look {
    /// The embedded preview of a raw file, a picture itself: in the
    /// working space; `None` for a raw without a preview.
    pub preview: Option<Image>,
    /// How a raw was shot; `None` for a picture.
    pub shot: Option<Shot>,
}

/// A file's preview and how it was shot, from one read: what an import
/// measures without decoding a mosaic.
pub fn look(path: &Path) -> Result<Look, String> {
    if Kind::of(path) == Kind::Picture {
        return Ok(Look {
            preview: Some(load(path)?.image),
            shot: None,
        });
    }
    let (_, bytes) = read(path)?;
    let named = |e: String| format!("{}: {e}", path.display());
    let source = RawSource::new_from_shared_vec(bytes);
    let decoder = rawler::get_decoder(&source).map_err(|e| named(e.to_string()))?;
    let params = RawDecodeParams::default();
    let preview = [
        decoder.preview_image(&source, &params),
        decoder.thumbnail_image(&source, &params),
        decoder.full_image(&source, &params),
    ]
    .into_iter()
    .find_map(|r| r.ok().flatten())
    .map(|img| working(&img));
    let shot = decoder.raw_metadata(&source, &params).ok().map(|m| {
        let exif = &m.exif;
        let unsigned = |r: Option<rawler::formats::tiff::Rational>| {
            r.filter(|r| r.d != 0)
                .map(|r| ratio(f64::from(r.n), f64::from(r.d)))
        };
        Shot {
            maker: m.make.trim().to_string(),
            model: m.model.trim().to_string(),
            taken: exif
                .date_time_original
                .clone()
                .or_else(|| exif.create_date.clone()),
            subsec: exif.sub_sec_time_original.clone(),
            exposure_time: unsigned(exif.exposure_time),
            aperture: unsigned(exif.fnumber),
            iso: exif
                .iso_speed
                .or(exif.recommended_exposure_index)
                .map(f64::from)
                .or(exif.iso_speed_ratings.map(f64::from)),
            exposure_bias: exif
                .exposure_bias
                .filter(|r| r.d != 0)
                .map(|r| ratio(f64::from(r.n), f64::from(r.d))),
            frame: exif.image_number,
            serial: exif.serial_number.clone(),
        }
    });
    Ok(Look { preview, shot })
}

/// The depth of an image's samples when they are integers.
fn integer_bits(img: &DynamicImage) -> Option<u32> {
    let color = img.color();
    match color.bytes_per_pixel() / color.channel_count() {
        1 => Some(8),
        2 => Some(16),
        _ => None,
    }
}

/// An image in `space` to the working space, RGBA.
fn convert(img: &DynamicImage, space: &InputSpace) -> Result<Image, String> {
    let rgba = img.to_rgba32f();
    let (w, h) = rgba.dimensions();
    let mut data = rgba.into_raw();
    space.to_working(&mut data, integer_bits(img))?;
    Ok(Image::new(Region::sized(w, h), 4, data))
}

/// An image taken as sRGB (an embedded preview) to the working space.
fn working(img: &DynamicImage) -> Image {
    convert(img, &InputSpace::srgb()).expect("the sRGB conversion cannot fail")
}

/// A picture in the working space, its colours and its EXIF orientation
/// tag (1 when it has none).
fn picture(bytes: &[u8]) -> Result<(Image, Colour, u8), String> {
    picture_at(bytes, None)
}

/// [`picture`], scaled down in 8 bits to `max` pixels on its long side
/// before its colours are converted.
fn picture_at(bytes: &[u8], max: Option<u32>) -> Result<(Image, Colour, u8), String> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let tags = colour::tags(bytes, format, &mut decoder);
    let orientation = tags.orientation();
    let img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    let img = match max {
        Some(m) if img.width().max(img.height()) > m => img.thumbnail(m, m),
        _ => img,
    };
    let (space, mut colour) = colour::choose(&tags, !img.color().has_color());
    match convert(&img, &space) {
        Ok(image) => Ok((image, colour, orientation)),
        Err(e) => {
            // The pixels are as they were: read them as sRGB instead.
            colour.ignored.push(e);
            colour.from = ColourTag::Untagged;
            colour.space = format!("{} (assumed)", OutputSpace::Srgb.name());
            Ok((working(&img), colour, orientation))
        }
    }
}

/// A picture's EXIF orientation tag, 1 to 8 (1 when it has none), read
/// from its bytes without decoding a pixel.
fn picture_tag(bytes: &[u8], format: Option<image::ImageFormat>) -> u8 {
    colour::exif_of(bytes, format)
        .and_then(|b| colour::exif_orientation(&b))
        .unwrap_or(1)
}

/// The EXIF orientation tag of a picture's bytes (JPEG, PNG, TIFF), 1
/// to 8; 1 when it has none or the bytes are no picture.
pub fn picture_orientation(bytes: &[u8]) -> u8 {
    let format = image::guess_format(bytes).ok();
    picture_tag(bytes, format)
}

/// An image turned as its EXIF orientation `tag` says it is seen, as
/// darktable's `flip` turns it at its default: for what is shown
/// without going through a network (a glance, a thumbnail).
pub fn upright(image: Image, tag: u8) -> Image {
    let Some(o) = image::metadata::Orientation::from_exif(tag)
        .filter(|o| *o != image::metadata::Orientation::NoTransforms)
    else {
        return image;
    };
    let region = image.region();
    let channels = image.channels();
    if channels != 4 {
        return image;
    }
    let Some(buffer) =
        image::Rgba32FImage::from_raw(region.width, region.height, image.data().to_vec())
    else {
        return image;
    };
    let mut turned = DynamicImage::ImageRgba32F(buffer);
    turned.apply_orientation(o);
    let (w, h) = (turned.width(), turned.height());
    Image::new(Region::sized(w, h), 4, turned.into_rgba32f().into_raw())
}

/// A picture's size as stored (before its orientation turns it) and its
/// EXIF orientation tag (1 to 8, 1 when it has none), read without
/// decoding its pixels: what places darktable's drawn masks on it.
/// `None` for a raw (see [`sensor`]).
///
/// # Errors
/// The file cannot be read, or is no picture.
pub fn picture_geometry(path: &Path) -> Result<Option<(u32, u32, u8)>, String> {
    if Kind::of(path) == Kind::Raw {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let named = |e: String| format!("{}: {e}", path.display());
    let reader = image::ImageReader::new(Cursor::new(&bytes[..]))
        .with_guessed_format()
        .map_err(|e| named(e.to_string()))?;
    let tag = picture_tag(&bytes, reader.format());
    let (width, height) = reader.into_dimensions().map_err(|e| named(e.to_string()))?;
    Ok(Some((width, height, tag)))
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
        // rawler leaves `RawImage::orientation` unread; the EXIF tag has it.
        orientation: exif
            .and_then(|e| e.orientation)
            .and_then(|o| u8::try_from(o).ok())
            .filter(|o| (1..=8).contains(o))
            .unwrap_or(1),
    }
}

/// The area of the sensor the loader keeps: `(x, y, width, height)`.
fn crop(raw: &RawImage) -> Result<(usize, usize, usize, usize), String> {
    let (x0, y0, w, h) = raw
        .crop_area
        .or(raw.active_area)
        .map_or((0, 0, raw.width, raw.height), |r| {
            (r.p.x, r.p.y, r.d.w, r.d.h)
        });
    if x0 + w > raw.width || y0 + h > raw.height || w == 0 || h == 0 {
        return Err(format!(
            "crop {w}x{h}+{x0}+{y0} outside the {}x{} sensor",
            raw.width, raw.height
        ));
    }
    Ok((x0, y0, w, h))
}

/// Where a raw's image sits on its sensor ([`Sensor`], as [`load`] gives
/// it) and its EXIF orientation tag (1 to 8, 1 when it has none), read
/// without decoding the mosaic: what places darktable's drawn masks.
/// `None` for a picture.
///
/// # Errors
/// The file cannot be read, or a raw's format is unknown.
pub fn sensor(path: &Path) -> Result<Option<(Sensor, u8)>, String> {
    if Kind::of(path) == Kind::Picture {
        return Ok(None);
    }
    let bytes =
        Arc::new(std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?);
    let named = |e: String| format!("{}: {e}", path.display());
    let source = RawSource::new_from_shared_vec(bytes);
    let decoder = rawler::get_decoder(&source).map_err(|e| named(e.to_string()))?;
    let params = RawDecodeParams::default();
    // A dummy image: its geometry without its photosites.
    let raw = decoder
        .raw_image(&source, &params, true)
        .map_err(|e| named(e.to_string()))?;
    let (x0, y0, _, _) = crop(&raw).map_err(named)?;
    let orientation = raw_tag(decoder.as_ref(), &source);
    Ok(Some((
        Sensor {
            width: raw.width as u32,
            height: raw.height as u32,
            crop_x: x0 as u32,
            crop_y: y0 as u32,
        },
        orientation,
    )))
}

fn raw(bytes: Arc<Vec<u8>>) -> Result<(Image, CameraInfo, Sensor), String> {
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
    let (x0, y0, w, h) = crop(&raw)?;
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
    let sensor = Sensor {
        width: raw.width as u32,
        height: raw.height as u32,
        crop_x: x0 as u32,
        crop_y: y0 as u32,
    };
    Ok((image, camera, sensor))
}
