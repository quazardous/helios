//! What a picture says about its colours.
//!
//! A picture's values mean nothing without the space they are in. The
//! loader reads the file's tags in this order, the first one it can use
//! winning (as browsers and the PNG third edition do):
//!
//! 1. PNG's cICP chunk;
//! 2. the embedded ICC profile (JPEG APP2, PNG iCCP, TIFF tag 34675);
//! 3. PNG's sRGB chunk, then its gAMA chunk (with cHRM when present);
//! 4. Exif `ColorSpace`: 1 is sRGB; 2, or "uncalibrated" (0xFFFF) with the
//!    interoperability index R03, is Adobe RGB, as cameras write it;
//! 5. nothing: sRGB, assumed, and the report says so.
//!
//! A tag that is there but cannot be used (a broken profile, an HDR cICP
//! curve) is skipped with its reason in [`Colour::ignored`]: the picture
//! still loads.

use std::borrow::Cow;
use std::io::Cursor;

use helios_colour::{Curve, InputSpace, OutputSpace};
use image::codecs::tiff::TiffDecoder;
use image::{ImageDecoder, ImageFormat};
use serde::Serialize;

/// Where a picture's colours were read from.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "tag")]
pub enum ColourTag {
    /// PNG's cICP chunk: primaries, transfer, matrix, full range.
    Cicp { code_points: [u8; 4] },
    /// The embedded ICC profile, with its description.
    Icc { description: Option<String> },
    /// PNG's sRGB chunk.
    PngSrgb,
    /// PNG's gAMA chunk (the file's encoding gamma), with cHRM or not.
    PngGamma { gamma: f64, chromaticities: bool },
    /// Exif `ColorSpace` (and the interoperability index).
    Exif {
        color_space: u16,
        interop: Option<String>,
    },
    /// Nothing usable: sRGB is assumed.
    Untagged,
}

/// What a picture's colours were read as: the load report's colour part.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Colour {
    pub from: ColourTag,
    /// The space in words: a profile's description, a known space's name,
    /// or "sRGB (assumed)".
    pub space: String,
    /// Tags found but not used, each with why.
    pub ignored: Vec<String>,
}

impl Colour {
    /// No tag was usable: the colours were taken as sRGB.
    pub fn assumed(&self) -> bool {
        self.from == ColourTag::Untagged
    }
}

/// The tags a file carries, before any is chosen.
#[derive(Default)]
pub(crate) struct Tags<'a> {
    cicp: Option<[u8; 4]>,
    icc: Option<Vec<u8>>,
    png_srgb: bool,
    /// PNG gAMA, the encoding gamma (0.45455 for a 2.2 display gamma).
    png_gamma: Option<f64>,
    /// PNG cHRM: white, red, green, blue (CIE xy).
    png_chromaticities: Option<[[f64; 2]; 4]>,
    /// The Exif block, a TIFF structure.
    exif: Option<Cow<'a, [u8]>>,
}

/// Reads the tags of a picture: its bytes, and the decoder that will give
/// its pixels (for JPEG, whose decoder has already read the tags). Never
/// fails: what cannot be read is absent.
pub(crate) fn tags<'a>(
    bytes: &'a [u8],
    format: Option<ImageFormat>,
    decoder: &mut impl ImageDecoder,
) -> Tags<'a> {
    match format {
        Some(ImageFormat::Png) => png_tags(bytes),
        Some(ImageFormat::Jpeg) => Tags {
            icc: decoder.icc_profile().ok().flatten(),
            exif: decoder.exif_metadata().ok().flatten().map(Cow::Owned),
            ..Tags::default()
        },
        Some(ImageFormat::Tiff) => Tags {
            // Not from the decoder `ImageReader` made: its default limits
            // give the TIFF decoder a tag value budget the size of the
            // pixel buffer (image 0.25), so a small image's profile
            // exceeds it and `icc_profile` answers None although the tag
            // is there. A decoder of its own keeps `tiff`'s limits.
            icc: TiffDecoder::new(Cursor::new(bytes))
                .ok()
                .and_then(|mut d| d.icc_profile().ok().flatten()),
            // A TIFF file is itself the TIFF structure Exif uses: its first
            // directory points to the Exif one.
            exif: Some(Cow::Borrowed(bytes)),
            ..Tags::default()
        },
        _ => Tags::default(),
    }
}

