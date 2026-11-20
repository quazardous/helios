//! Image buffers and regions.
//!
//! An [`Image`] is an immutable buffer of interleaved `f32` channels covering a
//! [`Region`] of an image plane. It is shared by reference (cloning is cheap),
//! so the same pixels are held once in memory whoever uses them. A crop of an
//! image ([`Image::crop`], [`Image::sample`] inside it) is a view into the
//! same buffer, not a copy.

use std::fmt;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

/// A rectangle of the image plane, in absolute pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Region {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub const fn new(x: i64, y: i64, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The region starting at the origin.
    pub const fn sized(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    #[inline]
    pub fn right(&self) -> i64 {
        self.x + i64::from(self.width)
    }

    #[inline]
    pub fn bottom(&self) -> i64 {
        self.y + i64::from(self.height)
    }

    #[inline]
    pub fn area(&self) -> usize {
        self.width as usize * self.height as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The region grown by `margin` pixels on every side.
    pub fn grow(&self, margin: u32) -> Self {
        let m = i64::from(margin);
        Self::new(
            self.x - m,
            self.y - m,
            self.width + 2 * margin,
            self.height + 2 * margin,
        )
    }

    /// The smallest region holding both.
    pub fn bounds(&self, other: &Region) -> Region {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        let (right, bottom) = (
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        );
        Region::new(x, y, (right - x) as u32, (bottom - y) as u32)
    }

    /// Whether the two share at least one pixel.
    pub fn overlaps(&self, other: &Region) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    pub fn contains(&self, other: &Region) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// The full-resolution pixels a region at `scale` covers: those
    /// [`Image::sample_at`] averages to make it.
    pub fn full_resolution(&self, scale: f64) -> Region {
        if self.is_empty() {
            return Region::new(span(self.x, scale).0, span(self.y, scale).0, 0, 0);
        }
        let (x0, y0) = (span(self.x, scale).0, span(self.y, scale).0);
        let (x1, y1) = (
            span(self.right() - 1, scale).1,
            span(self.bottom() - 1, scale).1,
        );
        Region::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    /// Splits the region into a grid of tiles of at most `tile` pixels a side.
    pub fn tiles(&self, tile: u32) -> Vec<Region> {
        assert!(tile > 0, "tile size must be positive");
        let mut out = Vec::new();
        let mut y = self.y;
        while y < self.bottom() {
            let h = (self.bottom() - y).min(i64::from(tile)) as u32;
            let mut x = self.x;
            while x < self.right() {
                let w = (self.right() - x).min(i64::from(tile)) as u32;
                out.push(Region::new(x, y, w, h));
                x += i64::from(w);
            }
            y += i64::from(h);
        }
        out
    }
}

/// The full-resolution pixels `[lo, hi)` that pixel `v` at `scale` covers,
/// along one axis.
fn span(v: i64, scale: f64) -> (i64, i64) {
    let lo = (v as f64 / scale).floor() as i64;
    let hi = (((v + 1) as f64 / scale).floor() as i64).max(lo + 1);
    (lo, hi)
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}+{}+{}", self.width, self.height, self.x, self.y)
    }
}

/// A content hash (BLAKE3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// The first 12 hex digits, enough to tell hashes apart in a report.
    pub fn short(&self) -> String {
        self.to_string()[..12].to_string()
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.short())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Read back from the 64 hex digits it is written as.
impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let hex = String::deserialize(d)?;
        if hex.len() != 64 || !hex.is_ascii() {
            return Err(D::Error::custom(format!(
                "`{hex}` is not a hash's 64 hex digits"
            )));
        }
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| D::Error::custom(format!("`{hex}` is not a hash's 64 hex digits")))?;
        }
        Ok(Self(bytes))
    }
}

