//! A chart placed in an image: the four handles sit on the centres of the
//! four corner patches, a homography maps the chart's coordinates (in
//! patch pitches) to the image's pixels, and each patch is sampled on the
//! central part of its square, mapped through it (a quadrilateral in the
//! image).
//!
//! Pixel `(x, y)` covers `[x, x + 1) × [y, y + 1)`: it belongs to a patch's
//! sampling area when its centre `(x + 0.5, y + 0.5)` does.

use crate::chart::Chart;
use crate::homography::Homography;

/// A point in the image, in pixels.
pub type Point = [f64; 2];

/// The part of a patch's side that is sampled, around its centre: the
/// edges of a patch blur into the gutter and a handle placed a little off
/// should not reach them.
pub const SAMPLED_FRACTION: f64 = 0.6;

/// An image's pixels as the sampler reads them: rows of `width` pixels of
/// `channels` interleaved values.
#[derive(Clone, Copy, Debug)]
pub struct ImageView<'a> {
    pub data: &'a [f32],
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

impl ImageView<'_> {
    fn pixel(&self, x: usize, y: usize) -> &[f32] {
        let at = (y * self.width + x) * self.channels;
        &self.data[at..at + self.channels]
    }
}

/// A patch's pixels summed up: per channel, the plain mean and the
/// population variance. The sampler's robust statistics and its checks
/// (trimmed mean, clipping, uniformity) build on these in a later slice.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchStats {
    pub count: usize,
    pub mean: Vec<f64>,
    pub variance: Vec<f64>,
}

/// A chart placed in an image by its four corner patches.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub chart: &'static Chart,
    /// The four handles, in the chart's corner order
    /// ([`Chart::corner_centres`]).
    pub corners: [Point; 4],
    /// Chart coordinates to image pixels.
    pub to_image: Homography,
    /// Image pixels to chart coordinates.
    pub to_chart: Homography,
}

impl Layout {
    /// The chart whose corner patches' centres are at `corners`, in the
    /// chart's corner order; `None` when the corners are degenerate (three
    /// on a line, or repeated).
    pub fn new(chart: &'static Chart, corners: [Point; 4]) -> Option<Self> {
        let to_image = Homography::from_points(chart.corner_centres(), corners)?;
        let to_chart = to_image.inverse()?;
        Some(Self {
            chart,
            corners,
            to_image,
            to_chart,
        })
    }

    /// Patch `k`'s centre in the image.
    pub fn centre(&self, k: usize) -> Point {
        let (i, j) = self.chart.position(k);
        self.to_image.apply([i as f64, j as f64])
    }

    /// Half the side of the sampled square, in pitches.
    fn half(&self, fraction: f64) -> f64 {
        0.5 * fraction * self.chart.patch_side
    }

    /// The image quadrilateral of patch `k`'s central square, `fraction`
    /// of its side (1 for the whole patch, [`SAMPLED_FRACTION`] for the
    /// sampled part): top-left, top-right, bottom-right, bottom-left in
    /// the chart's frame.
    pub fn quad(&self, k: usize, fraction: f64) -> [Point; 4] {
        let (i, j) = self.chart.position(k);
        let (ci, cj, h) = (i as f64, j as f64, self.half(fraction));
        [
            [ci - h, cj - h],
            [ci + h, cj - h],
            [ci + h, cj + h],
            [ci - h, cj + h],
        ]
        .map(|p| self.to_image.apply(p))
    }

    /// Patch `k`'s sampling quadrilateral.
    pub fn sampling_quad(&self, k: usize) -> [Point; 4] {
        self.quad(k, SAMPLED_FRACTION)
    }

    /// Whether every sampling quadrilateral lies inside a `width` ×
    /// `height` image.
    pub fn fits(&self, width: usize, height: usize) -> bool {
        (0..self.chart.len())
            .flat_map(|k| self.sampling_quad(k))
            .all(|[x, y]| x >= 0.0 && y >= 0.0 && x <= width as f64 && y <= height as f64)
    }

