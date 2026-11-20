//! The fast first read: the smallest embedded JPEG that is large enough,
//! decoded with DCT scaling straight to 8-bit sRGB (no floating point, no
//! full-size decode), then scaled to the size asked.

use std::path::Path;
use std::sync::Arc;

use helios_load::Kind;
use image::{DynamicImage, RgbImage, imageops::FilterType};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use crate::Rgb8;

pub use helios_load::embedded::{Embedded, choose, embedded_jpegs};

/// Whether a file holds a grey JPEG stream beside a colour one: what
/// [`choose`] took first before it put colour streams first.
pub fn has_grey_beside_colour(bytes: &[u8]) -> bool {
    let found = embedded_jpegs(bytes);
    found.iter().any(Embedded::grey) && found.iter().any(|e| !e.grey())
}

/// Decodes a JPEG stream at the smallest DCT scale that keeps its long
/// side at least `long`, then scales it to `long` (never up).
pub fn decode_jpeg(jpeg: &[u8], long: u32) -> Result<Rgb8, String> {
    helios_load::embedded::decode_scaled(jpeg, long).map(|image| fit(image, long))
}

/// An image scaled to `long` on its long side (never up), as 8-bit RGB.
pub fn fit(image: RgbImage, long: u32) -> Rgb8 {
    let (w, h) = image.dimensions();
    let image = if w.max(h) > long {
        let scale = f64::from(long) / f64::from(w.max(h));
        let nw = ((f64::from(w) * scale).round() as u32).max(1);
        let nh = ((f64::from(h) * scale).round() as u32).max(1);
        image::imageops::resize(&image, nw, nh, FilterType::Triangle)
    } else {
        image
    };
    Rgb8 {
        width: image.width(),
        height: image.height(),
        pixels: image.into_raw(),
    }
}

/// A file's thumbnail, `long` pixels on its long side: its smallest
/// embedded JPEG at least that large (a raw's preview, a JPEG's Exif
/// thumbnail or the JPEG itself), else what rawler finds (a preview that
/// is not a JPEG stream), else the picture itself (PNG, TIFF). `None`
/// when the file has nothing readable as a picture.
///
/// Embedded previews are sRGB: no colour conversion is made.
///
/// # Errors
/// The file cannot be read.
pub fn read(path: &Path, long: u32) -> Result<Option<Rgb8>, String> {
    if Kind::of(path) == Kind::Raw {
        // A raw's sensor data is left out: its previews and tags are read.
        let bytes = helios_load::preview_bytes(path)?;
        let tag = helios_load::raw_orientation(bytes.clone());
        return Ok(from_bytes_tagged(bytes, Kind::Raw, long, tag));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(from_bytes(bytes, Kind::of(path), long))
}

/// What rawler is asked for, smallest first.
enum RawGet {
    Thumbnail,
    Preview,
    Full,
}

/// [`read`] from a file's bytes. The thumbnail is turned as the file's
/// EXIF orientation says it is seen: a JPEG's Exif thumbnail is stored as
/// the picture is, and a raw's embedded previews as its sensor is, both
/// unturned (darktable turns a raw's embedded preview by the raw's
/// orientation too). A thumbnail rendered from an edit is not made here:
/// its `flip` step has turned it already.
pub fn from_bytes(bytes: Vec<u8>, kind: Kind, long: u32) -> Option<Rgb8> {
    let bytes = Arc::new(bytes);
    let tag = match kind {
        Kind::Picture => helios_load::picture_orientation(&bytes),
        Kind::Raw => helios_load::raw_orientation(bytes.clone()),
    };
    from_bytes_tagged(bytes, kind, long, tag)
}

/// [`from_bytes`] with the file's EXIF orientation `tag` (1 to 8) given:
/// the stored preview turned by it.
pub fn from_bytes_tagged(bytes: Arc<Vec<u8>>, kind: Kind, long: u32, tag: u8) -> Option<Rgb8> {
    stored(bytes, kind, long).map(|rgb| upright(rgb, tag))
}

/// An 8-bit thumbnail turned by an EXIF orientation tag.
fn upright(rgb: Rgb8, tag: u8) -> Rgb8 {
    let Some(o) = image::metadata::Orientation::from_exif(tag)
        .filter(|o| *o != image::metadata::Orientation::NoTransforms)
    else {
        return rgb;
    };
    let Some(image) = RgbImage::from_raw(rgb.width, rgb.height, rgb.pixels.clone()) else {
        return rgb;
    };
    let mut turned = DynamicImage::ImageRgb8(image);
    turned.apply_orientation(o);
    let turned = turned.into_rgb8();
    Rgb8 {
        width: turned.width(),
        height: turned.height(),
        pixels: turned.into_raw(),
    }
}

/// [`from_bytes`], the pixels as the file stores them.
fn stored(bytes: Arc<Vec<u8>>, kind: Kind, long: u32) -> Option<Rgb8> {
    for e in choose(&embedded_jpegs(&bytes), long) {
        if let Ok(rgb) = decode_jpeg(&bytes[e.offset..], long) {
            return Some(rgb);
        }
    }
    let image = match kind {
        Kind::Raw => {
            let source = RawSource::new_from_shared_vec(bytes);
            let decoder = rawler::get_decoder(&source).ok()?;
            let params = RawDecodeParams::default();
            let mut found: Vec<DynamicImage> = Vec::new();
            for get in [RawGet::Thumbnail, RawGet::Preview, RawGet::Full] {
                let img = match get {
                    RawGet::Thumbnail => decoder.thumbnail_image(&source, &params),
                    RawGet::Preview => decoder.preview_image(&source, &params),
                    RawGet::Full => decoder.full_image(&source, &params),
                };
                if let Ok(Some(img)) = img {
                    let large = img.width().max(img.height()) >= long;
                    found.push(img);
                    if large {
                        break;
                    }
                }
            }
            found.pop()?
        }
        Kind::Picture => image::load_from_memory(&bytes).ok()?,
    };
    Some(fit(image.to_rgb8(), long))
}
