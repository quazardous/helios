//! What a file says of how it was taken, read without decoding a pixel:
//! the metadata a library indexes (the camera, lens, settings, capture
//! time with its offset, size and orientation).

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use serde::Serialize;

use crate::{Kind, crop, ratio, raw_tag};

/// How a file was taken, as its metadata says; `None`: it does not say.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Capture {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    /// As the camera wrote it: `YYYY:MM:DD HH:MM:SS`, its local time.
    pub taken: Option<String>,
    /// The fraction of a second after `taken`, as digits.
    pub subsec: Option<String>,
    /// The clock's offset from UTC as written (`+02:00`).
    pub offset: Option<String>,
    /// Seconds.
    pub exposure_time: Option<f64>,
    /// f-number.
    pub aperture: Option<f64>,
    pub iso: Option<f64>,
    /// Millimetres.
    pub focal_length: Option<f64>,
    /// The picture's size as stored (a raw: its crop), before its
    /// orientation turns it.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The EXIF orientation, 1 to 8.
    pub orientation: Option<u8>,
}

fn some(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Reads how a file was taken. A raw is opened by rawler for its
/// metadata and its geometry (no photosite decoded); a picture's Exif
/// block and size are read from its bytes.
///
/// # Errors
/// The file cannot be read, or is no raw or picture Helios reads.
pub fn capture(path: &Path) -> Result<Capture, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let named = |e: String| format!("{}: {e}", path.display());
    if Kind::of(path) == Kind::Picture {
        let reader = image::ImageReader::new(Cursor::new(&bytes[..]))
            .with_guessed_format()
            .map_err(|e| named(e.to_string()))?;
        let format = reader.format();
        let mut c = crate::colour::exif_of(&bytes, format).map_or_else(Capture::default, |block| {
            crate::colour::exif_capture(&block)
        });
        let (w, h) = reader.into_dimensions().map_err(|e| named(e.to_string()))?;
        c.width = Some(w);
        c.height = Some(h);
        return Ok(c);
    }
    let source = RawSource::new_from_shared_vec(Arc::new(bytes));
    let decoder = rawler::get_decoder(&source).map_err(|e| named(e.to_string()))?;
    let params = RawDecodeParams::default();
    let unsigned = |r: Option<rawler::formats::tiff::Rational>| {
        r.filter(|r| r.d != 0)
            .map(|r| ratio(f64::from(r.n), f64::from(r.d)))
            .filter(|v| *v > 0.0)
    };
    let mut c = decoder.raw_metadata(&source, &params).map_or_else(
        |_| Capture::default(),
        |m| {
            let exif = &m.exif;
            Capture {
                make: some(&m.make),
                model: some(&m.model),
                lens: exif
                    .lens_model
                    .as_deref()
                    .and_then(some)
                    .or_else(|| m.lens.as_ref().and_then(|l| some(&l.lens_model))),
                taken: exif
                    .date_time_original
                    .clone()
                    .or_else(|| exif.create_date.clone()),
                subsec: exif.sub_sec_time_original.clone(),
                offset: exif
                    .offset_time_original
                    .clone()
                    .or_else(|| exif.offset_time.clone()),
                exposure_time: unsigned(exif.exposure_time),
                aperture: unsigned(exif.fnumber),
                iso: exif
                    .iso_speed
                    .or(exif.recommended_exposure_index)
                    .map(f64::from)
                    .or(exif.iso_speed_ratings.map(f64::from))
                    .filter(|v| *v > 0.0),
                focal_length: unsigned(exif.focal_length),
                ..Capture::default()
            }
        },
    );
    // A dummy image: its geometry without its photosites.
    if let Ok(raw) = decoder.raw_image(&source, &params, true)
        && let Ok((_, _, w, h)) = crop(&raw)
    {
        c.width = u32::try_from(w).ok();
        c.height = u32::try_from(h).ok();
    }
    c.orientation = Some(raw_tag(decoder.as_ref(), &source));
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian Exif block: Make, Model, Orientation, and an Exif
    /// directory with a time, its offset, settings and a lens.
    fn block() -> Vec<u8> {
        let mut b = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        let entry = |tag: u16, kind: u16, count: u32, value: u32| {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend(kind.to_le_bytes());
            e.extend(count.to_le_bytes());
            e.extend(value.to_le_bytes());
            e
        };
        // IFD0 at 8: 4 entries (2 + 48 + 4 = 54 bytes), data from 62.
        let data0 = 8 + 2 + 4 * 12 + 4;
        let make = b"Fuji\0";
        let model = b"X-T5\0";
        let exif_at = data0 + make.len() + model.len();
        b.extend(4u16.to_le_bytes());
        b.extend(entry(0x010F, 2, make.len() as u32, data0 as u32));
        b.extend(entry(
            0x0110,
            2,
            model.len() as u32,
            (data0 + make.len()) as u32,
        ));
        b.extend(entry(0x0112, 3, 1, 6));
        b.extend(entry(0x8769, 4, 1, exif_at as u32));
        b.extend(0u32.to_le_bytes());
        b.extend(make);
        b.extend(model);
        // The Exif directory: 7 entries, then its data.
        let data = exif_at + 2 + 7 * 12 + 4;
        let taken = b"2024:06:01 23:30:00\0";
        let offset = b"+09:00\0";
        let lens = b"XF35mmF1.4 R\0";
        let rationals = data + taken.len() + offset.len() + lens.len();
        b.extend(7u16.to_le_bytes());
        b.extend(entry(0x829A, 5, 1, rationals as u32));
        b.extend(entry(0x829D, 5, 1, (rationals + 8) as u32));
        b.extend(entry(0x8827, 3, 1, 800));
        b.extend(entry(0x9003, 2, taken.len() as u32, data as u32));
        b.extend(entry(
            0x9011,
            2,
            offset.len() as u32,
            (data + taken.len()) as u32,
        ));
        b.extend(entry(0x920A, 5, 1, (rationals + 16) as u32));
        b.extend(entry(
            0xA434,
            2,
            lens.len() as u32,
            (data + taken.len() + offset.len()) as u32,
        ));
        b.extend(0u32.to_le_bytes());
        b.extend(taken);
        b.extend(offset);
        b.extend(lens);
        for (n, d) in [(1u32, 250u32), (28, 10), (35, 1)] {
            b.extend(n.to_le_bytes());
            b.extend(d.to_le_bytes());
        }
        b
    }

    #[test]
    fn an_exif_block_says_how_a_picture_was_taken() {
        let c = crate::colour::exif_capture(&block());
        assert_eq!(c.make.as_deref(), Some("Fuji"));
        assert_eq!(c.model.as_deref(), Some("X-T5"));
        assert_eq!(c.orientation, Some(6));
        assert_eq!(c.taken.as_deref(), Some("2024:06:01 23:30:00"));
        assert_eq!(c.offset.as_deref(), Some("+09:00"));
        assert_eq!(c.lens.as_deref(), Some("XF35mmF1.4 R"));
        assert_eq!(c.iso, Some(800.0));
        assert_eq!(c.exposure_time, Some(1.0 / 250.0));
        assert_eq!(c.aperture, Some(2.8));
        assert_eq!(c.focal_length, Some(35.0));
        assert_eq!(crate::colour::exif_capture(b"junk"), Capture::default());
    }

    #[test]
    fn a_picture_without_exif_has_its_size() {
        let dir = std::env::temp_dir().join(format!("helios-load-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plain.png");
        image::RgbImage::new(6, 4).save(&path).unwrap();
        let c = capture(&path).unwrap();
        assert_eq!((c.width, c.height), (Some(6), Some(4)));
        assert_eq!(c.taken, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
