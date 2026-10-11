//! Image buffers and regions.
//!
//! An [`Image`] is an immutable buffer of interleaved `f32` channels covering a
//! [`Region`] of an image plane. It is shared by reference (cloning is cheap),
//! so the same pixels are held once in memory whoever uses them.

use std::fmt;
use std::sync::Arc;

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

    pub fn right(&self) -> i64 {
        self.x + i64::from(self.width)
    }

    pub fn bottom(&self) -> i64 {
        self.y + i64::from(self.height)
    }

    pub fn area(&self) -> usize {
        self.width as usize * self.height as usize
    }

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

    pub fn contains(&self, other: &Region) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
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

/// An immutable image buffer: interleaved `f32` channels over a region.
#[derive(Clone)]
pub struct Image {
    region: Region,
    channels: u32,
    data: Arc<[f32]>,
}

impl Image {
    /// Wraps pixel data; `data` holds `region.area() * channels` values, row by row.
    pub fn new(region: Region, channels: u32, data: Vec<f32>) -> Self {
        assert!(channels > 0, "an image has at least one channel");
        assert_eq!(
            data.len(),
            region.area() * channels as usize,
            "pixel data does not match {region} x {channels}"
        );
        Self {
            region,
            channels,
            data: data.into(),
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

    /// What a module gets for an optional input left unconnected: no
    /// pixels, an empty region at the origin.
    pub fn absent(channels: u32) -> Self {
        Self::new(Region::sized(0, 0), channels, Vec::new())
    }

    /// Whether this is an optional input left unconnected (no pixels).
    pub fn is_absent(&self) -> bool {
        self.region.is_empty()
    }

    pub fn region(&self) -> Region {
        self.region
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// The channels of the pixel at absolute coordinates, which must lie in the region.
    pub fn pixel(&self, x: i64, y: i64) -> &[f32] {
        let r = self.region;
        assert!(
            x >= r.x && x < r.right() && y >= r.y && y < r.bottom(),
            "({x}, {y}) outside {r}"
        );
        let i =
            ((y - r.y) as usize * r.width as usize + (x - r.x) as usize) * self.channels as usize;
        &self.data[i..i + self.channels as usize]
    }

    /// The pixels of `region`, replicating the edge pixels where it extends past
    /// this image (the framework's border policy for neighbourhood reads).
    pub fn sample(&self, region: Region) -> Image {
        if region == self.region {
            return self.clone();
        }
        let r = self.region;
        assert!(!r.is_empty(), "cannot sample an empty image");
        Image::from_fn(region, self.channels, |x, y, c| {
            let cx = x.clamp(r.x, r.right() - 1);
            let cy = y.clamp(r.y, r.bottom() - 1);
            self.pixel(cx, cy)[c as usize]
        })
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
        let span = |v: i64| {
            let lo = (v as f64 / scale).floor() as i64;
            let hi = (((v + 1) as f64 / scale).floor() as i64).max(lo + 1);
            (lo, hi)
        };
        let channels = self.channels as usize;
        let mut data = Vec::with_capacity(region.area() * channels);
        let mut sum = vec![0.0f64; channels];
        for y in region.y..region.bottom() {
            let (y0, y1) = span(y);
            for x in region.x..region.right() {
                let (x0, x1) = span(x);
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
    pub fn content_hash(&self) -> ContentHash {
        let mut h = blake3::Hasher::new();
        h.update(b"helios.image.v1");
        h.update(&self.region.x.to_le_bytes());
        h.update(&self.region.y.to_le_bytes());
        h.update(&self.region.width.to_le_bytes());
        h.update(&self.region.height.to_le_bytes());
        h.update(&self.channels.to_le_bytes());
        for v in self.data.iter() {
            h.update(&v.to_bits().to_le_bytes());
        }
        ContentHash(*h.finalize().as_bytes())
    }

    /// Whether two images share the same buffer in memory.
    pub fn same_buffer(&self, other: &Image) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
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
