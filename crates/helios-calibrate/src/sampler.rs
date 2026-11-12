//! The sampler: each patch's robust statistics and its checks, then the
//! chart-wide checks on the neutral patches.
//!
//! Per patch, on the pixels of its sampling area ([`Layout::pixels`]):
//!
//! - **value:** a trimmed mean per channel (the lowest and highest
//!   [`Thresholds::trim`] of the values dropped), so dust and hot pixels
//!   do not move it; the spread is the standard deviation of the values
//!   kept;
//! - **clipped:** the fraction of the area's photosites at or above the
//!   clip level, read on `rawprepare`'s output (where clipping is real:
//!   before white balance and demosaicing) or, for a picture, on its
//!   encoded values; above [`Thresholds::clipped_fraction`] the patch is
//!   excluded;
//! - **too dark:** a luminance below a fraction of the chart's white
//!   patch, or of the white level; a warning, kept;
//! - **not uniform:** a channel's coefficient of variation above
//!   [`Thresholds::variation`], or the area's two halves (left and right,
//!   top and bottom, in the chart's frame) apart by more than
//!   [`Thresholds::halves`] (glare, a shadow, a gradient of light, a
//!   misplaced corner); a warning, kept, excludable.
//!
//! Chart-wide, on the neutral patches: their luminance must decrease from
//! white to black (else the corners are swapped or the chart is not the
//! one chosen), their chromaticities must be close to each other (else the
//! light is mixed, or the patches read are not the neutral ones) and close
//! to neutral (a colour cast).

use crate::layout::{ImageView, Layout};

/// The clip level on `rawprepare`'s output (black at 0, the sensor's
/// white at 1) for `highlights`' `clip` parameter: the module cuts the
/// balanced photosites at `clip × 0.987` times the smallest white-balance
/// coefficient (green's, 1 once normalised), so a green photosite at or
/// above `clip × 0.987` before white balance is cut. Taking the same level
/// for every channel is the cautious reading: a red or blue photosite
/// there is at most a hair from the sensor's white.
pub fn raw_clip_level(clip: f64) -> f64 {
    clip * HIGHLIGHTS_CLIP_MAGIC
}

/// darktable's factor on `clip` in `highlights`' clip mode, the one
/// `helios-module-highlights` keeps.
const HIGHLIGHTS_CLIP_MAGIC: f64 = 0.987;

/// The clip level on `rawprepare`'s output with `highlights`' default
/// `clip` (1).
pub const RAW_CLIP_LEVEL: f64 = HIGHLIGHTS_CLIP_MAGIC;

/// The clip level of a picture (a TIFF, a JPEG), on its encoded values in
/// `[0, 1]`: any channel at or above it.
pub const PICTURE_CLIP_LEVEL: f64 = 0.995;

/// Every threshold of the sampler, in one place. [`THRESHOLDS`] are the
/// values Helios uses; the tests in this module (synthetic charts with
/// 1 % noise and hot pixels) show where each one fires. They are first
/// values, to be tuned on real shots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// The fraction of the values dropped at each end before the mean: 5 %
    /// drops a hot pixel or a speck of dust in an area of 20 pixels and
    /// more, and keeps 90 % of the data.
    pub trim: f64,
    /// A patch with more of its photosites clipped is excluded. 1 %: a hot
    /// photosite or two are tolerated (the trimmed mean drops them), a
    /// patch touching the white is not.
    pub clipped_fraction: f64,
    /// A patch whose luminance is under this fraction of the white
    /// patch's is too dark. 1 %: the chart's black is about 3.4 % of its
    /// white, so a chart lit evenly never fires; a patch in a shadow does.
    pub dark_relative: f64,
    /// A patch whose luminance is under this fraction of the white level
    /// (1 on `rawprepare`'s scale) is too dark: 0.2 %, where noise
    /// dominates whatever the chart's exposure.
    pub dark_absolute: f64,
    /// The largest coefficient of variation `σ/μ` of a channel of a
    /// uniform patch: 5 %, several times a well-exposed patch's noise.
    pub variation: f64,
    /// The largest relative difference between the trimmed means of an
    /// area's two halves: 3 %. A gradient of light across a patch
    /// barely moves its spread (a ±5 % ramp is a 2.9 % variation) but
    /// shows here.
    pub halves: f64,
    /// The largest distance (per component of `rgb / (r + g + b)`) between
    /// a neutral patch's chromaticity and the neutrals' mean: 0.02. The
    /// chart's neutrals stay within 0.005; a coloured patch read as a
    /// neutral one is 0.05 and more away.
    pub neutral_spread: f64,
    /// The largest distance (per component) between the neutrals' mean
    /// chromaticity and `1/3`: 0.03, a channel about 10 % above the
    /// others. Under the camera's reference white balance, a light far
    /// from it shows as a cast too: a warning for the report, the solver
    /// estimates the light from the neutrals.
    pub neutral_cast: f64,
}