fn png_tags(bytes: &[u8]) -> Tags<'static> {
    let Ok(reader) = png::Decoder::new(Cursor::new(bytes)).read_info() else {
        return Tags::default();
    };
    let info = reader.info();
    let xy = |(x, y): (png::ScaledFloat, png::ScaledFloat)| {
        [f64::from(x.into_value()), f64::from(y.into_value())]
    };
    Tags {
        cicp: info.coding_independent_code_points.map(|c| {
            [
                c.color_primaries,
                c.transfer_function,
                c.matrix_coefficients,
                u8::from(c.is_video_full_range_image),
            ]
        }),
        icc: info.icc_profile.as_ref().map(|p| p.to_vec()),
        png_srgb: info.srgb.is_some(),
        // The chunks as the file has them: `png`'s decoder fills
        // `gama_chunk` and `chrm_chunk`, while `source_gamma` and
        // `source_chromaticities` are only what an encoder is told to write
        // (png 0.18), so they stay empty on read.
        png_gamma: info.gama_chunk.map(|g| f64::from(g.into_value())),
        png_chromaticities: info
            .chrm_chunk
            .map(|c| [xy(c.white), xy(c.red), xy(c.green), xy(c.blue)]),
        exif: info.exif_metadata.as_ref().map(|e| Cow::Owned(e.to_vec())),
    }
}

/// Chooses the space of a picture from its tags; `gray` says whether its
/// data has one channel (a profile must agree).
pub(crate) fn choose(tags: &Tags<'_>, gray: bool) -> (InputSpace, Colour) {
    let mut ignored = Vec::new();
    let found = |space: InputSpace, from: ColourTag, name: String, ignored: Vec<String>| {
        (
            space,
            Colour {
                from,
                space: name,
                ignored,
            },
        )
    };
    if let Some(code_points) = tags.cicp {
        match InputSpace::from_cicp(code_points) {
            Ok(space) => {
                let name = cicp_name(code_points);
                return found(space, ColourTag::Cicp { code_points }, name, ignored);
            }
            Err(e) => ignored.push(e),
        }
    }
    if let Some(icc) = &tags.icc {
        match InputSpace::from_icc(icc) {
            Ok(space) if space.is_gray() != gray => ignored.push(format!(
                "the ICC profile is for {} data, the image is {}",
                if space.is_gray() { "grey" } else { "colour" },
                if gray { "grey" } else { "in colour" },
            )),
            Ok(space) => {
                let description = space.description().map(str::to_string);
                let name = description
                    .clone()
                    .unwrap_or_else(|| "embedded ICC profile".to_string());
                return found(space, ColourTag::Icc { description }, name, ignored);
            }
            Err(e) => ignored.push(e),
        }
    }
    if tags.png_srgb {
        let name = OutputSpace::Srgb.name().to_string();
        return found(InputSpace::srgb(), ColourTag::PngSrgb, name, ignored);
    }
    if let Some(gamma) = tags.png_gamma {
        if gamma > 0.0 {
            let (primaries, white) = match tags.png_chromaticities {
                Some([w, r, g, b]) => ([r, g, b], w),
                None => (OutputSpace::Srgb.primaries(), helios_colour::D65),
            };
            match InputSpace::from_chromaticities(primaries, white, Curve::Gamma(1.0 / gamma)) {
                Ok(space) => {
                    let chromaticities = tags.png_chromaticities.is_some();
                    let name = format!("gamma {:.2}", 1.0 / gamma);
                    let from = ColourTag::PngGamma {
                        gamma,
                        chromaticities,
                    };
                    return found(space, from, name, ignored);
                }
                Err(e) => ignored.push(format!("PNG cHRM: {e}")),
            }
        } else {
            ignored.push("PNG gAMA of zero".to_string());
        }
    }
    if let Some(exif) = tags.exif.as_deref().and_then(exif_colour) {
        let (color_space, interop) = exif;
        let interop_name = interop.map(|i| String::from_utf8_lossy(&i).into_owned());
        let space = match (color_space, interop.as_ref()) {
            (1, _) => Some(OutputSpace::Srgb),
            (2, _) | (0xFFFF, Some(b"R03")) => Some(OutputSpace::AdobeRgb),
            _ => None,
        };
        let from = ColourTag::Exif {
            color_space,
            interop: interop_name,
        };
        match space {
            // A grey picture comes as R = G = B: the space's curve on a
            // neutral, which is right for grey too.
            Some(space) => {
                let name = space.name().to_string();
                return found(InputSpace::of(space), from, name, ignored);
            }
            None => ignored.push(format!(
                "Exif ColorSpace {color_space:#06x} names no space Helios knows"
            )),
        }
    }
    let name = format!("{} (assumed)", OutputSpace::Srgb.name());
    found(InputSpace::srgb(), ColourTag::Untagged, name, ignored)
}

