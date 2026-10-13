//! How far two images are: the one comparison the parity jobs share
//! (`raw-corpus`'s thumbnails, `mask-parity`'s masked renders, the parity
//! tests against darktable).
//!
//! 1. A fast path: when the decoded pixels are identical, bit for bit,
//!    nothing else is computed.
//! 2. Otherwise each pixel's first three channels are read as linear
//!    Rec.2020 and compared in CIE Lab with Delta E 2000, and the
//!    differences are summed up together: their mean, their 99.9th and
//!    99.99th percentiles and their max.
//!
//! Each job judges the result with its own [`Thresholds`]: all of them
//! must hold, so a mean hides no local fault and a few outliers do not
//! fail a render that matches everywhere else.

use crate::color::{REC2020_TO_XYZ, apply};

/// Linear Rec.2020 to CIE Lab (D65 white).
pub fn lab(rgb: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = apply(&REC2020_TO_XYZ, rgb);
    let white = apply(&REC2020_TO_XYZ, [1.0, 1.0, 1.0]);
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x / white[0]), f(y / white[1]), f(z / white[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE Delta E 2000 between two Lab colours.
pub fn delta_e_2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let [l1, a1, b1] = a;
    let [l2, a2, b2] = b;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cm = f64::midpoint(c1, c2);
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).rem_euclid(2.0 * PI)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > PI {
            d - 2.0 * PI
        } else if d < -PI {
            d + 2.0 * PI
        } else {
            d
        }
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).sin();
    let lm = f64::midpoint(l1, l2);
    let cmp = f64::midpoint(c1p, c2p);
    let hm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= PI {
        f64::midpoint(h1p, h2p)
    } else if h1p + h2p < 2.0 * PI {
        (h1p + h2p + 2.0 * PI) / 2.0
    } else {
        (h1p + h2p - 2.0 * PI) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - PI / 6.0).cos()
        + 0.24 * (2.0 * hm).cos()
        + 0.32 * (3.0 * hm + PI / 30.0).cos()
        - 0.20 * (4.0 * hm - 63.0 * PI / 180.0).cos();
    let dtheta = PI / 6.0 * (-((hm * 180.0 / PI - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cmp.powi(7) / (cmp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -(2.0 * dtheta).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

/// An image's pixels as the comparison reads them: rows of `width`
/// pixels of `channels` interleaved values. The first three channels are
/// linear Rec.2020; a single channel is grey.
#[derive(Clone, Copy, Debug)]
pub struct Pixels<'a> {
    pub data: &'a [f32],
    pub channels: usize,
    pub width: usize,
    pub height: usize,
}

impl<'a> Pixels<'a> {
    pub fn new(
        data: &'a [f32],
        channels: usize,
        width: usize,
        height: usize,
    ) -> Result<Pixels<'a>, String> {
        if channels == 0 || data.len() != width * height * channels {
            return Err(format!(
                "{} values for {width} x {height} pixels of {channels} channels",
                data.len()
            ));
        }
        Ok(Pixels {
            data,
            channels,
            width,
            height,
        })
    }

    fn rgb(&self, i: usize) -> [f32; 3] {
        let p = &self.data[i * self.channels..(i + 1) * self.channels];
        if p.len() >= 3 {
            [p[0], p[1], p[2]]
        } else {
            [p[0]; 3]
        }
    }
}

/// How far two images are, in Delta E 2000 over every pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Difference {
    /// The decoded pixels are the same, bit for bit: every other field
    /// is 0, nothing was computed.
    pub identical: bool,
    pub mean: f64,
    /// The 99.9th percentile: the max of all but one pixel in a thousand.
    pub p999: f64,
    /// The 99.99th percentile: all but one pixel in ten thousand.
    pub p9999: f64,
    pub max: f64,
    /// The largest difference of a channel value, for the record.
    pub max_abs: f64,
}

impl Difference {
    pub const IDENTICAL: Difference = Difference {
        identical: true,
        mean: 0.0,
        p999: 0.0,
        p9999: 0.0,
        max: 0.0,
        max_abs: 0.0,
    };

    /// Within every bound of `t`.
    pub fn within(&self, t: &Thresholds) -> bool {
        self.identical
            || (self.mean <= t.mean
                && self.p999 <= t.p999
                && self.p9999 <= t.p9999
                && self.max <= t.max)
    }

    /// The bounds of `t` this difference goes beyond, named with their
    /// values, empty when it is within them.
    pub fn beyond(&self, t: &Thresholds) -> Vec<String> {
        if self.identical {
            return Vec::new();
        }
        [
            ("mean", self.mean, t.mean),
            ("p99.9", self.p999, t.p999),
            ("p99.99", self.p9999, t.p9999),
            ("max", self.max, t.max),
        ]
        .into_iter()
        .filter(|(_, v, bound)| v > bound)
        .map(|(name, v, bound)| format!("{name} {v:.3} > {bound}"))
        .collect()
    }
}

/// The bounds a job holds a [`Difference`] to, in Delta E 2000; an
/// infinite bound is not checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    pub mean: f64,
    pub p999: f64,
    pub p9999: f64,
    pub max: f64,
}