/// The thresholds Helios uses.
pub const THRESHOLDS: Thresholds = Thresholds {
    trim: 0.05,
    clipped_fraction: 0.01,
    dark_relative: 0.01,
    dark_absolute: 0.002,
    variation: 0.05,
    halves: 0.03,
    neutral_spread: 0.02,
    neutral_cast: 0.03,
};

impl Default for Thresholds {
    fn default() -> Self {
        THRESHOLDS
    }
}

/// A patch's pixels summed up, per channel, robustly.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchStats {
    /// The pixels in the sampling area.
    pub count: usize,
    /// The values kept per channel once trimmed.
    pub kept: usize,
    /// The trimmed mean.
    pub mean: Vec<f64>,
    /// The standard deviation of the values kept (about 0.89 σ for
    /// Gaussian noise trimmed at 5 % each side).
    pub sigma: Vec<f64>,
    /// The larger relative difference between the trimmed means of the
    /// area's left and right halves, and of its top and bottom halves.
    pub halves: Vec<f64>,
}

impl PatchStats {
    /// The largest coefficient of variation of a channel.
    pub fn variation(&self) -> f64 {
        self.sigma
            .iter()
            .zip(&self.mean)
            .map(|(s, m)| s / m.abs().max(f64::MIN_POSITIVE))
            .fold(0.0, f64::max)
    }

    /// The largest difference between halves of a channel.
    pub fn halves_difference(&self) -> f64 {
        self.halves.iter().copied().fold(0.0, f64::max)
    }

    /// The mean of the channels' trimmed means: the luminance the darkness
    /// and order checks compare.
    pub fn luminance(&self) -> f64 {
        self.mean.iter().sum::<f64>() / self.mean.len().max(1) as f64
    }
}

/// The trimmed mean, the spread of the kept values and their count.
fn trimmed(values: &mut [f64], trim: f64) -> (f64, f64, usize) {
    values.sort_by(f64::total_cmp);
    let cut = (values.len() as f64 * trim).floor() as usize;
    let kept = &values[cut..values.len() - cut];
    let n = kept.len() as f64;
    let mean = kept.iter().sum::<f64>() / n;
    // Two passes: the spread of nearly uniform patches stays exact.
    let variance = kept.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    (mean, variance.sqrt(), kept.len())
}

/// Patch `k`'s robust statistics in `image`; `None` when no pixel of the
/// image falls in its sampling area.
pub fn patch_stats(
    layout: &Layout,
    image: &ImageView<'_>,
    k: usize,
    trim: f64,
) -> Option<PatchStats> {
    let pixels = layout.pixels(k, image.width, image.height);
    if pixels.is_empty() {
        return None;
    }
    let (i, j) = layout.chart.position(k);
    // Each pixel's side of the patch's centre, in the chart's frame.
    let sides: Vec<[bool; 2]> = pixels
        .iter()
        .map(|&(x, y)| {
            let [u, v] = layout.to_chart.apply([x as f64 + 0.5, y as f64 + 0.5]);
            [u < i as f64, v < j as f64]
        })
        .collect();
    let mut stats = PatchStats {
        count: pixels.len(),
        kept: 0,
        mean: Vec::with_capacity(image.channels),
        sigma: Vec::with_capacity(image.channels),
        halves: Vec::with_capacity(image.channels),
    };
    for c in 0..image.channels {
        let mut values: Vec<f64> = pixels
            .iter()
            .map(|&(x, y)| f64::from(image.pixel(x, y)[c]))
            .collect();
        let mut halves = 0.0_f64;
        for axis in 0..2 {
            let (mut a, mut b): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
            for (v, side) in values.iter().zip(&sides) {
                if side[axis] { a.push(*v) } else { b.push(*v) }
            }
            if a.is_empty() || b.is_empty() {
                continue;
            }
            let (ma, _, _) = trimmed(&mut a, trim);
            let (mb, _, _) = trimmed(&mut b, trim);
            let scale = (0.5 * (ma + mb)).abs().max(f64::MIN_POSITIVE);
            halves = halves.max((ma - mb).abs() / scale);
        }
        let (mean, sigma, kept) = trimmed(&mut values, trim);
        stats.kept = kept;
        stats.mean.push(mean);
        stats.sigma.push(sigma);
        stats.halves.push(halves);
    }
    Some(stats)
}