fn cicp_name([primaries, transfer, _, _]: [u8; 4]) -> String {
    let named = OutputSpace::ALL.into_iter().find(|s| {
        s.cicp()
            .is_some_and(|c| c[0] == primaries && c[1] == transfer)
    });
    match named {
        Some(space) => space.name().to_string(),
        None => format!("cICP {primaries}/{transfer}"),
    }
}

/// A TIFF structure (an Exif block, or a TIFF file), read with bounds
/// checks everywhere: a broken one reads as nothing.
struct Ifds<'a> {
    b: &'a [u8],
    little: bool,
}

impl<'a> Ifds<'a> {
    fn new(b: &'a [u8]) -> Option<Ifds<'a>> {
        // Some writers keep the APP1 header in the block.
        let b = b.strip_prefix(b"Exif\0\0").unwrap_or(b);
        let little = match b.get(0..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Ifds { b, little })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let v: [u8; 2] = self.b.get(at..at + 2)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(v)
        } else {
            u16::from_be_bytes(v)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let v: [u8; 4] = self.b.get(at..at + 4)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(v)
        } else {
            u32::from_be_bytes(v)
        })
    }

    /// The entry `tag` of the directory at `ifd`: its type, count and the
    /// offset of its 4-byte value field.
    fn entry(&self, ifd: usize, tag: u16) -> Option<(u16, u32, usize)> {
        let count = self.u16(ifd)? as usize;
        (0..count.min(1024)).find_map(|i| {
            let at = ifd + 2 + 12 * i;
            (self.u16(at)? == tag).then_some(())?;
            Some((self.u16(at + 2)?, self.u32(at + 4)?, at + 8))
        })
    }

    /// The directory a pointer tag (LONG or IFD) of `ifd` points to.
    fn pointer(&self, ifd: usize, tag: u16) -> Option<usize> {
        let (kind, _, at) = self.entry(ifd, tag)?;
        matches!(kind, 4 | 13).then_some(())?;
        Some(self.u32(at)? as usize)
    }
}

