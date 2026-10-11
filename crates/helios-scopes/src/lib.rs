//! Scopes: what an image holds, measured. The GUI draws them; this crate
//! only counts, so the counts are tested alone and could run anywhere (a
//! GPU variant later, as for modules).
//!
//! They read the working data (linear Rec.2020, before the display
//! transform), where clipping and the exposure of a scene are what they
//! are, not what a display curve makes of them.
//!
//! - [`histogram`]: per channel, on a linear or a log2 (stops) scale; what
//!   falls below or above the scale is counted apart, so clipping shows;
//! - [`waveform`]: for each column band of the image, how its luminance is
//!   spread over the levels;
//! - [`parade`]: the same per channel, red, green and blue side by side.

use helios_core::Image;

/// Rec.2020 luminance weights.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// How values map to bins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    /// From 0 to `max`.
    Linear { max: f32 },
    /// From `min` to `max` stops around 1.0 (log2 of the value).
    Stops { min: f32, max: f32 },
}

impl Default for Scale {
    fn default() -> Self {
        Scale::Stops {
            min: -12.0,
            max: 4.0,
        }
    }
}

/// Where a value falls on a scale with `bins` bins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bin {
    Below,
    At(usize),
    Above,
}

impl Scale {
    pub fn bin(self, v: f32, bins: usize) -> Bin {
        let t = match self {
            Scale::Linear { max } => v / max,
            Scale::Stops { min, max } => {
                if v <= 0.0 {
                    return Bin::Below;
                }
                (v.log2() - min) / (max - min)
            }
        };
        if t.is_nan() || t < 0.0 {
            Bin::Below
        } else if t > 1.0 {
            Bin::Above
        } else {
            Bin::At(((t * bins as f32) as usize).min(bins - 1))
        }
    }
}

/// Counts per bin, and what fell outside the scale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub bins: Vec<u32>,
    pub below: u32,
    pub above: u32,
}

impl Counts {
    fn new(bins: usize) -> Self {
        Counts {
            bins: vec![0; bins],
            below: 0,
            above: 0,
        }
    }

    fn add(&mut self, bin: Bin) {
        match bin {
            Bin::Below => self.below += 1,
            Bin::At(i) => self.bins[i] += 1,
            Bin::Above => self.above += 1,
        }
    }

    pub fn peak(&self) -> u32 {
        self.bins.iter().copied().max().unwrap_or(0)
    }
}

/// A histogram of the red, green and blue channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub scale: Scale,
    pub channels: [Counts; 3],
}

pub fn histogram(image: &Image, bins: usize, scale: Scale) -> Histogram {
    let mut channels = [Counts::new(bins), Counts::new(bins), Counts::new(bins)];
    let n = image.channels() as usize;
    for pixel in image.data().chunks_exact(n) {
        for (c, counts) in channels.iter_mut().enumerate() {
            counts.add(scale.bin(pixel[c.min(n - 1)], bins));
        }
    }
    Histogram { scale, channels }
}

/// For each column band, the counts of its levels: `columns[x].bins[level]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Waveform {
    pub scale: Scale,
    pub columns: Vec<Counts>,
}

fn columns_of(
    image: &Image,
    columns: usize,
    levels: usize,
    scale: Scale,
    value: impl Fn(&[f32]) -> f32,
) -> Waveform {
    let r = image.region();
    let n = image.channels() as usize;
    let width = r.width.max(1) as usize;
    let mut out = vec![Counts::new(levels); columns];
    for (i, pixel) in image.data().chunks_exact(n).enumerate() {
        let x = i % width;
        out[x * columns / width].add(scale.bin(value(pixel), levels));
    }
    Waveform {
        scale,
        columns: out,
    }
}

/// Luminance over `columns` bands and `levels` levels.
pub fn waveform(image: &Image, columns: usize, levels: usize, scale: Scale) -> Waveform {
    let n = image.channels() as usize;
    columns_of(image, columns, levels, scale, |p| {
        if n < 3 {
            p[0]
        } else {
            LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2]
        }
    })
}

/// Red, green and blue waveforms, each over `columns` bands.
pub fn parade(image: &Image, columns: usize, levels: usize, scale: Scale) -> [Waveform; 3] {
    let n = image.channels() as usize;
    [0, 1, 2].map(|c| columns_of(image, columns, levels, scale, |p| p[c.min(n - 1)]))
}
