// Ported from vkdt (https://github.com/hanatos/vkdt, commit afc3425):
// src/pipe/modules/shared.glsl (sample_soft, sample_semisoft) and the
// sampler vkdt creates in src/qvk/qvk.c (linear, mirrored repeat). vkdt:
// copyright 2019 johannes hanika, under the BSD-2-Clause licence; its notice
// is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

//! vkdt's texture samplers over an image of known size: bilinear lookups
//! with vkdt's mirrored repeat at the image's edges, and the soft kernels
//! of `shared.glsl` built on them (`sample_soft`, 3x3 lookups 1.5 pixels
//! apart; `sample_semisoft`, 2x2 lookups one pixel apart).
//!
//! vkdt samples in texture coordinates, a texel's centre at `i + 0.5`;
//! here positions are in pixels relative to the image's domain, a pixel's
//! centre at its integer index (vkdt's position minus a half). An image
//! here may cover more or less than its domain: the lookups mirror into
//! the domain first and read the image at the domain's absolute
//! coordinates, so the image must hold the pixels [`Domain::reads`] names.

use helios_core::{Image, Region};

/// The pixels an image stands for (its size in vkdt, where it starts in
/// Helios's plane): lookups mirror into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Domain {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl Domain {
    pub fn new(x: i64, y: i64, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
        }
    }

    /// The domain at half the resolution, as vkdt sizes it (odd sides
    /// round up), starting at the origin of its own plane.
    pub fn half(&self) -> Self {
        Self::new(0, 0, (self.width - 1) / 2 + 1, (self.height - 1) / 2 + 1)
    }

    /// The bounding box of the absolute pixels read for the relative
    /// columns `x0..=x1` and rows `y0..=y1`, mirrored into the domain.
    pub fn reads(&self, x0: i64, x1: i64, y0: i64, y1: i64) -> Region {
        let (a, b) = mirrored_span(x0, x1, self.width);
        let (c, d) = mirrored_span(y0, y1, self.height);
        Region::new(
            self.x + a,
            self.y + c,
            (b - a + 1) as u32,
            (d - c + 1) as u32,
        )
    }
}

/// Vulkan's mirrored repeat: index `i` folded into `0..n`.
pub fn mirror(i: i64, n: u32) -> i64 {
    let n = i64::from(n);
    let m = i.rem_euclid(2 * n);
    if m < n { m } else { 2 * n - 1 - m }
}

/// The smallest and largest of `mirror(i, n)` for `i` in `lo..=hi`.
fn mirrored_span(lo: i64, hi: i64, n: u32) -> (i64, i64) {
    if lo >= 0 && hi < i64::from(n) {
        return (lo, hi);
    }
    if hi - lo + 1 >= 2 * i64::from(n) {
        return (0, i64::from(n) - 1);
    }
    (lo..=hi).fold((i64::MAX, i64::MIN), |(a, b), i| {
        let m = mirror(i, n);
        (a.min(m), b.max(m))
    })
}

/// Channel `c` of `image` at relative pixel `(x, y)` of `domain`, mirrored.
pub fn texel(image: &Image, domain: &Domain, x: i64, y: i64, c: usize) -> f32 {
    image.pixel(
        domain.x + mirror(x, domain.width),
        domain.y + mirror(y, domain.height),
    )[c]
}

/// Bilinear at relative position `(px, py)` of `domain` (pixel centres at
/// whole numbers), mirrored at the edges: vkdt's texture lookup with its
/// linear sampler.
pub fn bilinear(image: &Image, domain: &Domain, px: f32, py: f32, c: usize) -> f32 {
    let (x0, y0) = (px.floor(), py.floor());
    let (fx, fy) = (px - x0, py - y0);
    let (ix, iy) = (x0 as i64, y0 as i64);
    let at = |x, y| texel(image, domain, x, y, c);
    let mix = |a: f32, b: f32, t: f32| a * (1.0 - t) + b * t;
    let top = mix(at(ix, iy), at(ix + 1, iy), fx);
    let bottom = mix(at(ix, iy + 1), at(ix + 1, iy + 1), fx);
    mix(top, bottom, fy)
}

/// vkdt's `sample_soft`: the mean of 3x3 bilinear lookups, 1.5 pixels
/// apart, around `(px, py)`.
pub fn sample_soft(image: &Image, domain: &Domain, px: f32, py: f32, c: usize) -> f32 {
    let mut sum = 0.0;
    for dy in [-1.5, 0.0, 1.5] {
        for dx in [-1.5, 0.0, 1.5] {
            sum += bilinear(image, domain, px + dx, py + dy, c);
        }
    }
    sum / 9.0
}

/// The relative pixels [`sample_soft`] reads around `p` on one axis.
pub fn soft_reach(p: f32) -> (i64, i64) {
    ((p - 1.5).floor() as i64, (p + 1.5).floor() as i64 + 1)
}

/// vkdt's `sample_semisoft`: the mean of 2x2 bilinear lookups half a pixel
/// away from `(px, py)` on each axis.
pub fn sample_semisoft(image: &Image, domain: &Domain, px: f32, py: f32, c: usize) -> f32 {
    let (x0, x1) = (px - 0.5, px + 0.5);
    let (y0, y1) = (py - 0.5, py + 0.5);
    (bilinear(image, domain, x0, y0, c)
        + bilinear(image, domain, x1, y0, c)
        + bilinear(image, domain, x0, y1, c)
        + bilinear(image, domain, x1, y1, c))
        / 4.0
}

