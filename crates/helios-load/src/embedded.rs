//! The JPEG streams a file embeds (a raw's previews and thumbnail, a
//! JPEG itself and its Exif thumbnail), found in its bytes, and their
//! decode with DCT scaling: a large preview decoded at 1/2, 1/4 or 1/8 of
//! its size straight from its coefficients, never decoded whole and then
//! scaled down.
//!
//! Decoding a JPEG at a fraction of its size by scaling its inverse DCT is
//! libjpeg's `scale_num` / `scale_denom`, which darktable's and
//! Lightroom's quick previews rely on; here it is `jpeg-decoder`'s
//! `Decoder::scale`.

use std::io::Cursor;

use image::RgbImage;

/// An embedded JPEG stream: where it starts, its size in pixels and its
/// colour components (1 for a grey stream, 3 or 4 for a colour one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Embedded {
    pub offset: usize,
    pub width: u32,
    pub height: u32,
    pub components: u8,
}

impl Embedded {
    /// Its long side, in pixels.
    pub fn long(&self) -> u32 {
        self.width.max(self.height)
    }

    /// A one-component stream: a grey picture, or no picture at all (a
    /// phone's HDR gain map, a depth map).
    pub fn grey(&self) -> bool {
        self.components == 1
    }
}

/// The baseline or progressive JPEG streams a file holds (a raw's
/// previews and thumbnail, a JPEG itself and its Exif thumbnail), found by
/// their start marker and checked up to their frame header. Lossless
/// streams (a raw's mosaic) are left out.
pub fn embedded_jpegs(bytes: &[u8]) -> Vec<Embedded> {
    let mut found = Vec::new();
    for offset in memchr::memmem::find_iter(bytes, b"\xFF\xD8\xFF") {
        if let Some((width, height, components)) = frame_size(&bytes[offset..])
            && width > 0
            && height > 0
        {
            found.push(Embedded {
                offset,
                width,
                height,
                components,
            });
        }
    }
    found
}

/// A JPEG stream's size and component count from its baseline, extended
/// or progressive frame header; `None` when its markers do not read as a
/// JPEG's.
fn frame_size(jpeg: &[u8]) -> Option<(u32, u32, u8)> {
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
                let components = *jpeg.get(at + 9)?;
                return Some((u32::from(w), u32::from(h), components));
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

/// The streams to read for a picture `long` pixels on its long side, in
/// order: the colour ones before the grey ones, and among each the
/// smallest at least that large, else the largest there is. A phone's
/// JPEG carries grey streams beside its picture (an Ultra HDR gain map, a
/// depth map), smaller than the picture and large enough: taken first,
/// they would show a colour photo grey. A grey picture's only stream is
/// still read.
pub fn choose(found: &[Embedded], long: u32) -> Vec<Embedded> {
    let mut order: Vec<Embedded> = found.to_vec();
    // Large enough first, smallest first; then the rest, largest first.
    order.sort_by_key(|e| {
        let (rank, size) = if e.long() >= long {
            (0, e.long())
        } else {
            (1, u32::MAX - e.long())
        };
        (e.grey(), rank, size)
    });
    order
}

/// Decodes the JPEG stream `jpeg` starts with at the smallest DCT scale
/// (1, 1/2, 1/4 or 1/8) that keeps its long side at least `long` pixels,
/// as 8-bit RGB: a 6048 px preview asked at least 2560 px long is decoded
/// at 3024 px, a quarter of the inverse DCT's work. Never scaled up: a
/// stream shorter than `long` is decoded whole.
///
/// # Errors
/// The stream does not decode.
pub fn decode_scaled(jpeg: &[u8], long: u32) -> Result<RgbImage, String> {
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
    RgbImage::from_raw(w, h, rgb).ok_or_else(|| "a short JPEG decode".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 128]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 90)
            .encode_image(&img)
            .unwrap();
        out
    }

    #[test]
    fn a_stream_is_decoded_at_the_smallest_scale_still_long_enough() {
        let file = jpeg(640, 400);
        // 1/4 is 160 long, 1/8 would be 80: at least 100 asked.
        assert_eq!(decode_scaled(&file, 100).unwrap().dimensions(), (160, 100));
        // 1/2 is 320: at least 200 asked.
        assert_eq!(decode_scaled(&file, 200).unwrap().dimensions(), (320, 200));
        // Never up.
        assert_eq!(decode_scaled(&file, 4000).unwrap().dimensions(), (640, 400));
    }

    #[test]
    fn the_smallest_colour_stream_long_enough_is_chosen_first() {
        let (small, large) = (jpeg(160, 100), jpeg(640, 400));
        let mut file = vec![0u8; 16];
        file.extend_from_slice(&small);
        file.extend_from_slice(&large);
        let found = embedded_jpegs(&file);
        assert_eq!(found.len(), 2);
        assert_eq!(choose(&found, 150)[0].width, 160);
        assert_eq!(choose(&found, 300)[0].width, 640);
        assert_eq!(choose(&found, 1000)[0].width, 640);
    }
}