/// The fraction of patch `k`'s sampling area clipped in `image` (one
/// channel for `rawprepare`'s mosaic, or a picture's colour channels: a
/// pixel is clipped when any of its first three channels is at or above
/// `level`, a decoded picture's alpha, 1, left out); `None` when no pixel
/// of the image falls in the area.
pub fn clipped_fraction(
    layout: &Layout,
    image: &ImageView<'_>,
    k: usize,
    level: f64,
) -> Option<f64> {
    let pixels = layout.pixels(k, image.width, image.height);
    if pixels.is_empty() {
        return None;
    }
    let clipped = pixels
        .iter()
        .filter(|&&(x, y)| {
            image
                .pixel(x, y)
                .iter()
                .take(3)
                .any(|&v| f64::from(v) >= level)
        })
        .count();
    Some(clipped as f64 / pixels.len() as f64)
}

/// Where clipping is read: `rawprepare`'s output (or the picture itself)
/// at the layout's coordinates, and its clip level.
#[derive(Clone, Copy, Debug)]
pub struct ClipSource<'a> {
    pub image: ImageView<'a>,
    pub level: f64,
}

/// A patch measured and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchCheck {
    pub stats: PatchStats,
    /// The fraction of the area clipped; `None` when no clip source was
    /// given or it does not cover the area.
    pub clipped_fraction: Option<f64>,
    pub clipped: bool,
    pub dark: bool,
    pub not_uniform: bool,
}

impl PatchCheck {
    /// Whether the solver leaves the patch out: a clipped patch's value is
    /// wrong. A dark or non-uniform one is kept, with its warning.
    pub fn excluded(&self) -> bool {
        self.clipped
    }
}

/// What the sampler reports, patch by patch then chart-wide, in a stable
/// order: the patches by index, then the neutral checks.
#[derive(Clone, Debug, PartialEq)]
pub enum Warning {
    /// The patch's sampling area is outside the image.
    Missing(usize),
    Clipped(usize),
    Dark(usize),
    NotUniform(usize),
    /// The neutral patch `brighter` (lighter on the chart) does not
    /// measure lighter than `darker`, the next one.
    NeutralOrder {
        brighter: usize,
        darker: usize,
    },
    /// The neutral patches' chromaticities spread more than the
    /// threshold (the largest distance to their mean).
    NeutralSpread(f64),
    /// The neutrals' mean chromaticity is that far from neutral.
    NeutralCast(f64),
}

/// A chart measured and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartCheck {
    /// Each patch's check, `None` when its area is outside the image.
    pub patches: Vec<Option<PatchCheck>>,
    /// The pairs of consecutive neutrals (patch indices, lighter one on
    /// the chart first) not in decreasing luminance.
    pub neutral_order: Vec<(usize, usize)>,
    /// The largest distance of a neutral's chromaticity to the neutrals'
    /// mean; `None` with fewer than two unclipped neutrals or not three
    /// channels.
    pub neutral_spread: Option<f64>,
    /// The distance of the neutrals' mean chromaticity to neutral.
    pub neutral_cast: Option<f64>,
    pub thresholds: Thresholds,
}

impl ChartCheck {
    /// Every warning, in a stable order.
    pub fn warnings(&self) -> Vec<Warning> {
        let mut out = Vec::new();
        for (k, check) in self.patches.iter().enumerate() {
            let Some(check) = check else {
                out.push(Warning::Missing(k));
                continue;
            };
            if check.clipped {
                out.push(Warning::Clipped(k));
            }
            if check.dark {
                out.push(Warning::Dark(k));
            }
            if check.not_uniform {
                out.push(Warning::NotUniform(k));
            }
        }
        for &(brighter, darker) in &self.neutral_order {
            out.push(Warning::NeutralOrder { brighter, darker });
        }
        if let Some(spread) = self
            .neutral_spread
            .filter(|s| *s > self.thresholds.neutral_spread)
        {
            out.push(Warning::NeutralSpread(spread));
        }
        if let Some(cast) = self
            .neutral_cast
            .filter(|c| *c > self.thresholds.neutral_cast)
        {
            out.push(Warning::NeutralCast(cast));
        }
        out
    }
}