    /// The pixels of a `width` × `height` image inside patch `k`'s
    /// sampling area, row by row.
    pub fn pixels(&self, k: usize, width: usize, height: usize) -> Vec<(usize, usize)> {
        let quad = self.sampling_quad(k);
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for [x, y] in quad {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return Vec::new();
        }
        let clamp = |v: f64, n: usize| v.clamp(0.0, n as f64) as usize;
        let (x0, x1) = (clamp(x0.floor(), width), clamp(x1.ceil(), width));
        let (y0, y1) = (clamp(y0.floor(), height), clamp(y1.ceil(), height));
        let (i, j) = self.chart.position(k);
        let (ci, cj, h) = (i as f64, j as f64, self.half(SAMPLED_FRACTION));
        let mut out = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let [u, v] = self.to_chart.apply([x as f64 + 0.5, y as f64 + 0.5]);
                if (u - ci).abs() <= h && (v - cj).abs() <= h {
                    out.push((x, y));
                }
            }
        }
        out
    }

    /// Patch `k`'s mean and variance per channel in `image`; `None` when
    /// no pixel of the image falls in its sampling area.
    pub fn measure(&self, image: &ImageView<'_>, k: usize) -> Option<PatchStats> {
        let pixels = self.pixels(k, image.width, image.height);
        if pixels.is_empty() {
            return None;
        }
        let n = pixels.len() as f64;
        let mut mean = vec![0.0; image.channels];
        for &(x, y) in &pixels {
            for (m, &v) in mean.iter_mut().zip(image.pixel(x, y)) {
                *m += f64::from(v);
            }
        }
        for m in &mut mean {
            *m /= n;
        }
        // Two passes: the variance of nearly uniform patches stays exact.
        let mut variance = vec![0.0; image.channels];
        for &(x, y) in &pixels {
            for ((s, &v), m) in variance.iter_mut().zip(image.pixel(x, y)).zip(&mean) {
                *s += (f64::from(v) - m).powi(2);
            }
        }
        for s in &mut variance {
            *s /= n;
        }
        Some(PatchStats {
            count: pixels.len(),
            mean,
            variance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::XRITE_CC24_2014;
    use crate::lab::lab_to_xyz;

    const W: usize = 640;
    const H: usize = 440;
    /// The corner patches' centres of a chart shot in perspective and
    /// slightly rotated.
    const CORNERS: [Point; 4] = [[95.0, 88.0], [548.0, 70.0], [560.0, 352.0], [80.0, 340.0]];

    /// The synthetic chart: each pixel whose centre falls on a patch has
    /// that patch's reference XYZ, the gutters black.
    fn draw(layout: &Layout) -> Vec<f32> {
        let chart = layout.chart;
        let half = 0.5 * chart.patch_side;
        let mut data = vec![0.0_f32; W * H * 3];
        for y in 0..H {
            for x in 0..W {
                let [u, v] = layout.to_chart.apply([x as f64 + 0.5, y as f64 + 0.5]);
                let (i, j) = (u.round(), v.round());
                if i < 0.0 || j < 0.0 || i >= chart.columns as f64 || j >= chart.rows as f64 {
                    continue;
                }
                if (u - i).abs() > half || (v - j).abs() > half {
                    continue;
                }
                let k = chart.index(i as usize, j as usize);
                let xyz = lab_to_xyz(chart.patches[k].lab);
                let at = (y * W + x) * 3;
                for c in 0..3 {
                    data[at + c] = xyz[c] as f32;
                }
            }
        }
        data
    }

    #[test]
    fn the_handles_are_the_corner_patches_centres() {
        let layout = Layout::new(&XRITE_CC24_2014, CORNERS).unwrap();
        for (k, corner) in [0, 5, 23, 18].into_iter().zip(CORNERS) {
            let c = layout.centre(k);
            assert!(
                (c[0] - corner[0]).abs() < 1e-9 && (c[1] - corner[1]).abs() < 1e-9,
                "{k}"
            );
        }
        assert!(layout.fits(W, H));
        assert!(!layout.fits(400, 300));
    }

    #[test]
    fn the_sampling_quad_is_centred_and_inside_the_patch() {
        let layout = Layout::new(&XRITE_CC24_2014, CORNERS).unwrap();
        for k in 0..XRITE_CC24_2014.len() {
            let sampled = layout.sampling_quad(k);
            let whole = layout.quad(k, 1.0);
            // Each sampled corner maps back strictly inside the patch.
            let (i, j) = XRITE_CC24_2014.position(k);
            for p in sampled {
                let [u, v] = layout.to_chart.apply(p);
                let h = 0.5 * SAMPLED_FRACTION * XRITE_CC24_2014.patch_side;
                assert!(((u - i as f64).abs() - h).abs() < 1e-9);
                assert!(((v - j as f64).abs() - h).abs() < 1e-9);
            }
            // The quads' diagonals cross at the centre, as projections
            // keep them: the centre is inside both.
            let c = layout.centre(k);
            for q in [sampled, whole] {
                assert!(c[0] > q[0][0].min(q[3][0]) && c[0] < q[1][0].max(q[2][0]));
                assert!(c[1] > q[0][1].min(q[1][1]) && c[1] < q[2][1].max(q[3][1]));
            }
        }
    }

    #[test]
    fn a_chart_in_perspective_samples_back_to_its_patch_values() {
        let layout = Layout::new(&XRITE_CC24_2014, CORNERS).unwrap();
        let data = draw(&layout);
        let image = ImageView {
            data: &data,
            width: W,
            height: H,
            channels: 3,
        };
        for (k, patch) in XRITE_CC24_2014.patches.iter().enumerate() {
            let stats = layout.measure(&image, k).unwrap();
            assert!(stats.count > 300, "{}: {} pixels", patch.name, stats.count);
            let xyz = lab_to_xyz(patch.lab);
            for (c, v) in xyz.into_iter().enumerate() {
                // The image holds f32: the mean equals the value to f32's
                // precision, and the patch is uniform.
                assert!((stats.mean[c] - v).abs() < 1e-6, "{} {c}", patch.name);
                assert!(stats.variance[c] < 1e-12, "{} {c}", patch.name);
            }
        }
    }

    #[test]
    fn a_misplaced_corner_puts_patches_across_an_edge() {
        let placed = Layout::new(&XRITE_CC24_2014, CORNERS).unwrap();
        let data = draw(&placed);
        let image = ImageView {
            data: &data,
            width: W,
            height: H,
            channels: 3,
        };
        // The handles moved by half a pitch to the right: every sampled
        // area straddles a gutter or two patches.
        let pitch = (CORNERS[1][0] - CORNERS[0][0]) / 5.0;
        let shifted = CORNERS.map(|[x, y]| [x + 0.5 * pitch, y]);
        let misplaced = Layout::new(&XRITE_CC24_2014, shifted).unwrap();
        let mut straddling = 0;
        for k in 0..XRITE_CC24_2014.len() {
            let good = placed.measure(&image, k).unwrap();
            let Some(bad) = misplaced.measure(&image, k) else {
                continue;
            };
            let worst = |s: &PatchStats| {
                (0..3)
                    .map(|c| s.variance[c].sqrt() / s.mean[c].abs().max(1e-6))
                    .fold(0.0, f64::max)
            };
            assert!(worst(&good) < 1e-6);
            if worst(&bad) > 0.05 {
                straddling += 1;
            }
        }
        assert_eq!(straddling, XRITE_CC24_2014.len());
    }

    #[test]
    fn degenerate_corners_are_refused_and_an_area_off_the_image_is_empty() {
        let line = [[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0]];
        assert!(Layout::new(&XRITE_CC24_2014, line).is_none());
        let far = CORNERS.map(|[x, y]| [x + 5000.0, y]);
        let layout = Layout::new(&XRITE_CC24_2014, far).unwrap();
        let data = vec![0.0_f32; W * H * 3];
        let image = ImageView {
            data: &data,
            width: W,
            height: H,
            channels: 3,
        };
        assert_eq!(layout.measure(&image, 0), None);
    }
}