/// An immutable image buffer: interleaved `f32` channels over a region, or
/// a view of a part of one.
#[derive(Clone)]
pub struct Image {
    region: Region,
    channels: u32,
    /// The pixels, shared by every clone and every view of them. A vector
    /// rather than a slice: wrapping a module's output is then free, where
    /// `Arc<[f32]>` from a `Vec` copies it (100 MB on a 25 MP mosaic).
    buffer: Arc<Vec<f32>>,
    /// Where the first value of the region sits in `buffer`.
    offset: usize,
    /// Values from one row of the region to the next in `buffer`.
    stride: usize,
    /// For a view whose rows are not contiguous in `buffer`: its pixels
    /// row after row, copied the first time [`Image::data`] is asked.
    packed: Arc<OnceLock<Vec<f32>>>,
}

impl Image {
    /// Wraps pixel data; `data` holds `region.area() * channels` values, row by row.
    pub fn new(region: Region, channels: u32, mut data: Vec<f32>) -> Self {
        assert!(channels > 0, "an image has at least one channel");
        assert_eq!(
            data.len(),
            region.area() * channels as usize,
            "pixel data does not match {region} x {channels}"
        );
        // Kept as it comes (no copy), unless it holds much more room than
        // its pixels: the caches count an image by its pixels.
        if data.capacity() > data.len() + data.len() / 4 {
            data.shrink_to_fit();
        }
        Self {
            region,
            channels,
            buffer: Arc::new(data),
            offset: 0,
            stride: region.width as usize * channels as usize,
            packed: Arc::default(),
        }
    }

    /// Builds an image from a function of absolute coordinates and channel.
    pub fn from_fn(region: Region, channels: u32, f: impl Fn(i64, i64, u32) -> f32) -> Self {
        let mut data = Vec::with_capacity(region.area() * channels as usize);
        for y in region.y..region.bottom() {
            for x in region.x..region.right() {
                for c in 0..channels {
                    data.push(f(x, y, c));
                }
            }
        }
        Self::new(region, channels, data)
    }

    /// Builds an image row by row, `fill(y, row)` writing row `y`
    /// (absolute) of the region, `row` its `width * channels` values: in
    /// row bands over the machine's threads ([`crate::bands`]), so a
    /// per-pixel step on a full-resolution mosaic runs at memory speed.
    /// Each row is computed by the same code whatever thread runs it: the
    /// image is the one-thread image, bit for bit. The current cancel
    /// token ([`crate::cancel::current`]) is checked before each band:
    /// once set, the call gives up with [`crate::cancel::CANCELLED`].
    pub fn from_rows(
        region: Region,
        channels: u32,
        fill: impl Fn(i64, &mut [f32]) + Sync,
    ) -> Result<Self, String> {
        Self::from_rows_until(region, channels, &crate::cancel::current(), fill)
    }

    /// [`Image::from_rows`] checking `cancel`.
    pub fn from_rows_until(
        region: Region,
        channels: u32,
        cancel: &crate::cancel::Token,
        fill: impl Fn(i64, &mut [f32]) + Sync,
    ) -> Result<Self, String> {
        let row_len = region.width as usize * channels as usize;
        let mut data = vec![0.0f32; region.area() * channels as usize];
        crate::bands::rows(&mut data, row_len, cancel, |first, band| {
            for (i, row) in band.chunks_mut(row_len).enumerate() {
                fill(region.y + (first + i) as i64, row);
            }
        })?;
        Ok(Self::new(region, channels, data))
    }

    /// What a module gets for an optional input left unconnected: no
    /// pixels, an empty region at the origin.
    pub fn absent(channels: u32) -> Self {
        Self::new(Region::sized(0, 0), channels, Vec::new())
    }

    /// Whether this is an optional input left unconnected (no pixels).
    pub fn is_absent(&self) -> bool {
        self.region.is_empty()
    }

    #[inline]
    pub fn region(&self) -> Region {
        self.region
    }

    #[inline]
    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// Values in a row of the region.
    fn row_len(&self) -> usize {
        self.region.width as usize * self.channels as usize
    }

