//! The hue/saturation table: the look's second colour stage, after the
//! matrix (`docs/design/camera-look.md`, section 4.2).
//!
//! DNG's `ProfileHueSatMap` layout: [`HUES`] hues × [`SATURATIONS`]
//! saturations × [`VALUES`] values, each node a hue shift in degrees, a
//! saturation scale and a value scale, stored value by value, then hue by
//! hue, then saturation by saturation. Hue, saturation and value are the
//! HSV of the pixel divided by the neutral channel by channel, so the
//! neutral has saturation 0 and the zero-saturation row (the identity)
//! keeps it. The value axis is scene-linear, so it is read through
//! [`value_coordinate`]; the luminance is restored after the table (the
//! tone curve owns it), so every value scale is 1.

use super::{at_least, luma, positive};
use serde::{Deserialize, Serialize};

/// Hue divisions, every 5 degrees.
pub const HUES: usize = 72;
/// Saturation divisions, from 0 to 1.
pub const SATURATIONS: usize = 5;
/// Value divisions, from 0 to 1 in [`value_coordinate`].
pub const VALUES: usize = 5;

/// The kernels' widths: hue in degrees, saturation, value coordinate.
const SIGMA_HUE: f64 = 2.5;
const SIGMA_SAT: f64 = 0.15;
const SIGMA_VALUE: f64 = 0.12;
/// Pairs under this saturation say nothing about hue.
const MIN_SATURATION: f64 = 0.02;
/// The shrinkage toward the identity, in kernel weight: a node with
/// fewer samples near it stays near the identity.
const SHRINK: f64 = 1.0;
/// The bounds of a node.
const MAX_HUE_SHIFT: f64 = 45.0;
const MAX_SAT_SCALE: f64 = 3.0;

/// A hue/saturation table in the DNG layout, with the neutral it is read
/// against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HueSatTable {
    /// The neutral, as the look's matrix keeps it.
    pub neutral: [f64; 3],
    /// `[hue shift (degrees), saturation scale, value scale]` per node,
    /// index `(value × HUES + hue) × SATURATIONS + saturation`.
    pub nodes: Vec<[f64; 3]>,
}

/// A scene-linear HSV value (0 and up) on the table's value axis (0 to 1):
/// `sqrt(v / (1 + v))`, which spreads shadows and keeps highlights in.
pub fn value_coordinate(v: f64) -> f64 {
    let v = v.max(0.0);
    (v / (1.0 + v)).sqrt()
}

fn index(v: usize, h: usize, s: usize) -> usize {
    (v * HUES + h) * SATURATIONS + s
}

/// Hexcone HSV of `rgb`: hue in degrees [0, 360), saturation, value.
/// Saturation may pass 1 for a channel under 0.
pub(super) fn hsv(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    if !positive(max) || !positive(c) {
        return [0.0, 0.0, max];
    }
    let h = if max == r {
        60.0 * ((g - b) / c)
    } else if max == g {
        60.0 * ((b - r) / c + 2.0)
    } else {
        60.0 * ((r - g) / c + 4.0)
    };
    [h.rem_euclid(360.0), c / max, max]
}

