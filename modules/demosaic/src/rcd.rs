//! RCD, Ratio Corrected Demosaicing (Luis Sanz Rodríguez, release 2.3,
//! GPLv3), as darktable runs it (`src/iop/demosaicing/rcd.c`, tuned by
//! Ingo Weyrich and Hanno Schwalm). Bayer patterns only.
//!
//! darktable works on tiles and keeps the pixels at least `BORDER` from a
//! tile's edge; here the tile is the input region, which the module grows
//! by `BORDER` around the output, so every output pixel is computed as
//! darktable computes an inner one. The packed half-size buffers index
//! like darktable's, which needs an even row stride: the tile starts on an
//! even column and row, so a pixel's result does not depend on where a
//! render's region begins.

use helios_photo::Cfa;
use helios_photo::cfa::Channel;

/// What darktable keeps of a tile: pixels this far from its edges.
pub const BORDER: u32 = 10;

const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;

fn sqr(x: f32) -> f32 {
    x * x
}

/// `a * (b - c) + c`, darktable's `interpolatef`.
fn interpolate(a: f32, b: f32, c: f32) -> f32 {
    a * (b - c) + c
}

fn color(cfa: &Cfa, x: i64, y: i64) -> usize {
    match cfa.color_at(x, y) {
        Channel::Red => 0,
        Channel::Green => 1,
        Channel::Blue => 2,
    }
}