    /// Whether the region's rows follow each other in the buffer.
    fn contiguous(&self) -> bool {
        self.stride == self.row_len() || self.region.height <= 1
    }

    /// The pixels, row after row. A view whose rows are not contiguous in
    /// its buffer (a crop narrower than the image it comes from) copies them
    /// once, the first time: read such a view with [`Image::row`] or
    /// [`Image::pixel`] to avoid the copy.
    pub fn data(&self) -> &[f32] {
        if self.contiguous() {
            return &self.buffer
                [self.offset..self.offset + self.region.area() * self.channels as usize];
        }
        self.packed.get_or_init(|| self.to_vec())
    }

    /// The pixels, row after row, in a vector of their own.
    pub fn to_vec(&self) -> Vec<f32> {
        if self.contiguous() {
            return self.data().to_vec();
        }
        let mut out = Vec::with_capacity(self.region.area() * self.channels as usize);
        for y in self.region.y..self.region.bottom() {
            out.extend_from_slice(self.row(y));
        }
        out
    }

    /// The values of row `y` (absolute), which must lie in the region.
    pub fn row(&self, y: i64) -> &[f32] {
        let r = self.region;
        assert!(y >= r.y && y < r.bottom(), "row {y} outside {r}");
        let start = self.offset + (y - r.y) as usize * self.stride;
        &self.buffer[start..start + self.row_len()]
    }

    /// Bytes of its pixels (and of its packed copy, if one was made). A
    /// view counts its own pixels, not the whole buffer it shares.
    pub fn bytes(&self) -> usize {
        let own = self.region.area() * self.channels as usize * std::mem::size_of::<f32>();
        own + self
            .packed
            .get()
            .map_or(0, |p| std::mem::size_of_val(p.as_slice()))
    }

    /// A view of `region`, which must lie inside this image's region: no
    /// pixel is copied. `None` when it does not lie inside.
    pub fn crop(&self, region: Region) -> Option<Image> {
        let r = self.region;
        if !r.contains(&region) {
            return None;
        }
        if region == r {
            return Some(self.clone());
        }
        let offset = if region.is_empty() {
            self.offset
        } else {
            self.offset
                + (region.y - r.y) as usize * self.stride
                + (region.x - r.x) as usize * self.channels as usize
        };
        Some(Image {
            region,
            channels: self.channels,
            buffer: self.buffer.clone(),
            offset,
            stride: self.stride,
            packed: Arc::default(),
        })
    }

    /// The channels of the pixel at absolute coordinates, which must lie in the region.
    #[inline]
    pub fn pixel(&self, x: i64, y: i64) -> &[f32] {
        let r = self.region;
        assert!(
            x >= r.x && x < r.right() && y >= r.y && y < r.bottom(),
            "({x}, {y}) outside {r}"
        );
        let i = self.offset
            + (y - r.y) as usize * self.stride
            + (x - r.x) as usize * self.channels as usize;
        &self.buffer[i..i + self.channels as usize]
    }