pub(super) fn from_hsv([h, s, v]: [f64; 3]) -> [f64; 3] {
    let c = v * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// The signed difference `a − b` of two hues, in (−180, 180].
pub(super) fn hue_difference(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

impl HueSatTable {
    /// The identity table for `neutral`.
    pub fn identity(neutral: [f64; 3]) -> HueSatTable {
        HueSatTable {
            neutral,
            nodes: vec![[0.0, 1.0, 1.0]; HUES * SATURATIONS * VALUES],
        }
    }

    /// The node at value `v`, hue `h`, saturation `s` divisions.
    pub fn node(&self, v: usize, h: usize, s: usize) -> [f64; 3] {
        self.nodes[index(v, h, s)]
    }

    /// The hue shift and saturation scale at hue `h` (degrees),
    /// saturation `s` and value coordinate `vc`: trilinear, the hue
    /// wrapping round, saturation and value clamped to the grid.
    pub fn lookup(&self, h: f64, s: f64, vc: f64) -> [f64; 2] {
        let hx = h.rem_euclid(360.0) / (360.0 / HUES as f64);
        let h0 = (hx.floor() as usize) % HUES;
        let fh = hx - hx.floor();
        let h1 = (h0 + 1) % HUES;
        let cell = |x: f64, n: usize| {
            let x = x.clamp(0.0, 1.0) * (n - 1) as f64;
            let i = (x.floor() as usize).min(n - 2);
            (i, x - i as f64)
        };
        let (s0, fs) = cell(s, SATURATIONS);
        let (v0, fv) = cell(vc, VALUES);
        let mut out = [0.0; 2];
        for (v, wv) in [(v0, 1.0 - fv), (v0 + 1, fv)] {
            for (hh, wh) in [(h0, 1.0 - fh), (h1, fh)] {
                for (ss, ws) in [(s0, 1.0 - fs), (s0 + 1, fs)] {
                    let w = wv * wh * ws;
                    let n = self.node(v, hh, ss);
                    out[0] += w * n[0];
                    out[1] += w * n[1];
                }
            }
        }
        out
    }

    /// The table applied to a scene-linear pixel, its luminance kept.
    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let n = self.neutral;
        let l = luma(p);
        if !positive(l) {
            return p;
        }
        let q = [p[0] / n[0], p[1] / n[1], p[2] / n[2]];
        let [h, s, v] = hsv(q);
        if !positive(s) {
            return p;
        }
        let [dh, scale] = self.lookup(h, s, value_coordinate(v));
        let r = from_hsv([h + dh, s * scale, v]);
        let o = [r[0] * n[0], r[1] * n[1], r[2] * n[2]];
        let lo = luma(o);
        if !positive(lo) {
            return p;
        }
        let k = l / lo;
        o.map(|c| c * k)
    }
}

/// The table taking `scene` (pixels after the matrix) to `preview`'s hues
/// and saturations, by Gaussian kernel regression around every node,
/// samples weighted by their saturation, shrunk toward the identity where
/// samples are few; the zero-saturation row stays the identity. `None`
/// when the neutral has a channel at 0 or under, or no pair has colour.
pub fn fit_table(
    scene: &[[f64; 3]],
    preview: &[[f64; 3]],
    neutral: [f64; 3],
) -> Option<HueSatTable> {
    if !neutral.iter().all(|&c| positive(c) && c.is_finite()) {
        return None;
    }
    let size = HUES * SATURATIONS * VALUES;
    // Per node: Σw, Σw·Δh, Σw·s·s', Σw·s².
    let mut sums = vec![[0.0; 4]; size];
    let mut used = 0usize;
    let step = 360.0 / HUES as f64;
    let reach_h = (3.0 * SIGMA_HUE / step).ceil() as i64;
    for (&x, &y) in scene.iter().zip(preview) {
        let q = [x[0] / neutral[0], x[1] / neutral[1], x[2] / neutral[2]];
        let r = [y[0] / neutral[0], y[1] / neutral[1], y[2] / neutral[2]];
        let [h, s, v] = hsv(q);
        let [hr, sr, vr] = hsv(r);
        if !at_least(s, MIN_SATURATION) || !positive(vr) || !s.is_finite() || !sr.is_finite() {
            continue;
        }
        let dh = hue_difference(hr, h);
        if dh.abs() > MAX_HUE_SHIFT {
            continue;
        }
        used += 1;
        let vc = value_coordinate(v);
        let centre = (h / step).round() as i64;
        for vi in 0..VALUES {
            let dv = vc - vi as f64 / (VALUES - 1) as f64;
            if dv.abs() > 3.0 * SIGMA_VALUE {
                continue;
            }
            for si in 1..SATURATIONS {
                let ds = s.min(1.0) - si as f64 / (SATURATIONS - 1) as f64;
                if ds.abs() > 3.0 * SIGMA_SAT {
                    continue;
                }
                for k in centre - reach_h..=centre + reach_h {
                    let hi = k.rem_euclid(HUES as i64) as usize;
                    let dhn = hue_difference(h, hi as f64 * step);
                    let g = (-0.5
                        * ((dhn / SIGMA_HUE).powi(2)
                            + (ds / SIGMA_SAT).powi(2)
                            + (dv / SIGMA_VALUE).powi(2)))
                    .exp();
                    let w = g * s;
                    let e = &mut sums[index(vi, hi, si)];
                    e[0] += w;
                    e[1] += w * dh;
                    e[2] += w * s * sr;
                    e[3] += w * s * s;
                }
            }
        }
    }
    if used == 0 {
        return None;
    }
    let mut table = HueSatTable::identity(neutral);
    for vi in 0..VALUES {
        for hi in 0..HUES {
            for si in 1..SATURATIONS {
                let e = sums[index(vi, hi, si)];
                let sn = si as f64 / (SATURATIONS - 1) as f64;
                let prior = SHRINK * sn * sn;
                let shift = (e[1] / (e[0] + SHRINK)).clamp(-MAX_HUE_SHIFT, MAX_HUE_SHIFT);
                let scale = ((e[2] + prior) / (e[3] + prior)).clamp(0.0, MAX_SAT_SCALE);
                table.nodes[index(vi, hi, si)] = [shift, scale, 1.0];
            }
        }
    }
    let finite = table.nodes.iter().flatten().all(|v| v.is_finite());
    finite.then_some(table)
}

/// The table's numbers, for the non-finite gate.
pub(super) fn numbers(table: &HueSatTable) -> impl Iterator<Item = &f64> {
    table.neutral.iter().chain(table.nodes.iter().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips() {
        for rgb in [
            [0.2, 0.5, 0.1],
            [0.9, 0.1, 0.4],
            [0.3, 0.3, 0.6],
            [0.0, 0.0, 0.0],
        ] {
            let back = from_hsv(hsv(rgb));
            for c in 0..3 {
                assert!((back[c] - rgb[c]).abs() < 1e-12, "{rgb:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn the_identity_changes_nothing_and_keeps_luminance() {
        let t = HueSatTable::identity([1.0; 3]);
        for p in [[0.2, 0.5, 0.1], [1.5, 0.3, 0.2], [0.05, 0.05, 0.05]] {
            let o = t.apply(p);
            for c in 0..3 {
                assert!((o[c] - p[c]).abs() < 1e-12, "{p:?} -> {o:?}");
            }
        }
    }
}