impl Thresholds {
    /// No bound at all.
    pub const NONE: Thresholds = Thresholds {
        mean: f64::INFINITY,
        p999: f64::INFINITY,
        p9999: f64::INFINITY,
        max: f64::INFINITY,
    };
}

/// Compares two images of the same size: identical decoded pixels first,
/// then Delta E 2000 (see the module's documentation).
pub fn difference(ours: &Pixels, theirs: &Pixels) -> Result<Difference, String> {
    if (ours.width, ours.height) != (theirs.width, theirs.height) {
        return Err(format!(
            "sizes differ: {} x {} against {} x {}",
            ours.width, ours.height, theirs.width, theirs.height
        ));
    }
    let n = ours.width * ours.height;
    let same = |a: [f32; 3], b: [f32; 3]| (0..3).all(|c| a[c].to_bits() == b[c].to_bits());
    if (0..n).all(|i| same(ours.rgb(i), theirs.rgb(i))) {
        return Ok(Difference::IDENTICAL);
    }
    let (mut max, mut sum, mut max_abs) = (0.0f64, 0.0f64, 0.0f64);
    let mut all = Vec::with_capacity(n);
    let wide = |p: [f32; 3]| p.map(f64::from);
    for i in 0..n {
        let (p, q) = (ours.rgb(i), theirs.rgb(i));
        for c in 0..3 {
            max_abs = max_abs.max(f64::from((p[c] - q[c]).abs()));
        }
        let d = delta_e_2000(lab(wide(p)), lab(wide(q)));
        max = max.max(d);
        sum += d;
        all.push(d);
    }
    let rank = |q: f64| ((n as f64 * q) as usize).min(n - 1);
    let (r999, r9999) = (rank(0.999), rank(0.9999));
    let p9999 = *all.select_nth_unstable_by(r9999, f64::total_cmp).1;
    // everything before r9999 is at most p9999: the 99.9th is among them
    let p999 = if r999 < r9999 {
        *all[..r9999].select_nth_unstable_by(r999, f64::total_cmp).1
    } else {
        p9999
    };
    Ok(Difference {
        identical: false,
        mean: sum / n as f64,
        p999,
        p9999,
        max,
        max_abs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(values: &[f32]) -> Pixels<'_> {
        Pixels::new(values, 1, values.len(), 1).unwrap()
    }

    #[test]
    fn identical_pixels_take_the_fast_path() {
        let a = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let d = difference(
            &Pixels::new(&a, 3, 2, 1).unwrap(),
            &Pixels::new(&a, 3, 2, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(d, Difference::IDENTICAL);
        // a fourth channel is not compared
        let b = [0.1, 0.2, 0.3, 9.0, 0.4, 0.5, 0.6, 7.0];
        let d = difference(
            &Pixels::new(&a, 3, 2, 1).unwrap(),
            &Pixels::new(&b, 4, 2, 1).unwrap(),
        )
        .unwrap();
        assert!(d.identical);
        assert!(d.within(&Thresholds {
            mean: 0.0,
            p999: 0.0,
            p9999: 0.0,
            max: 0.0
        }));
    }

    #[test]
    fn one_outlier_shows_in_the_max_and_the_high_percentiles_only() {
        let mut ours = vec![0.18f32; 100_000];
        let theirs = ours.clone();
        ours[0] = 0.9;
        let d = difference(&grey(&ours), &grey(&theirs)).unwrap();
        assert!(!d.identical);
        assert!(d.max > 20.0, "{d:?}");
        assert_eq!(d.p999, 0.0, "one pixel in 100 000");
        assert_eq!(d.p9999, 0.0, "below one in 10 000");
        assert!(d.mean < 0.001, "{d:?}");
        let cap = Thresholds {
            max: 10.0,
            ..Thresholds::NONE
        };
        assert!(!d.within(&cap));
        assert_eq!(d.beyond(&cap).len(), 1);
        assert!(d.beyond(&cap)[0].starts_with("max"), "{:?}", d.beyond(&cap));
        assert!(d.within(&Thresholds { max: 50.0, ..cap }));
    }

    #[test]
    fn the_percentiles_rank_the_differences() {
        // 10 000 pixels: the 20 last differ, more and more
        let theirs = vec![0.2f32; 10_000];
        let mut ours = theirs.clone();
        for k in 0..20 {
            ours[9_980 + k] = 0.2 + 0.001 * (k + 1) as f32;
        }
        let d = difference(&grey(&ours), &grey(&theirs)).unwrap();
        assert!(d.p999 < d.p9999 && d.p9999 <= d.max, "{d:?}");
        assert!(d.p999 > 0.0, "the 10 largest are above the 99.9th");
    }

    #[test]
    fn sizes_must_match() {
        assert!(difference(&grey(&[0.1, 0.2]), &grey(&[0.1])).is_err());
        assert!(Pixels::new(&[0.1, 0.2], 3, 1, 1).is_err());
    }

    #[test]
    fn delta_e_is_zero_on_the_same_colour() {
        let c = lab([0.2, 0.3, 0.4]);
        assert_eq!(delta_e_2000(c, c), 0.0);
        assert!(
            (lab([1.0, 1.0, 1.0])[0] - 100.0).abs() < 1e-9,
            "white is L 100"
        );
    }
}