/// Exif `ColorSpace` (tag 0xA001) and the interoperability index (tag 1 of
/// the interoperability directory, "R98" or "R03"), when there.
fn exif_colour(block: &[u8]) -> Option<(u16, Option<[u8; 3]>)> {
    let t = Ifds::new(block)?;
    let ifd0 = t.u32(4)? as usize;
    let exif = t.pointer(ifd0, 0x8769)?;
    let (kind, _, at) = t.entry(exif, 0xA001)?;
    // SHORT, in the first two bytes of the value field.
    (kind == 3).then_some(())?;
    let color_space = t.u16(at)?;
    let interop = t.pointer(exif, 0xA005).and_then(|ifd| {
        let (kind, count, at) = t.entry(ifd, 0x0001)?;
        // ASCII, "R03\0" fits in the value field.
        (kind == 2 && count <= 4).then_some(())?;
        t.b.get(at..at + 3)?.try_into().ok()
    });
    Some((color_space, interop))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little- or big-endian Exif block: `ColorSpace` and, when given, an
    /// interoperability index.
    fn exif_block(little: bool, color_space: u16, interop: Option<&[u8; 3]>) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut b = if little {
            b"II".to_vec()
        } else {
            b"MM".to_vec()
        };
        b.extend(u16b(42));
        b.extend(u32b(8));
        // IFD0 at 8: one entry, the Exif pointer to 26.
        b.extend(u16b(1));
        b.extend(u16b(0x8769));
        b.extend(u16b(4));
        b.extend(u32b(1));
        b.extend(u32b(26));
        b.extend(u32b(0));
        // Exif IFD at 26: `ColorSpace`, then the interop pointer to 56.
        let entries = if interop.is_some() { 2 } else { 1 };
        b.extend(u16b(entries));
        b.extend(u16b(0xA001));
        b.extend(u16b(3));
        b.extend(u32b(1));
        b.extend(u16b(color_space));
        b.extend([0, 0]);
        if let Some(index) = interop {
            b.extend(u16b(0xA005));
            b.extend(u16b(4));
            b.extend(u32b(1));
            b.extend(u32b(56));
            b.extend(u32b(0));
            assert_eq!(b.len(), 56);
            b.extend(u16b(1));
            b.extend(u16b(1));
            b.extend(u16b(2));
            b.extend(u32b(4));
            b.extend(index);
            b.push(0);
        }
        b.extend(u32b(0));
        b
    }

    #[test]
    fn exif_colour_space_and_interop_are_read() {
        for little in [true, false] {
            assert_eq!(
                exif_colour(&exif_block(little, 0xFFFF, Some(b"R03"))),
                Some((0xFFFF, Some(*b"R03")))
            );
            assert_eq!(exif_colour(&exif_block(little, 1, None)), Some((1, None)));
        }
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend(exif_block(true, 1, None));
        assert_eq!(exif_colour(&prefixed), Some((1, None)));
    }

    #[test]
    fn broken_exif_reads_as_nothing() {
        let whole = exif_block(true, 0xFFFF, Some(b"R03"));
        for len in 0..whole.len() {
            // Cut anywhere: never a panic.
            let _ = exif_colour(&whole[..len]);
        }
        assert_eq!(exif_colour(&whole), Some((0xFFFF, Some(*b"R03"))));
        assert_eq!(exif_colour(b"II*\0\xff\xff\xff\xff"), None);
    }

    #[test]
    fn tags_are_taken_in_order() {
        let p3 = OutputSpace::DisplayP3.icc_v4();
        let both = Tags {
            cicp: Some([9, 1, 0, 1]),
            icc: Some(p3.clone()),
            ..Tags::default()
        };
        assert_eq!(
            choose(&both, false).1.from,
            ColourTag::Cicp {
                code_points: [9, 1, 0, 1]
            }
        );
        // An HDR cICP is skipped, said, and the profile used.
        let hdr = Tags {
            cicp: Some([9, 16, 0, 1]),
            icc: Some(p3),
            png_srgb: true,
            ..Tags::default()
        };
        let (_, colour) = choose(&hdr, false);
        assert!(matches!(colour.from, ColourTag::Icc { .. }), "{colour:?}");
        assert_eq!(colour.space, "Helios Display P3");
        assert_eq!(colour.ignored.len(), 1, "{colour:?}");
        // A broken profile falls to the next tag, with its reason.
        let broken = Tags {
            icc: Some(b"garbage".to_vec()),
            exif: Some(Cow::Owned(exif_block(true, 0xFFFF, Some(b"R03")))),
            ..Tags::default()
        };
        let (_, colour) = choose(&broken, false);
        assert_eq!(colour.space, "Adobe RGB (1998)");
        assert!(colour.ignored[0].contains("ICC"), "{colour:?}");
        // Nothing: assumed sRGB.
        let (_, colour) = choose(&Tags::default(), false);
        assert!(colour.assumed());
        assert_eq!(colour.space, "sRGB (assumed)");
        assert!(colour.ignored.is_empty());
        // A colour profile on a grey image is not used.
        let (_, colour) = choose(
            &Tags {
                icc: Some(OutputSpace::AdobeRgb.icc_v2()),
                ..Tags::default()
            },
            true,
        );
        assert!(colour.assumed() && colour.ignored.len() == 1, "{colour:?}");
    }
}