/// Measures and checks every patch of `layout` in `image` (the linear
/// camera RGB the solver reads), with clipping read on `clip` when given.
pub fn check_chart(
    layout: &Layout,
    image: &ImageView<'_>,
    clip: Option<&ClipSource<'_>>,
    thresholds: &Thresholds,
) -> ChartCheck {
    let chart = layout.chart;
    let stats: Vec<Option<PatchStats>> = (0..chart.len())
        .map(|k| patch_stats(layout, image, k, thresholds.trim))
        .collect();
    let white = chart
        .neutrals
        .first()
        .and_then(|&k| stats[k].as_ref())
        .map_or(0.0, PatchStats::luminance);
    let dark_below = thresholds
        .dark_absolute
        .max(thresholds.dark_relative * white);
    let patches: Vec<Option<PatchCheck>> = stats
        .into_iter()
        .enumerate()
        .map(|(k, stats)| {
            let stats = stats?;
            let fraction = clip.and_then(|c| clipped_fraction(layout, &c.image, k, c.level));
            Some(PatchCheck {
                clipped: fraction.is_some_and(|f| f > thresholds.clipped_fraction),
                clipped_fraction: fraction,
                dark: stats.luminance() < dark_below,
                not_uniform: stats.variation() > thresholds.variation
                    || stats.halves_difference() > thresholds.halves,
                stats,
            })
        })
        .collect();

    let neutral_order = chart
        .neutrals
        .windows(2)
        .filter_map(|pair| {
            let (a, b) = (patches[pair[0]].as_ref()?, patches[pair[1]].as_ref()?);
            (a.stats.luminance() <= b.stats.luminance()).then_some((pair[0], pair[1]))
        })
        .collect();

    let chromas: Vec<[f64; 3]> = chart
        .neutrals
        .iter()
        .filter_map(|&k| patches[k].as_ref())
        .filter(|p| !p.clipped && p.stats.mean.len() == 3)
        .filter_map(|p| {
            let m = &p.stats.mean;
            let sum = m[0] + m[1] + m[2];
            if sum > 0.0 {
                Some([m[0] / sum, m[1] / sum, m[2] / sum])
            } else {
                None
            }
        })
        .collect();
    let (neutral_spread, neutral_cast) = if chromas.len() < 2 {
        (None, None)
    } else {
        let n = chromas.len() as f64;
        let mut centre = [0.0; 3];
        for c in &chromas {
            for (s, v) in centre.iter_mut().zip(c) {
                *s += v / n;
            }
        }
        let spread = chromas
            .iter()
            .flat_map(|c| c.iter().zip(&centre).map(|(v, m)| (v - m).abs()))
            .fold(0.0, f64::max);
        let cast = centre
            .iter()
            .map(|m| (m - 1.0 / 3.0).abs())
            .fold(0.0, f64::max);
        (Some(spread), Some(cast))
    };

    ChartCheck {
        patches,
        neutral_order,
        neutral_spread,
        neutral_cast,
        thresholds: *thresholds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::XRITE_CC24_2014;
    use crate::lab::{D50, lab_to_xyz};
    use crate::layout::Point;

    const W: usize = 640;
    const H: usize = 440;
    const CORNERS: [Point; 4] = [[95.0, 88.0], [548.0, 70.0], [560.0, 352.0], [80.0, 340.0]];
    /// The mosaic's exposure: the white patch at about 0.55 of the white.
    const EXPOSURE: f64 = 0.6;

    /// A deterministic value in `[-1, 1]` per pixel and channel.
    fn noise(x: usize, y: usize, c: usize) -> f64 {
        let mut h = (x as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
            .wrapping_add(c as u64 * 0x1656_67B1_9E37_79F9);
        h ^= h >> 31;
        h = h.wrapping_mul(0xD6E8_FEB8_6659_FD93);
        h ^= h >> 32;
        (h % 20_001) as f64 / 10_000.0 - 1.0
    }

    /// A hot pixel every so many: one in about 500.
    fn hot(x: usize, y: usize) -> bool {
        (x * 7 + y * 13).is_multiple_of(499)
    }

    /// The synthetic chart, drawn where the layout `placed` says, in
    /// camera RGB (each patch's reference XYZ over the D50 white, so the
    /// neutrals are neutral), with ±2 % noise (a 1.2 % variation) and hot
    /// pixels. `shade(k, du, dv)` scales patch `k` at `(du, dv)` pitches
    /// from its centre. Returns the RGB image and the `rawprepare` mosaic
    /// (one channel, the mean of the RGB times [`EXPOSURE`], clamped at
    /// the white, its own hot photosites at the white).
    fn draw(placed: &Layout, shade: impl Fn(usize, f64, f64) -> f64) -> (Vec<f32>, Vec<f32>) {
        let chart = placed.chart;
        let half = 0.5 * chart.patch_side;
        let mut rgb = vec![0.0_f32; W * H * 3];
        let mut mosaic = vec![0.0_f32; W * H];
        for y in 0..H {
            for x in 0..W {
                let [u, v] = placed.to_chart.apply([x as f64 + 0.5, y as f64 + 0.5]);
                let (i, j) = (u.round(), v.round());
                if i < 0.0 || j < 0.0 || i >= chart.columns as f64 || j >= chart.rows as f64 {
                    continue;
                }
                if (u - i).abs() > half || (v - j).abs() > half {
                    continue;
                }
                let k = chart.index(i as usize, j as usize);
                let xyz = lab_to_xyz(chart.patches[k].lab);
                let s = shade(k, u - i, v - j);
                let at = y * W + x;
                let mut sum = 0.0;
                for c in 0..3 {
                    let value = xyz[c] / D50[c] * s * (1.0 + 0.02 * noise(x, y, c));
                    sum += value;
                    rgb[at * 3 + c] = if hot(x, y) { 1.0 } else { value as f32 };
                }
                mosaic[at] = if hot(x + 3, y) {
                    1.0
                } else {
                    (sum / 3.0 * EXPOSURE).min(1.0) as f32
                };
            }
        }
        (rgb, mosaic)
    }

    fn check(placed: &Layout, read: &Layout, shade: impl Fn(usize, f64, f64) -> f64) -> ChartCheck {
        let (rgb, mosaic) = draw(placed, shade);
        let image = ImageView {
            data: &rgb,
            width: W,
            height: H,
            channels: 3,
        };
        let clip = ClipSource {
            image: ImageView {
                data: &mosaic,
                width: W,
                height: H,
                channels: 1,
            },
            level: RAW_CLIP_LEVEL,
        };
        check_chart(read, &image, Some(&clip), &THRESHOLDS)
    }

    fn layout() -> Layout {
        Layout::new(&XRITE_CC24_2014, CORNERS).unwrap()
    }

    #[test]
    fn a_clean_chart_passes_every_check() {
        let layout = layout();
        let result = check(&layout, &layout, |_, _, _| 1.0);
        assert_eq!(result.warnings(), []);
        for (k, patch) in XRITE_CC24_2014.patches.iter().enumerate() {
            let check = result.patches[k].as_ref().unwrap();
            assert!(check.stats.count > 300, "{}", patch.name);
            assert!(!check.excluded());
            // The hot pixels are trimmed away: the mean stays within the
            // noise's reach of the value (a plain mean moves by 0.2 % of
            // the white per hot pixel in 500).
            let xyz = lab_to_xyz(patch.lab);
            for c in 0..3 {
                let value = xyz[c] / D50[c];
                let error = (check.stats.mean[c] - value).abs() / value;
                assert!(error < 0.005, "{} {c}: {error}", patch.name);
            }
            assert!(check.stats.variation() < 0.015, "{}", patch.name);
            assert!(check.stats.halves_difference() < 0.01, "{}", patch.name);
            // A hot photosite or two clip; under the threshold.
            assert!(check.clipped_fraction.unwrap() < THRESHOLDS.clipped_fraction);
        }
        assert!(result.neutral_spread.unwrap() < 0.01);
        assert!(result.neutral_cast.unwrap() < 0.01);
    }

    #[test]
    fn a_clipped_patch_is_flagged_and_excluded() {
        let layout = layout();
        // The white patch twice as bright: past the white on the mosaic.
        let result = check(&layout, &layout, |k, _, _| if k == 18 { 2.0 } else { 1.0 });
        assert_eq!(result.warnings(), [Warning::Clipped(18)]);
        assert!(result.patches[18].as_ref().unwrap().excluded());
        // Partly clipped: the patch's right half only.
        let result = check(&layout, &layout, |k, du, _| {
            if k == 18 && du > 0.0 { 1.9 } else { 1.0 }
        });
        assert!(result.warnings().contains(&Warning::Clipped(18)));
    }

    #[test]
    fn a_dark_patch_is_flagged_and_kept() {
        let layout = layout();
        let result = check(&layout, &layout, |k, _, _| if k == 1 { 0.005 } else { 1.0 });
        assert_eq!(result.warnings(), [Warning::Dark(1)]);
        assert!(!result.patches[1].as_ref().unwrap().excluded());
    }

    #[test]
    fn a_gradient_across_a_patch_shows_in_its_halves() {
        let layout = layout();
        // ±5 % across the sampled area, left to right.
        let ramp = 0.05 / (0.5 * crate::layout::SAMPLED_FRACTION * XRITE_CC24_2014.patch_side);
        let result = check(
            &layout,
            &layout,
            |k, du, _| if k == 7 { 1.0 + ramp * du } else { 1.0 },
        );
        assert_eq!(result.warnings(), [Warning::NotUniform(7)]);
        let stats = &result.patches[7].as_ref().unwrap().stats;
        // The spread alone would not have caught it.
        assert!(
            stats.variation() < THRESHOLDS.variation,
            "{}",
            stats.variation()
        );
        assert!(stats.halves_difference() > THRESHOLDS.halves);
        // The same ramp top to bottom.
        let result = check(
            &layout,
            &layout,
            |k, _, dv| if k == 7 { 1.0 + ramp * dv } else { 1.0 },
        );
        assert_eq!(result.warnings(), [Warning::NotUniform(7)]);
    }

    #[test]
    fn a_misplaced_corner_makes_patches_not_uniform() {
        let placed = layout();
        let pitch = (CORNERS[1][0] - CORNERS[0][0]) / 5.0;
        let mut moved = CORNERS;
        moved[2][0] += 0.5 * pitch;
        let read = Layout::new(&XRITE_CC24_2014, moved).unwrap();
        let result = check(&placed, &read, |_, _, _| 1.0);
        // The bottom-right patch's area straddles the gutter.
        assert!(result.warnings().contains(&Warning::NotUniform(23)));
        // The top-left one has not moved.
        assert!(!result.patches[0].as_ref().unwrap().not_uniform);
    }

    #[test]
    fn swapped_corners_break_the_neutral_order() {
        let placed = layout();
        // Left and right swapped: the neutral row reads black to white.
        let mirrored = [CORNERS[1], CORNERS[0], CORNERS[3], CORNERS[2]];
        let read = Layout::new(&XRITE_CC24_2014, mirrored).unwrap();
        let result = check(&placed, &read, |_, _, _| 1.0);
        let order: Vec<Warning> = (18..23)
            .map(|k| Warning::NeutralOrder {
                brighter: k,
                darker: k + 1,
            })
            .collect();
        assert_eq!(result.warnings(), order);
    }

    #[test]
    fn a_chart_upside_down_reads_coloured_neutrals() {
        let placed = layout();
        // Top and bottom swapped: the neutral row reads the first row.
        let flipped = [CORNERS[3], CORNERS[2], CORNERS[1], CORNERS[0]];
        let read = Layout::new(&XRITE_CC24_2014, flipped).unwrap();
        let result = check(&placed, &read, |_, _, _| 1.0);
        let warnings = result.warnings();
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, Warning::NeutralSpread(_))),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, Warning::NeutralOrder { .. })),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().all(|w| matches!(
                w,
                Warning::NeutralSpread(_) | Warning::NeutralOrder { .. } | Warning::NeutralCast(_)
            )),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_colour_cast_on_the_chart_is_reported() {
        let layout = layout();
        let (rgb, _) = draw(&layout, |_, _, _| 1.0);
        // Red up by 25 % everywhere: the neutrals stay together, off grey.
        let cast: Vec<f32> = rgb
            .chunks(3)
            .flat_map(|p| [p[0] * 1.25, p[1], p[2]])
            .collect();
        let image = ImageView {
            data: &cast,
            width: W,
            height: H,
            channels: 3,
        };
        let result = check_chart(&layout, &image, None, &THRESHOLDS);
        let warnings = result.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(matches!(warnings[0], Warning::NeutralCast(c) if c > 0.04));
    }

    #[test]
    fn trimming_drops_the_extremes() {
        let mut values: Vec<f64> = (0..20).map(f64::from).collect();
        values[3] = 1000.0;
        let (mean, _, kept) = trimmed(&mut values, 0.05);
        assert_eq!(kept, 18);
        // 0 and 1000 dropped: the mean of 1..=19 without 3.
        assert!((mean - (190.0 - 3.0) / 18.0).abs() < 1e-12);
        let (_, sigma, kept) = trimmed(&mut [2.0, 2.0, 2.0], 0.05);
        assert_eq!((sigma, kept), (0.0, 3));
    }
}