    /// The pixels of `region`, replicating the edge pixels where it extends past
    /// this image (the framework's border policy for neighbourhood reads). A
    /// region inside the image is a view ([`Image::crop`]), not a copy.
    pub fn sample(&self, region: Region) -> Image {
        if let Some(view) = self.crop(region) {
            return view;
        }
        let r = self.region;
        assert!(!r.is_empty(), "cannot sample an empty image");
        // Row by row, in bands over the machine's threads: the columns
        // inside copied at once, those past each side the edge pixel
        // repeated. Never cancelled: a copy has no token to answer to.
        let channels = self.channels as usize;
        let (left, right) = (region.x, region.right());
        let (inside_from, inside_to) = (left.clamp(r.x, r.right()), right.clamp(r.x, r.right()));
        let before = (inside_from.min(right) - left).max(0) as usize * channels;
        let inside = (inside_to - inside_from).max(0) as usize * channels;
        let fill = |y: i64, out: &mut [f32]| {
            let row = self.row(y.clamp(r.y, r.bottom() - 1));
            let at = |x: i64| {
                let i = (x.clamp(r.x, r.right() - 1) - r.x) as usize * channels;
                &row[i..i + channels]
            };
            let (head, rest) = out.split_at_mut(before);
            for (x, px) in (left..).zip(head.chunks_exact_mut(channels)) {
                px.copy_from_slice(at(x));
            }
            let (middle, tail) = rest.split_at_mut(inside);
            if inside > 0 {
                let a = (inside_from - r.x) as usize * channels;
                middle.copy_from_slice(&row[a..a + inside]);
            }
            let start = inside_to.max(left);
            for (x, px) in (start..).zip(tail.chunks_exact_mut(channels)) {
                px.copy_from_slice(at(x));
            }
        };
        Image::from_rows_until(region, self.channels, &crate::cancel::Token::new(), fill)
            .expect("a token nobody holds is never cancelled")
    }

