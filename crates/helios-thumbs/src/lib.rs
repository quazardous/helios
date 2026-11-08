//! Thumbnails: what the library, the filmstrip and Compare draw.
//!
//! - [`read`]: the fast first read of a file, its smallest embedded JPEG
//!   at least as large as the thumbnail, decoded with DCT scaling straight
//!   to 8-bit sRGB;
//! - [`ThumbCache`]: JPEG thumbnails on disk ([`SMALL`] and [`LARGE`]),
//!   keyed by image (its file's hash: content-addressed), version and
//!   chain version, with a size limit kept by LRU eviction;
//! - [`Loader`]: the few threads that fill them, the cache first, the
//!   images in view first;
//! - [`placeholder`]: the cell of a file with no readable preview.

pub mod cache;
pub mod pool;
pub mod read;

pub use cache::{DEFAULT_LIMIT, Key, READER, ThumbCache};
pub use pool::{Job, Loader, Outcome, workers};

/// The library's thumbnails: their long side, in pixels.
pub const SMALL: u32 = 256;
/// Compare's and the loupe's first paint: the long side, in pixels.
pub const LARGE: u32 = 1024;

/// An 8-bit sRGB picture, three bytes a pixel, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgb8 {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The cache key of an image's thumbnail, from its id. Every lookup goes
/// through it: keys move to the image map's content identity later (see
/// `docs/design/image-map.md`).
pub fn key_of(id: &str) -> Key {
    Key::preview(id)
}

/// The cache's folder for a library whose catalogue is `vault_file`:
/// `.helios/thumbs` beside it.
pub fn cache_dir(vault_file: &std::path::Path) -> std::path::PathBuf {
    let library = vault_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    library.join(".helios").join("thumbs")
}

/// 5×7 glyphs, a row a byte (the low five bits, the leftmost the highest).
const GLYPHS: &[(char, [u8; 7])] = &[
    ('0', [14, 17, 19, 21, 25, 17, 14]),
    ('1', [4, 12, 4, 4, 4, 4, 14]),
    ('2', [14, 17, 1, 2, 4, 8, 31]),
    ('3', [31, 2, 4, 2, 1, 17, 14]),
    ('4', [2, 6, 10, 18, 31, 2, 2]),
    ('5', [31, 16, 30, 1, 1, 17, 14]),
    ('6', [6, 8, 16, 30, 17, 17, 14]),
    ('7', [31, 1, 2, 4, 8, 8, 8]),
    ('8', [14, 17, 17, 14, 17, 17, 14]),
    ('9', [14, 17, 17, 15, 1, 2, 12]),
    ('A', [14, 17, 17, 31, 17, 17, 17]),
    ('B', [30, 17, 17, 30, 17, 17, 30]),
    ('C', [14, 17, 16, 16, 16, 17, 14]),
    ('D', [28, 18, 17, 17, 17, 18, 28]),
    ('E', [31, 16, 16, 30, 16, 16, 31]),
    ('F', [31, 16, 16, 30, 16, 16, 16]),
    ('G', [14, 17, 16, 23, 17, 17, 15]),
    ('H', [17, 17, 17, 31, 17, 17, 17]),
    ('I', [14, 4, 4, 4, 4, 4, 14]),
    ('J', [7, 2, 2, 2, 2, 18, 12]),
    ('K', [17, 18, 20, 24, 20, 18, 17]),
    ('L', [16, 16, 16, 16, 16, 16, 31]),
    ('M', [17, 27, 21, 21, 17, 17, 17]),
    ('N', [17, 17, 25, 21, 19, 17, 17]),
    ('O', [14, 17, 17, 17, 17, 17, 14]),
    ('P', [30, 17, 17, 30, 16, 16, 16]),
    ('Q', [14, 17, 17, 17, 21, 18, 13]),
    ('R', [30, 17, 17, 30, 20, 18, 17]),
    ('S', [15, 16, 16, 14, 1, 1, 30]),
    ('T', [31, 4, 4, 4, 4, 4, 4]),
    ('U', [17, 17, 17, 17, 17, 17, 14]),
    ('V', [17, 17, 17, 17, 17, 10, 4]),
    ('W', [17, 17, 17, 21, 21, 21, 10]),
    ('X', [17, 17, 10, 4, 10, 17, 17]),
    ('Y', [17, 17, 17, 10, 4, 4, 4]),
    ('Z', [31, 1, 2, 4, 8, 16, 31]),
    ('?', [14, 17, 1, 2, 4, 0, 4]),
];

/// The cell of a file with no readable preview: a neutral grey picture,
/// [`SMALL`] wide, with the file's type (`kind`, its extension) written
/// large and a crossed frame under it ("no preview").
pub fn placeholder(kind: &str) -> Rgb8 {
    const W: u32 = SMALL;
    const H: u32 = SMALL * 2 / 3;
    let back = [72u8, 72, 72];
    let ink = [168u8, 168, 168];
    let mut pixels: Vec<u8> = std::iter::repeat_n(back, (W * H) as usize)
        .flatten()
        .collect();
    let mut dot = |x: u32, y: u32| {
        if x < W && y < H {
            let i = ((y * W + x) * 3) as usize;
            pixels[i..i + 3].copy_from_slice(&ink);
        }
    };
    let text: Vec<char> = kind
        .chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(|c| GLYPHS.iter().any(|(g, _)| g == c))
        .take(5)
        .collect();
    let text = if text.is_empty() { vec!['?'] } else { text };
    let scale = 7;
    let advance = 6 * scale;
    let width = advance * text.len() as u32 - scale;
    let (x0, y0) = ((W - width.min(W)) / 2, 28);
    for (n, c) in text.iter().enumerate() {
        let rows = GLYPHS.iter().find(|(g, _)| g == c).map(|(_, r)| r);
        for (y, row) in rows.into_iter().flatten().enumerate() {
            for x in 0..5u32 {
                if row & (16 >> x) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            dot(
                                x0 + n as u32 * advance + x * scale + dx,
                                y0 + y as u32 * scale + dy,
                            );
                        }
                    }
                }
            }
        }
    }
    // "No preview": a frame, crossed.
    let (fx, fy, fw, fh) = (W / 2 - 30, 108, 60, 40);
    for t in 0..3 {
        for x in fx..fx + fw {
            dot(x, fy + t);
            dot(x, fy + fh - 1 - t);
        }
        for y in fy..fy + fh {
            dot(fx + t, y);
            dot(fx + fw - 1 - t, y);
        }
    }
    for x in 0..fw {
        let y = x * fh / fw;
        for t in 0..3 {
            dot(fx + x, fy + y + t);
            dot(fx + x, fy + fh - 1 - y - t);
        }
    }
    Rgb8 {
        width: W,
        height: H,
        pixels,
    }
}
