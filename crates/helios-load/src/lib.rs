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
pub mod embedded;
pub mod sparse;

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

/// A raw file's bytes for its previews and metadata: its sensor data
/// left out when its layout allows ([`sparse::read`]), else the whole
/// file. Not for decoding the mosaic, nor for hashing.
///
/// # Errors
/// The file cannot be read.
pub fn preview_bytes(path: &Path) -> Result<Arc<Vec<u8>>, String> {
    let cannot = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(cannot)?;
    sparse::read(&mut file).map(Arc::new).map_err(cannot)
}

/// Loads a file: a raw mosaic with its camera, or a picture in the working space.
pub fn load(path: &Path) -> Result<Loaded, String> {
    let bytes = {
        let _span = helios_core::trace::span("decode.read", || path.display().to_string());
        Arc::new(std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?)
    };
    let named = |e: String| format!("{}: {e}", path.display());
    // The file's hash beside the decode, on a thread of its own: the
    // decode never waits on it.
    let (hash, decoded) = std::thread::scope(|scope| {
        let hashing = {
            let bytes = bytes.clone();
            scope.spawn(move || {
                let _span = helios_core::trace::span("decode.file_hash", String::new);
                blake3::hash(&bytes).to_hex().to_string()
            })
        };
        let decoded = match Kind::of(path) {
            Kind::Picture => picture(&bytes)
                .map(|(image, colour, orientation)| (image, None, None, Some(colour), orientation)),
            Kind::Raw => raw(bytes.clone()).map(|(image, camera, sensor)| {
                let orientation = camera.orientation;
                (image, Some(camera), Some(sensor), None, orientation)
            }),
        };
        let hash = hashing
            .join()
            .unwrap_or_else(|_| blake3::hash(&bytes).to_hex().to_string());
        (hash, decoded)
    });
    let (image, camera, sensor, colour, orientation) = decoded.map_err(named)?;
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
    preview_oriented_at(path, None)
}

/// [`preview_oriented`] for a first paint `max` pixels long: the preview
/// at least `max` pixels on its long side (when it is that large), and
/// less than twice that, made in 8 bits before it is taken to the working
/// space, so a first paint never converts a full-size preview in floating
/// point.
///
/// The colour JPEG stream the file embeds that is the smallest at least
/// `max` long (else the largest) is decoded at the smallest DCT scale
/// that keeps it so ([`embedded::decode_scaled`]): a Nikon Z6 II's
/// 6048 px preview asked 2560 px is decoded at 3024 px, never whole.
/// A raw whose previews are no JPEG stream falls back to rawler, which
/// decodes its largest preview alone (the smaller ones only when it is
/// missing or unreadable), then the result is scaled by a whole-factor
/// block mean.
///
/// # Errors
/// The file cannot be read, or its format is unknown.
pub fn preview_oriented_at(path: &Path, max: Option<u32>) -> Result<(Option<Image>, u8), String> {
    let bytes = preview_bytes(path)?;
    let source = RawSource::new_from_shared_vec(bytes.clone());
    let decoder = rawler::get_decoder(&source).map_err(|e| format!("{}: {e}", path.display()))?;
    let params = RawDecodeParams::default();
    let scaled = max.and_then(|m| {
        let _span = helios_core::trace::span("preview.decode", || {
            format!("{} scaled to {m}", path.display())
        });
        embedded::choose(&embedded::embedded_jpegs(&bytes), m)
            .into_iter()
            .filter(|e| !e.grey())
            .find_map(|e| embedded::decode_scaled(&bytes[e.offset..], m).ok())
            .map(DynamicImage::ImageRgb8)
    });
    let found = scaled.or_else(|| {
        let _span = helios_core::trace::span("preview.decode", || path.display().to_string());
        decoder
            .full_image(&source, &params)
            .ok()
            .flatten()
            .or_else(|| decoder.preview_image(&source, &params).ok().flatten())
            .or_else(|| decoder.thumbnail_image(&source, &params).ok().flatten())
    });
    let metadata = decoder.raw_metadata(&source, &params).ok();
    let tag = metadata
        .as_ref()
        .and_then(|m| m.exif.orientation)
        .and_then(|o| u8::try_from(o).ok())
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1);
    let space = raw_preview_space(&bytes, metadata.as_ref());
    let fit = |img: DynamicImage| {
        let _span = helios_core::trace::span("preview.shrink", String::new);
        match max {
            Some(m) if img.width().max(img.height()) >= m.saturating_mul(2) => shrink(img, m),
            _ => img,
        }
    };
    Ok((
        found.map(|img| {
            let img = fit(img);
            let _span = helios_core::trace::span("preview.working", || {
                format!("{}x{}", img.width(), img.height())
            });
            preview_working(&img, space)
        }),
        tag,
    ))
}