    /// [`Image::sample`] at a scale: `region` is in pixels at that scale,
    /// each one the mean of the full-resolution pixels it covers (edges
    /// replicated). At scale 1 it is `sample`.
    pub fn sample_at(&self, region: Region, scale: f64) -> Image {
        if scale == 1.0 {
            return self.sample(region);
        }
        let r = self.region;
        assert!(!r.is_empty(), "cannot sample an empty image");
        let channels = self.channels as usize;
        let mut data = Vec::with_capacity(region.area() * channels);
        let mut sum = vec![0.0f64; channels];
        for y in region.y..region.bottom() {
            let (y0, y1) = span(y, scale);
            for x in region.x..region.right() {
                let (x0, x1) = span(x, scale);
                sum.fill(0.0);
                for fy in y0..y1 {
                    let cy = fy.clamp(r.y, r.bottom() - 1);
                    for fx in x0..x1 {
                        let cx = fx.clamp(r.x, r.right() - 1);
                        for (s, v) in sum.iter_mut().zip(self.pixel(cx, cy)) {
                            *s += f64::from(*v);
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f64;
                data.extend(sum.iter().map(|s| (s / n) as f32));
            }
        }
        Image::new(region, self.channels, data)
    }

    /// The hash of the image content: region, channel count and pixel bits.
    ///
    /// The pixels' bytes go to the hasher in large batches, never 4 bytes
    /// at a time: BLAKE3 hashes a batch of many KiB with SIMD across its
    /// chunks, while one call per value cost about 17 ns each (some 430 ms
    /// for a 25 MP mosaic, most of building a raw's graph). The bytes fed
    /// are the same, so is the hash.
    pub fn content_hash(&self) -> ContentHash {
        /// Bytes gathered before each hasher update.
        const BATCH: usize = 1 << 18;
        let _span = crate::trace::span("image.hash", || {
            format!(
                "{}x{}x{}",
                self.region.width, self.region.height, self.channels
            )
        });
        let mut h = blake3::Hasher::new();
        h.update(b"helios.image.v1");
        h.update(&self.region.x.to_le_bytes());
        h.update(&self.region.y.to_le_bytes());
        h.update(&self.region.width.to_le_bytes());
        h.update(&self.region.height.to_le_bytes());
        h.update(&self.channels.to_le_bytes());
        let mut bytes: Vec<u8> = Vec::with_capacity(BATCH + self.row_len() * 4);
        for y in self.region.y..self.region.bottom() {
            bytes.extend(self.row(y).iter().flat_map(|v| v.to_bits().to_le_bytes()));
            if bytes.len() >= BATCH {
                h.update(&bytes);
                bytes.clear();
            }
        }
        h.update(&bytes);
        ContentHash(*h.finalize().as_bytes())
    }

    /// Whether two images share the same buffer in memory.
    pub fn same_buffer(&self, other: &Image) -> bool {
        Arc::ptr_eq(&self.buffer, &other.buffer)
    }
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image({}, {} channels)", self.region, self.channels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(region: Region) -> Image {
        Image::from_fn(region, 1, |x, y, _| (x + 10 * y) as f32)
    }

    #[test]
    fn tiles_cover_the_region_exactly() {
        let r = Region::new(-3, 2, 10, 7);
        let tiles = r.tiles(4);
        assert_eq!(tiles.len(), 3 * 2);
        assert_eq!(tiles.iter().map(Region::area).sum::<usize>(), r.area());
        assert!(tiles.iter().all(|t| r.contains(t)));
    }

    #[test]
    fn from_rows_in_bands_is_from_fn() {
        let region = Region::new(-3, 7, 517, 409);
        let f = |x: i64, y: i64, c: u32| (x * 31 + y * 17 + i64::from(c)) as f32 * 0.25;
        let rows = Image::from_rows(region, 2, |y, row| {
            for (i, px) in row.chunks_exact_mut(2).enumerate() {
                let x = region.x + i as i64;
                px[0] = f(x, y, 0);
                px[1] = f(x, y, 1);
            }
        })
        .unwrap();
        assert_eq!(rows.data(), Image::from_fn(region, 2, f).data());
        let cancelled = crate::cancel::Token::new();
        cancelled.cancel();
        let err = Image::from_rows_until(region, 2, &cancelled, |_, _| {}).unwrap_err();
        assert_eq!(err, crate::cancel::CANCELLED);
    }

    #[test]
    fn a_large_sample_past_the_edges_replicates_them_in_bands() {
        let img = Image::from_fn(Region::sized(600, 500), 1, |x, y, _| (x * 1000 + y) as f32);
        let region = Region::new(-4, -3, 610, 509);
        let out = img.sample(region);
        let want = Image::from_fn(region, 1, |x, y, _| {
            (x.clamp(0, 599) * 1000 + y.clamp(0, 499)) as f32
        });
        assert_eq!(out.data(), want.data());
        // Wholly outside, on either side.
        for region in [Region::new(-9, 2, 4, 3), Region::new(700, 2, 4, 3)] {
            let want = Image::from_fn(region, 1, |x, y, _| {
                (x.clamp(0, 599) * 1000 + y.clamp(0, 499)) as f32
            });
            assert_eq!(img.sample(region).data(), want.data(), "{region}");
        }
    }

    #[test]
    fn sample_inside_crops_and_outside_replicates_edges() {
        let img = ramp(Region::sized(4, 3));
        let inner = img.sample(Region::new(1, 1, 2, 1));
        assert_eq!(inner.data(), &[11.0, 12.0]);
        let outer = img.sample(Region::new(-1, -1, 2, 2));
        assert_eq!(outer.data(), &[0.0, 0.0, 0.0, 0.0]);
        let corner = img.sample(Region::new(3, 2, 2, 2));
        assert_eq!(corner.data(), &[23.0, 23.0, 23.0, 23.0]);
    }

    #[test]
    fn a_crop_is_a_view_that_reads_like_a_copy() {
        let img = ramp(Region::sized(5, 4));
        let part = Region::new(1, 1, 3, 2);
        let view = img.crop(part).unwrap();
        assert!(view.same_buffer(&img), "no pixel copied");
        assert_eq!(view.pixel(2, 2), img.pixel(2, 2));
        assert_eq!(view.row(1), &[11.0, 12.0, 13.0]);
        assert_eq!(view.data(), &[11.0, 12.0, 13.0, 21.0, 22.0, 23.0]);
        assert_eq!(view.to_vec(), view.data());
        let copy = Image::from_fn(part, 1, |x, y, _| (x + 10 * y) as f32);
        assert_eq!(view.content_hash(), copy.content_hash());
        // A crop of a crop, and full rows (contiguous: no packed copy).
        let rows = img.crop(Region::new(0, 1, 5, 2)).unwrap();
        assert_eq!(rows.data(), &img.data()[5..15]);
        assert_eq!(rows.bytes(), 10 * 4);
        let inner = view.crop(Region::new(2, 2, 2, 1)).unwrap();
        assert_eq!(inner.data(), &[22.0, 23.0]);
        assert!(img.crop(Region::new(4, 0, 2, 1)).is_none(), "past the edge");
    }

    #[test]
    fn full_resolution_covers_what_sample_at_reads() {
        assert_eq!(
            Region::new(1, 2, 3, 1).full_resolution(0.5),
            Region::new(2, 4, 6, 2)
        );
        assert_eq!(
            Region::new(0, 0, 2, 2).full_resolution(0.3),
            Region::new(0, 0, 6, 6)
        );
        let img = ramp(Region::new(-4, -4, 20, 20));
        let r = Region::new(-1, 0, 3, 2);
        let scale = 0.37;
        let full = r.full_resolution(scale);
        // Reading only the covered pixels gives the same mean.
        let part = img.sample(full);
        assert_eq!(
            part.sample_at(r, scale).data(),
            img.sample_at(r, scale).data()
        );
    }

    #[test]
    fn bounds_hold_both_regions() {
        let a = Region::new(0, 0, 4, 4);
        let b = Region::new(-2, 3, 3, 5);
        assert_eq!(a.bounds(&b), Region::new(-2, 0, 6, 8));
        assert!(a.bounds(&b).contains(&a) && a.bounds(&b).contains(&b));
    }

    #[test]
    fn overlapping_regions_share_a_pixel() {
        let a = Region::new(0, 0, 4, 4);
        assert!(a.overlaps(&Region::new(3, 3, 2, 2)));
        assert!(!a.overlaps(&Region::new(4, 0, 2, 2)), "edges touch only");
        assert!(!a.overlaps(&Region::new(-2, 1, 2, 1)));
        assert!(!a.overlaps(&Region::new(1, 1, 0, 3)), "an empty region");
        assert!(Region::new(-5, -5, 20, 20).overlaps(&a));
    }

    #[test]
    fn content_hash_in_batches_is_the_hash_value_by_value() {
        // Rows wider than one batch and a view off the origin: the bytes
        // fed in batches hash as the same bytes fed 4 at a time.
        let a = ramp(Region::new(-2, 3, 70_001, 3));
        let mut h = blake3::Hasher::new();
        h.update(b"helios.image.v1");
        let r = a.region();
        h.update(&r.x.to_le_bytes());
        h.update(&r.y.to_le_bytes());
        h.update(&r.width.to_le_bytes());
        h.update(&r.height.to_le_bytes());
        h.update(&1u32.to_le_bytes());
        for y in r.y..r.bottom() {
            for v in a.row(y) {
                h.update(&v.to_bits().to_le_bytes());
            }
        }
        assert_eq!(a.content_hash(), ContentHash(*h.finalize().as_bytes()));
    }

    #[test]
    fn content_hash_depends_on_pixels_and_region() {
        let a = ramp(Region::sized(4, 3));
        assert_eq!(a.content_hash(), ramp(Region::sized(4, 3)).content_hash());
        assert_ne!(
            a.content_hash(),
            ramp(Region::new(1, 0, 4, 3)).content_hash()
        );
        let b = Image::new(Region::sized(1, 1), 1, vec![0.0]);
        let c = Image::new(Region::sized(1, 1), 1, vec![-0.0]);
        assert_ne!(b.content_hash(), c.content_hash());
    }

    #[test]
    fn clones_share_the_buffer() {
        let a = ramp(Region::sized(4, 3));
        assert!(a.same_buffer(&a.clone()));
        assert!(a.same_buffer(&a.sample(a.region())));
    }
}
