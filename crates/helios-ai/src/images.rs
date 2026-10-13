//! Images on their way to a model: decoded, downscaled to a longest side,
//! re-encoded. Re-encoding drops the file's metadata (its GPS position
//! among them): only pixels leave.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, RgbImage};

use crate::provider::ImageData;

/// `bytes` (a PNG, a JPEG) at most `max_side` pixels on its longest side,
/// as a PNG when it was one and small, a JPEG otherwise.
pub fn prepare(bytes: &[u8], max_side: u32) -> Result<ImageData, String> {
    let decoded =
        image::load_from_memory(bytes).map_err(|e| format!("the image cannot be read: {e}"))?;
    let (w, h) = (decoded.width(), decoded.height());
    let scaled = if w.max(h) > max_side && max_side > 0 {
        decoded.resize(max_side, max_side, FilterType::Triangle)
    } else {
        decoded
    };
    let was_png = image::guess_format(bytes).is_ok_and(|f| f == ImageFormat::Png);
    let mut out = Cursor::new(Vec::new());
    if was_png && scaled.width() * scaled.height() <= 512 * 512 {
        scaled
            .write_to(&mut out, ImageFormat::Png)
            .map_err(|e| format!("encoding the image: {e}"))?;
        return Ok(ImageData {
            mime: "image/png".into(),
            bytes: out.into_inner(),
        });
    }
    DynamicImage::ImageRgb8(scaled.to_rgb8())
        .write_to(&mut out, ImageFormat::Jpeg)
        .map_err(|e| format!("encoding the image: {e}"))?;
    Ok(ImageData {
        mime: "image/jpeg".into(),
        bytes: out.into_inner(),
    })
}

/// A test card: gradients and a disc, `side` pixels square, as a PNG. The
/// bench's image when it is given none.
pub fn test_card(side: u32) -> Vec<u8> {
    let side = side.max(8);
    let c = side as f32 / 2.0;
    let card = RgbImage::from_fn(side, side, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let inside = (fx - c).hypot(fy - c) < c / 2.0;
        let r = (fx / side as f32 * 255.0) as u8;
        let g = (fy / side as f32 * 255.0) as u8;
        if inside {
            image::Rgb([230, 200, 40])
        } else {
            image::Rgb([r, g, 160])
        }
    });
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(card)
        .write_to(&mut out, ImageFormat::Png)
        .expect("a PNG encodes in memory");
    out.into_inner()
}

/// The size of an encoded image.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::load_from_memory(bytes)
        .ok()
        .map(|i| (i.width(), i.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_image_is_downscaled_and_re_encoded() {
        let card = test_card(600);
        let sent = prepare(&card, 256).unwrap();
        assert_eq!(dimensions(&sent.bytes), Some((256, 256)));
        assert_eq!(sent.mime, "image/png");
        let big = prepare(&test_card(1200), 1024).unwrap();
        assert_eq!(big.mime, "image/jpeg");
        assert_eq!(dimensions(&big.bytes), Some((1024, 1024)));
        assert!(prepare(b"not an image", 256).is_err());
    }
}
