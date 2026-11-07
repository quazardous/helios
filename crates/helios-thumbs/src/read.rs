//! The fast first read: the smallest embedded JPEG that is large enough,
//! decoded with DCT scaling straight to 8-bit sRGB (no floating point, no
//! full-size decode), then scaled to the size asked.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use helios_load::Kind;
use image::{DynamicImage, RgbImage, imageops::FilterType};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use crate::Rgb8;

/// An embedded JPEG stream: where it starts and its size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Embedded {
    pub offset: usize,
    pub width: u32,
    pub height: u32,
}

impl Embedded {
    fn long(&self) -> u32 {
        self.width.max(self.height)
    }
}

/// The baseline or progressive JPEG streams a file holds (a raw's
/// previews and thumbnail, a JPEG itself and its Exif thumbnail), found by
/// their start marker and checked up to their frame header. Lossless
/// streams (a raw's mosaic) are left out.
pub fn embedded_jpegs(bytes: &[u8]) -> Vec<Embedded> {
    let mut found = Vec::new();
    for offset in memchr::memmem::find_iter(bytes, b"\xFF\xD8\xFF") {
        if let Some((width, height)) = frame_size(&bytes[offset..])
            && width > 0
            && height > 0
        {
            found.push(Embedded {
                offset,
                width,
                height,
            });
        }
    }
    found
}

/// A JPEG stream's size from its baseline, extended or progressive frame
/// header; `None` when its markers do not read as a JPEG's.
fn frame_size(jpeg: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    for _ in 0..64 {
        if *jpeg.get(at)? != 0xFF {
            return None;
        }
        let mut marker = *jpeg.get(at + 1)?;
        // Fill bytes.
        while marker == 0xFF {
            at += 1;
            marker = *jpeg.get(at + 1)?;
        }
        match marker {
            0xC0..=0xC2 => {
                let h = u16::from_be_bytes([*jpeg.get(at + 5)?, *jpeg.get(at + 6)?]);
                let w = u16::from_be_bytes([*jpeg.get(at + 7)?, *jpeg.get(at + 8)?]);
                return Some((u32::from(w), u32::from(h)));
            }
            0x01 | 0xD0..=0xD7 => at += 2,
            // Lossless, arithmetic or hierarchical frames (a raw's
            // mosaic, not a preview), or a stream with no frame header.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xD8..=0xDA | 0x00 => return None,
            _ => {
                let len = u16::from_be_bytes([*jpeg.get(at + 2)?, *jpeg.get(at + 3)?]);
                if len < 2 {
                    return None;
                }
                at += 2 + usize::from(len);
            }
        }
    }
    None
}

/// The stream to read for a thumbnail `long` pixels on its long side: the
/// smallest at least that large, else the largest there is.
pub fn choose(found: &[Embedded], long: u32) -> Vec<Embedded> {
    let mut order: Vec<Embedded> = found.to_vec();
    // Large enough first, smallest first; then the rest, largest first.
    order.sort_by_key(|e| {
        if e.long() >= long {
            (0, e.long())
        } else {
            (1, u32::MAX - e.long())
        }
    });
    order
}

/// Decodes a JPEG stream at the smallest DCT scale that keeps its long
/// side at least `long`, then scales it to `long` (never up).
pub fn decode_jpeg(jpeg: &[u8], long: u32) -> Result<Rgb8, String> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
    decoder.read_info().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("no frame header")?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let ratio = f64::from(long) / f64::from(w.max(h).max(1));
    if ratio < 1.0 {
        let want = |v: u32| ((f64::from(v) * ratio).ceil() as u16).max(1);
        decoder.scale(want(w), want(h)).map_err(|e| e.to_string())?;
    }
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("no frame header")?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let rgb = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels,
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&v| [v, v, v]).collect(),
        jpeg_decoder::PixelFormat::L16 => pixels
            .chunks_exact(2)
            .flat_map(|v| [v[0], v[0], v[0]])
            .collect(),
        jpeg_decoder::PixelFormat::CMYK32 => pixels
            .chunks_exact(4)
            .flat_map(|p| {
                let k = u16::from(p[3]);
                [0, 1, 2].map(|c| ((u16::from(p[c]) * k) / 255) as u8)
            })
            .collect(),
    };
    let image = RgbImage::from_raw(w, h, rgb).ok_or("a short JPEG decode")?;
    Ok(fit(image, long))
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
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(from_bytes(bytes, Kind::of(path), long))
}

/// What rawler is asked for, smallest first.
enum RawGet {
    Thumbnail,
    Preview,
    Full,
}

/// [`read`] from a file's bytes.
pub fn from_bytes(bytes: Vec<u8>, kind: Kind, long: u32) -> Option<Rgb8> {
    for e in choose(&embedded_jpegs(&bytes), long) {
        if let Ok(rgb) = decode_jpeg(&bytes[e.offset..], long) {
            return Some(rgb);
        }
    }
    let image = match kind {
        Kind::Raw => {
            let source = RawSource::new_from_shared_vec(Arc::new(bytes));
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