/// Demosaics a tile of `width` × `height` photosites whose top left one
/// is at `(x0, y0)` in the image (both even), read by `at(col, row)` in
/// tile coordinates. Returns the red, green and blue planes, `stride`
/// wide; only pixels `BORDER` or more from the tile's edges are exact.
/// `scaler` is the largest value expected (darktable's processed
/// maximum): data is normalized by it, which only matters for `EPS`.
pub fn demosaic(
    cfa: &Cfa,
    x0: i64,
    y0: i64,
    width: usize,
    height: usize,
    scaler: f32,
    at: impl Fn(usize, usize) -> f32,
) -> (usize, [Vec<f32>; 3]) {
    debug_assert!(x0 % 2 == 0 && y0 % 2 == 0);
    let stride = width + width % 2;
    let (w1, w2, w3, w4) = (stride, 2 * stride, 3 * stride, 4 * stride);
    let n = stride * height;
    let fc = |row: usize, col: usize| color(cfa, x0 + col as i64, y0 + row as i64);
    let revscaler = 1.0 / scaler;

    let mut cfa_buf = vec![0.0f32; n];
    let mut rgb = [vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]];
    // Step 0: the data, never negative, in every plane its colour feeds.
    for row in 0..height {
        for col in 0..width {
            let v = at(col, row).max(0.0) * revscaler;
            let i = row * stride + col;
            cfa_buf[i] = v;
            rgb[fc(row, col)][i] = v;
        }
    }
    // darktable fills both planes of a row's two colours; the other
    // colour's plane is overwritten before it is read, except green on
    // red and blue rows, which step 0 sets as darktable does.
    for row in 0..height {
        let c0 = fc(row, 0);
        let c1 = fc(row, 1);
        for col in 0..width {
            let i = row * stride + col;
            rgb[c0][i] = cfa_buf[i];
            rgb[c1][i] = cfa_buf[i];
        }
    }
    let c = &cfa_buf;
    let (rows, cols) = (height, width);
    if rows < 2 * BORDER as usize || cols < 2 * BORDER as usize {
        return (stride, rgb);
    }

    // Step 1: vertical and horizontal discrimination.
    let hpf_v = |i: usize| {
        sqr(
            (c[i - w3] - c[i - w1] - c[i + w1] + c[i + w3]) - 3.0 * (c[i - w2] + c[i + w2])
                + 6.0 * c[i],
        )
    };
    let hpf_h = |i: usize| {
        sqr((c[i - 3] - c[i - 1] - c[i + 1] + c[i + 3]) - 3.0 * (c[i - 2] + c[i + 2]) + 6.0 * c[i])
    };
    let mut v_hpf = vec![0.0f32; n];
    for row in 3..rows - 3 {
        for col in 4..cols - 4 {
            v_hpf[row * stride + col] = hpf_v(row * stride + col);
        }
    }
    let mut h_hpf = vec![0.0f32; n];
    for row in 4..rows - 4 {
        for col in 3..cols - 3 {
            h_hpf[row * stride + col] = hpf_h(row * stride + col);
        }
    }
    let mut vh_dir = vec![0.0f32; n];
    for row in 4..rows - 4 {
        for col in 4..cols - 4 {
            let i = row * stride + col;
            let v = (v_hpf[i - w1] + v_hpf[i] + v_hpf[i + w1]).max(EPSSQ);
            let h = (h_hpf[i - 1] + h_hpf[i] + h_hpf[i + 1]).max(EPSSQ);
            vh_dir[i] = v / (v + h);
        }
    }

    // Step 2: low pass filter at red and blue photosites (packed by half).
    let mut lpf = vec![0.0f32; n / 2 + 1];
    for row in 2..rows - 2 {
        let mut col = 2 + (fc(row, 0) & 1);
        while col < cols - 2 {
            let i = row * stride + col;
            lpf[i / 2] = c[i]
                + 0.5 * (c[i - w1] + c[i + w1] + c[i - 1] + c[i + 1])
                + 0.25 * (c[i - w1 - 1] + c[i - w1 + 1] + c[i + w1 - 1] + c[i + w1 + 1]);
            col += 2;
        }
    }

    // Step 3: green at red and blue photosites.
    for row in 4..rows - 4 {
        let mut col = 4 + (fc(row, 0) & 1);
        while col < cols - 4 {
            let i = row * stride + col;
            let li = i / 2;
            let ci = c[i];
            let n_grad = EPS
                + (c[i - w1] - c[i + w1]).abs()
                + (ci - c[i - w2]).abs()
                + (c[i - w1] - c[i - w3]).abs()
                + (c[i - w2] - c[i - w4]).abs();
            let s_grad = EPS
                + (c[i + w1] - c[i - w1]).abs()
                + (ci - c[i + w2]).abs()
                + (c[i + w1] - c[i + w3]).abs()
                + (c[i + w2] - c[i + w4]).abs();
            let w_grad = EPS
                + (c[i - 1] - c[i + 1]).abs()
                + (ci - c[i - 2]).abs()
                + (c[i - 1] - c[i - 3]).abs()
                + (c[i - 2] - c[i - 4]).abs();
            let e_grad = EPS
                + (c[i + 1] - c[i - 1]).abs()
                + (ci - c[i + 2]).abs()
                + (c[i + 1] - c[i + 3]).abs()
                + (c[i + 2] - c[i + 4]).abs();
            let l = lpf[li];
            let n_est = c[i - w1] * (l + l) / (EPS + l + lpf[li - w1]);
            let s_est = c[i + w1] * (l + l) / (EPS + l + lpf[li + w1]);
            let w_est = c[i - 1] * (l + l) / (EPS + l + lpf[li - 1]);
            let e_est = c[i + 1] * (l + l) / (EPS + l + lpf[li + 1]);
            let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
            let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);
            let central = vh_dir[i];
            let around = 0.25
                * (vh_dir[i - w1 - 1]
                    + vh_dir[i - w1 + 1]
                    + vh_dir[i + w1 - 1]
                    + vh_dir[i + w1 + 1]);
            let disc = if (0.5 - central).abs() < (0.5 - around).abs() {
                around
            } else {
                central
            };
            rgb[1][i] = interpolate(disc.clamp(0.0, 1.0), h_est, v_est);
            col += 2;
        }
    }

    // Step 4.0: diagonal colour difference high pass (packed by half).
    let mut p_hpf = vec![0.0f32; n / 2 + 1];
    let mut q_hpf = vec![0.0f32; n / 2 + 1];
    for row in 3..rows - 3 {
        let mut col = 3;
        while col < cols - 3 {
            let i = row * stride + col;
            p_hpf[i / 2] = sqr(
                (c[i - w3 - 3] - c[i - w1 - 1] - c[i + w1 + 1] + c[i + w3 + 3])
                    - 3.0 * (c[i - w2 - 2] + c[i + w2 + 2])
                    + 6.0 * c[i],
            );
            q_hpf[i / 2] = sqr(
                (c[i - w3 + 3] - c[i - w1 + 1] - c[i + w1 - 1] + c[i + w3 - 3])
                    - 3.0 * (c[i - w2 + 2] + c[i + w2 - 2])
                    + 6.0 * c[i],
            );
            col += 2;
        }
    }
    // Step 4.1: diagonal discrimination at red and blue photosites.
    let mut pq_dir = vec![0.0f32; n / 2 + 1];
    for row in 4..rows - 4 {
        let mut col = 4 + (fc(row, 0) & 1);
        while col < cols - 4 {
            let i = row * stride + col;
            let (i2, i3, i4) = (i / 2, (i - w1 - 1) / 2, (i + w1 - 1) / 2);
            let p = (p_hpf[i3] + p_hpf[i2] + p_hpf[i4 + 1]).max(EPSSQ);
            let q = (q_hpf[i3 + 1] + q_hpf[i2] + q_hpf[i4]).max(EPSSQ);
            pq_dir[i2] = p / (p + q);
            col += 2;
        }
    }
    // Step 4.2: red at blue photosites and blue at red ones.
    for row in 4..rows - 4 {
        let mut col = 4 + (fc(row, 0) & 1);
        while col < cols - 4 {
            let i = row * stride + col;
            let ch = 2 - fc(row, col);
            let (pi, pi2, pi3) = (i / 2, (i - w1 - 1) / 2, (i + w1 - 1) / 2);
            let central = pq_dir[pi];
            let around = 0.25 * (pq_dir[pi2] + pq_dir[pi2 + 1] + pq_dir[pi3] + pq_dir[pi3 + 1]);
            let disc = if (0.5 - central).abs() < (0.5 - around).abs() {
                around
            } else {
                central
            };
            let (r, g) = (&rgb[ch], &rgb[1]);
            let nw_grad = EPS
                + (r[i - w1 - 1] - r[i + w1 + 1]).abs()
                + (r[i - w1 - 1] - r[i - w3 - 3]).abs()
                + (g[i] - g[i - w2 - 2]).abs();
            let ne_grad = EPS
                + (r[i - w1 + 1] - r[i + w1 - 1]).abs()
                + (r[i - w1 + 1] - r[i - w3 + 3]).abs()
                + (g[i] - g[i - w2 + 2]).abs();
            let sw_grad = EPS
                + (r[i - w1 + 1] - r[i + w1 - 1]).abs()
                + (r[i + w1 - 1] - r[i + w3 - 3]).abs()
                + (g[i] - g[i + w2 - 2]).abs();
            let se_grad = EPS
                + (r[i - w1 - 1] - r[i + w1 + 1]).abs()
                + (r[i + w1 + 1] - r[i + w3 + 3]).abs()
                + (g[i] - g[i + w2 + 2]).abs();
            let nw_est = r[i - w1 - 1] - g[i - w1 - 1];
            let ne_est = r[i - w1 + 1] - g[i - w1 + 1];
            let sw_est = r[i + w1 - 1] - g[i + w1 - 1];
            let se_est = r[i + w1 + 1] - g[i + w1 + 1];
            let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
            let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);
            let value = g[i] + interpolate(disc.clamp(0.0, 1.0), q_est, p_est);
            rgb[ch][i] = value;
            col += 2;
        }
    }
    // Step 4.3: red and blue at green photosites.
    for row in 4..rows - 4 {
        let mut col = 4 + (fc(row, 1) & 1);
        while col < cols - 4 {
            let i = row * stride + col;
            let central = vh_dir[i];
            let around = 0.25
                * (vh_dir[i - w1 - 1]
                    + vh_dir[i - w1 + 1]
                    + vh_dir[i + w1 - 1]
                    + vh_dir[i + w1 + 1]);
            let disc = if (0.5 - central).abs() < (0.5 - around).abs() {
                around
            } else {
                central
            };
            let g = &rgb[1];
            let g1 = g[i];
            let n1 = EPS + (g1 - g[i - w2]).abs();
            let s1 = EPS + (g1 - g[i + w2]).abs();
            let w1g = EPS + (g1 - g[i - 2]).abs();
            let e1 = EPS + (g1 - g[i + 2]).abs();
            let (gn, gs, gw, ge) = (g[i - w1], g[i + w1], g[i - 1], g[i + 1]);
            for ch in [0, 2] {
                let r = &rgb[ch];
                let sn = (r[i - w1] - r[i + w1]).abs();
                let ew = (r[i - 1] - r[i + 1]).abs();
                let n_grad = n1 + sn + (r[i - w1] - r[i - w3]).abs();
                let s_grad = s1 + sn + (r[i + w1] - r[i + w3]).abs();
                let w_grad = w1g + ew + (r[i - 1] - r[i - 3]).abs();
                let e_grad = e1 + ew + (r[i + 1] - r[i + 3]).abs();
                let n_est = r[i - w1] - gn;
                let s_est = r[i + w1] - gs;
                let w_est = r[i - 1] - gw;
                let e_est = r[i + 1] - ge;
                let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);
                let value = g1 + interpolate(disc.clamp(0.0, 1.0), h_est, v_est);
                rgb[ch][i] = value;
            }
            col += 2;
        }
    }
    for plane in &mut rgb {
        for v in plane.iter_mut() {
            *v = scaler * v.max(0.0);
        }
    }
    (stride, rgb)
}