/// The relative pixels [`sample_semisoft`] reads around `p` on one axis.
pub fn semisoft_reach(p: f32) -> (i64, i64) {
    ((p - 0.5).floor() as i64, (p + 0.5).floor() as i64 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(domain: Domain) -> Image {
        let r = Region::new(domain.x, domain.y, domain.width, domain.height);
        Image::from_fn(r, 1, |x, y, _| (x * 10 + y) as f32)
    }

    #[test]
    fn mirror_is_vulkans_mirrored_repeat() {
        let folded: Vec<i64> = (-5..9).map(|i| mirror(i, 3)).collect();
        assert_eq!(folded, [1, 2, 2, 1, 0, 0, 1, 2, 2, 1, 0, 0, 1, 2]);
        assert_eq!(mirror(0, 1), 0);
        assert_eq!(mirror(-7, 1), 0);
    }

    #[test]
    fn bilinear_reads_pixel_centres_and_mirrors_past_the_edge() {
        let d = Domain::new(5, -2, 4, 3);
        let img = ramp(d);
        assert_eq!(bilinear(&img, &d, 1.0, 2.0, 0), 60.0);
        assert_eq!(bilinear(&img, &d, 1.5, 0.0, 0), 63.0);
        // Half a pixel before the first column: the mirror repeats it.
        assert_eq!(bilinear(&img, &d, -0.5, 0.0, 0), 48.0);
        // One and a half before: between mirrored pixels 0 and 1.
        assert_eq!(bilinear(&img, &d, -1.5, 0.0, 0), 53.0);
        assert_eq!(bilinear(&img, &d, 3.5, 1.0, 0), 79.0);
    }

    #[test]
    fn the_soft_samplers_keep_a_constant_and_a_ramp_inside() {
        let d = Domain::new(0, 0, 12, 9);
        let flat = Image::from_fn(Region::sized(12, 9), 1, |_, _, _| 0.25);
        let img = ramp(d);
        for (px, py) in [(0.0, 0.0), (3.0, 4.5), (11.0, 8.0), (-2.0, 10.0)] {
            assert!((sample_soft(&flat, &d, px, py, 0) - 0.25).abs() < 1e-6);
            assert!((sample_semisoft(&flat, &d, px, py, 0) - 0.25).abs() < 1e-6);
        }
        // Away from the edges both are symmetric about their centre: a
        // ramp keeps its value there.
        for (px, py) in [(5.0, 4.0), (6.5, 3.5)] {
            let v = px * 10.0 + py;
            assert!((sample_soft(&img, &d, px, py, 0) - v).abs() < 1e-4);
            assert!((sample_semisoft(&img, &d, px, py, 0) - v).abs() < 1e-4);
        }
    }

    #[test]
    fn semisoft_is_a_1_2_1_filter_at_pixel_centres_and_soft_is_wider() {
        let d = Domain::new(0, 0, 9, 1);
        let spike = Image::from_fn(Region::sized(9, 1), 1, |x, _, _| f32::from(x == 4));
        let along = |f: fn(&Image, &Domain, f32, f32, usize) -> f32| {
            (0..9)
                .map(|x| f(&spike, &d, x as f32, 0.0, 0))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            along(sample_semisoft),
            [0.0, 0.0, 0.0, 0.25, 0.5, 0.25, 0.0, 0.0, 0.0]
        );
        let soft = along(sample_soft);
        // Lookups at -1.5, 0 and 1.5 split the outer ones half and half.
        let third = 1.0 / 3.0;
        let expect = [
            0.0,
            0.0,
            third / 2.0,
            third / 2.0,
            third,
            third / 2.0,
            third / 2.0,
            0.0,
            0.0,
        ];
        for (a, b) in soft.iter().zip(expect) {
            assert!((a - b).abs() < 1e-6, "{soft:?}");
        }
    }

    #[test]
    fn reads_name_every_pixel_the_samplers_touch() {
        let d = Domain::new(-16, 3, 10, 6);
        // Inside: the relative span, moved to the domain.
        assert_eq!(d.reads(2, 5, 1, 2), Region::new(-14, 4, 4, 2));
        // Past either edge: mirrored back, so the box stays in the domain.
        assert_eq!(d.reads(-3, 1, 4, 8), Region::new(-16, 6, 3, 3));
        assert_eq!(d.reads(-40, 40, 0, 0), Region::new(-16, 3, 10, 1));
        assert_eq!(soft_reach(2.0), (0, 4));
        assert_eq!(soft_reach(2.5), (1, 5));
        assert_eq!(semisoft_reach(4.0), (3, 5));
    }

    #[test]
    fn half_rounds_odd_sides_up_like_vkdt() {
        let d = Domain::new(7, 7, 13, 8).half();
        assert_eq!(d, Domain::new(0, 0, 7, 4));
        assert_eq!(Domain::new(0, 0, 1, 2).half(), Domain::new(0, 0, 1, 1));
    }
}