/// `img` made smaller for a first paint at least `min` pixels long: an
/// 8-bit RGB picture (a JPEG preview) by the mean of each k × k block, k
/// the largest whole factor that keeps it at least `min` long, in row
/// bands over the machine's threads; any other by
/// [`DynamicImage::thumbnail`] to `min`. The block mean is several times
/// faster than a resampling filter on a full-size preview.
fn shrink(img: DynamicImage, min: u32) -> DynamicImage {
    let DynamicImage::ImageRgb8(rgb) = img else {
        return img.thumbnail(min, min);
    };
    let (w, h) = rgb.dimensions();
    let k = (w.max(h) / min.max(1)).max(1) as usize;
    let (ow, oh) = ((w as usize / k).max(1), (h as usize / k).max(1));
    if k == 1 {
        return DynamicImage::ImageRgb8(rgb);
    }
    let src = rgb.as_raw();
    let stride = w as usize * 3;
    let mut out = vec![0u8; ow * oh * 3];
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let band = oh.div_ceil(threads).max(1);
    let area = (k * k) as u32;
    std::thread::scope(|scope| {
        for (b, rows) in out.chunks_mut(band * ow * 3).enumerate() {
            scope.spawn(move || {
                let mut sums = vec![0u32; ow * 3];
                for (r, row) in rows.chunks_mut(ow * 3).enumerate() {
                    let oy = b * band + r;
                    sums.fill(0);
                    for sy in oy * k..oy * k + k {
                        let line = &src[sy * stride..sy * stride + ow * k * 3];
                        for (ox, block) in line.chunks_exact(k * 3).enumerate() {
                            for px in block.chunks_exact(3) {
                                sums[ox * 3] += u32::from(px[0]);
                                sums[ox * 3 + 1] += u32::from(px[1]);
                                sums[ox * 3 + 2] += u32::from(px[2]);
                            }
                        }
                    }
                    for (o, s) in row.iter_mut().zip(&sums) {
                        *o = u8::try_from((s + area / 2) / area).unwrap_or(u8::MAX);
                    }
                }
            });
        }
    });
    image::RgbImage::from_raw(ow as u32, oh as u32, out)
        .map_or_else(|| DynamicImage::ImageRgb8(rgb), DynamicImage::ImageRgb8)
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
    let named = |e: String| format!("{}: {e}", path.display());
    // Not hashed: a glance identifies nothing.
    if Kind::of(path) == Kind::Raw {
        let bytes = preview_bytes(path)?;
        let source = RawSource::new_from_shared_vec(bytes.clone());
        let decoder = rawler::get_decoder(&source).map_err(|e| named(e.to_string()))?;
        let params = RawDecodeParams::default();
        let metadata = decoder.raw_metadata(&source, &params).ok();
        let space = raw_preview_space(&bytes, metadata.as_ref());
        let preview = [
            decoder.thumbnail_image(&source, &params),
            decoder.preview_image(&source, &params),
        ]
        .into_iter()
        .find_map(|r| r.ok().flatten())
        .map(|img| preview_working(&fit(img), space));
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
    let bytes =
        Arc::new(std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?);
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
    let bytes = preview_bytes(path)?;
    let named = |e: String| format!("{}: {e}", path.display());
    let source = RawSource::new_from_shared_vec(bytes.clone());
    let decoder = rawler::get_decoder(&source).map_err(|e| named(e.to_string()))?;
    let params = RawDecodeParams::default();
    let metadata = decoder.raw_metadata(&source, &params).ok();
    let space = raw_preview_space(&bytes, metadata.as_ref());
    let preview = [
        decoder.preview_image(&source, &params),
        decoder.thumbnail_image(&source, &params),
        decoder.full_image(&source, &params),
    ]
    .into_iter()
    .find_map(|r| r.ok().flatten())
    .map(|img| preview_working(&img, space));
    let shot = metadata.map(|m| {
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

/// An image in `space` to the working space, RGBA. An 8-bit RGB image (a
/// JPEG preview) goes the u8 path ([`InputSpace::rgb8_to_working`]): the
/// same values, bit for bit, without its float copy.
fn convert(img: &DynamicImage, space: &InputSpace) -> Result<Image, String> {
    if let DynamicImage::ImageRgb8(rgb) = img {
        let data = space.rgb8_to_working(rgb.as_raw())?;
        return Ok(Image::new(
            Region::sized(rgb.width(), rgb.height()),
            4,
            data,
        ));
    }
    let rgba = img.to_rgba32f();
    let (w, h) = rgba.dimensions();
    let mut data = rgba.into_raw();
    space.to_working(&mut data, integer_bits(img))?;
    Ok(Image::new(Region::sized(w, h), 4, data))
}

/// An image taken as sRGB to the working space.
fn working(img: &DynamicImage) -> Image {
    convert(img, &InputSpace::srgb()).expect("the sRGB conversion cannot fail")
}

/// A raw's embedded preview to the working space, read in the space the
/// raw's Exif names ([`colour::preview_space`]): a camera set to Adobe RGB
/// writes its previews in Adobe RGB, untagged.
fn preview_working(img: &DynamicImage, space: OutputSpace) -> Image {
    convert(img, &InputSpace::of(space)).unwrap_or_else(|_| working(img))
}

/// The space of a raw's embedded previews, from its bytes and metadata.
fn raw_preview_space(bytes: &[u8], metadata: Option<&RawMetadata>) -> OutputSpace {
    colour::preview_space(bytes, metadata.and_then(|m| m.exif.color_space))
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

/// A raw's camera, its make and model as [`load`] names them (the
/// decoder's clean names, trimmed), read without decoding the mosaic:
/// what keys a camera's defaults before the photo is opened. `None` for
/// a picture.
///
/// # Errors
/// The file cannot be read, or a raw's format is unknown.
pub fn raw_camera(path: &Path) -> Result<Option<(String, String)>, String> {
    if Kind::of(path) == Kind::Picture {
        return Ok(None);
    }
    let bytes =
        Arc::new(std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?);
    let named = |e: String| format!("{}: {e}", path.display());
    let source = RawSource::new_from_shared_vec(bytes);
    let decoder = rawler::get_decoder(&source).map_err(|e| named(e.to_string()))?;
    // A dummy image: its camera without its photosites.
    let raw = decoder
        .raw_image(&source, &RawDecodeParams::default(), true)
        .map_err(|e| named(e.to_string()))?;
    Ok(Some((
        raw.clean_make.trim().to_string(),
        raw.clean_model.trim().to_string(),
    )))
}

fn raw(bytes: Arc<Vec<u8>>) -> Result<(Image, CameraInfo, Sensor), String> {
    let source = RawSource::new_from_shared_vec(bytes);
    let decoder = rawler::get_decoder(&source).map_err(|e| e.to_string())?;
    let params = RawDecodeParams::default();
    let raw = {
        let _span = helios_core::trace::span("decode.raw", String::new);
        decoder
            .raw_image(&source, &params, false)
            .map_err(|e| e.to_string())?
    };
    let meta = {
        let _span = helios_core::trace::span("decode.metadata", String::new);
        decoder.raw_metadata(&source, &params).ok()
    };
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
    let _span = helios_core::trace::span("decode.mosaic", || format!("{w}x{h}"));
    // Row slice by row slice, the sample type matched once: a loop the
    // compiler vectorises, not a match per photosite.
    let mut data = Vec::with_capacity(w * h);
    let rows = (y0..y0 + h).map(|y| y * raw.width + x0..y * raw.width + x0 + w);
    match &raw.data {
        RawImageData::Integer(v) => {
            for row in rows {
                data.extend(v[row].iter().map(|&s| f32::from(s)));
            }
        }
        RawImageData::Float(v) => {
            for row in rows {
                data.extend_from_slice(&v[row]);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_takes_each_blocks_mean_keeping_the_size_asked() {
        // 7 × 5, at least 3 long: k = 2, 3 × 2 out of the top-left 6 × 4.
        let rgb =
            image::RgbImage::from_fn(7, 5, |x, y| image::Rgb([(x * 10) as u8, (y * 20) as u8, 9]));
        let out = shrink(DynamicImage::ImageRgb8(rgb), 3).to_rgb8();
        assert_eq!(out.dimensions(), (3, 2));
        assert_eq!(out.get_pixel(0, 0).0, [5, 10, 9]);
        assert_eq!(out.get_pixel(1, 0).0, [25, 10, 9]);
        let small = image::RgbImage::new(5, 2);
        assert_eq!(shrink(DynamicImage::ImageRgb8(small), 3).width(), 5);
    }

    fn pixel(image: &Image) -> [f32; 3] {
        let d = image.data();
        [d[0], d[1], d[2]]
    }

    #[test]
    fn an_adobe_rgb_preview_is_not_read_as_srgb() {
        // A camera set to Adobe RGB writes its preview's full green: in
        // the working space it is a deeper green than sRGB's.
        let green =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([0, 255, 0])));
        let adobe = pixel(&preview_working(&green, OutputSpace::AdobeRgb));
        let srgb = pixel(&preview_working(&green, OutputSpace::Srgb));
        assert_eq!(srgb, pixel(&working(&green)));
        assert!(adobe[0] < srgb[0] - 0.05, "{adobe:?} against {srgb:?}");
        // White stays white in both.
        let white =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([255; 3])));
        for c in pixel(&preview_working(&white, OutputSpace::AdobeRgb)) {
            assert!((c - 1.0).abs() < 1e-3, "{c}");
        }
    }
}
