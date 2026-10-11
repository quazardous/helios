//! Markesteijn's X-Trans demosaicing, one pass, as darktable runs it
//! (`src/iop/demosaicing/xtrans.c`, adapted from dcraw 9.20): greens
//! interpolated in four directions, red and blue rebuilt from them, then
//! for each pixel the most homogeneous directions averaged.
//!
//! darktable works on tiles of 122 photosites and keeps those at least
//! `BORDER` from a tile's edge; here the tile is the input region, grown
//! by `BORDER` around the output. Coordinates are the image's, so the
//! pattern's phase is the image's, as in darktable. darktable's tile
//! stride is even, which one test below relies on: it is kept as the
//! parity of "one row" rather than taken from this tile's width.

// Indexed loops, as in darktable's code, so the port reads against it.
#![allow(clippy::needless_range_loop)]

use helios_photo::Cfa;
use helios_photo::cfa::Channel;

/// What darktable keeps of a tile: pixels this far from its edges.
pub const BORDER: u32 = 12;

const NDIR: usize = 4;

fn color(cfa: &Cfa, x: i64, y: i64) -> usize {
    match cfa.color_at(x, y) {
        Channel::Red => 0,
        Channel::Green => 1,
        Channel::Blue => 2,
    }
}

fn sqr(x: f32) -> f32 {
    x * x
}

fn clamps(a: f32, low: f32, high: f32) -> f32 {
    if a > low {
        if a < high { a } else { high }
    } else {
        low
    }
}

/// Demosaics a tile of `width` × `height` photosites whose top left one
/// is at `(x0, y0)` in the image, read by `at(col, row)` in tile
/// coordinates. Returns the RGB pixels, `width` wide; only those
/// `BORDER` or more from the tile's edges are computed.
pub fn demosaic(
    cfa: &Cfa,
    x0: i64,
    y0: i64,
    width: usize,
    height: usize,
    at: impl Fn(usize, usize) -> f32,
) -> Vec<[f32; 3]> {
    let ts = width as isize;
    let n = width * height;
    let fc = |row: i64, col: i64| color(cfa, col, row);
    let orth: [isize; 12] = [1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1];
    let patt: [[isize; 16]; 2] = [
        [0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0, 0, 0, 0],
        [0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1, -1, -1, 1],
    ];
    let dir: [isize; 4] = [1, ts, ts + 1, ts - 1];

    // A green hexagon around each non-green photosite and vice versa.
    let mut allhex = [[[0isize; 8]; 3]; 3];
    let (mut sgrow, mut sgcol) = (0i64, 0i64);
    for row in 0..3i64 {
        for col in 0..3i64 {
            let mut ng = 0;
            let mut d = 0;
            while d < 10 {
                let g = usize::from(fc(row, col) == 1);
                if fc(row + orth[d] as i64, col + orth[d + 2] as i64) == 1 {
                    ng = 0;
                } else {
                    ng += 1;
                }
                if ng == 4 {
                    sgrow = row;
                    sgcol = col;
                }
                if ng == g + 1 {
                    for c in 0..8 {
                        let v = orth[d] * patt[g][c * 2] + orth[d + 1] * patt[g][c * 2 + 1];
                        let h = orth[d + 2] * patt[g][c * 2] + orth[d + 3] * patt[g][c * 2 + 1];
                        allhex[row as usize][col as usize][c ^ ((g * 2) & d)] = h + v * ts;
                    }
                }
                d += 2;
            }
        }
    }
    let hexmap =
        |row: i64, col: i64| &allhex[((row + 600) % 3) as usize][((col + 600) % 3) as usize];
    // `(row - sgrow) % 3` as C computes it (the sign of the dividend).
    let third = |row: i64| (row - sgrow) % 3;

    let (top, left) = (y0, x0);
    let (mrow, mcol) = (top + height as i64, left + width as i64);
    let idx = |row: i64, col: i64| ((row - top) as isize * ts + (col - left) as isize) as usize;
    let at_off = |p: usize, off: isize| (p as isize + off) as usize;

    let mut rgb: Vec<Vec<[f32; 3]>> = vec![vec![[0.0; 3]; n]; NDIR];
    for row in top..mrow {
        for col in left..mcol {
            let f = fc(row, col);
            let v = at((col - left) as usize, (row - top) as usize).max(0.0);
            let mut pix = [0.0; 3];
            pix[f] = v;
            rgb[0][idx(row, col)] = pix;
        }
    }
    let (first, rest) = rgb.split_at_mut(1);
    for dir in rest {
        dir.copy_from_slice(&first[0]);
    }

    // Green 1 and 3: the min and max of the greens around each red/blue pair.
    let mut gmin = vec![0.0f32; n];
    let mut gmax = vec![0.0f32; n];
    let pad_g1_g3 = 3;
    let mut row = top + pad_g1_g3;
    while row < mrow - pad_g1_g3 {
        let (mut min, mut max) = (f32::MAX, 0.0f32);
        let mut col = left + pad_g1_g3;
        while col < mcol - pad_g1_g3 {
            if fc(row, col) == 1 {
                min = f32::MAX;
                max = 0.0;
                col += 1;
                continue;
            }
            if max == 0.0 {
                let p = idx(row, col);
                let hex = hexmap(row, col);
                for &h in &hex[..6] {
                    let val = rgb[0][at_off(p, h)][1];
                    if min > val {
                        min = val;
                    }
                    if max < val {
                        max = val;
                    }
                }
            }
            gmin[idx(row, col)] = min;
            gmax[idx(row, col)] = max;
            match third(row) {
                1 if row < mrow - 4 => {
                    row += 1;
                    col -= 1;
                }
                2 => {
                    min = f32::MAX;
                    max = 0.0;
                    col += 2;
                    if col < mcol - 4 && row > top + 3 {
                        row -= 1;
                    }
                }
                _ => {}
            }
            col += 1;
        }
        row += 1;
    }

    // Green interpolated horizontally, vertically and along both diagonals.
    let pad_g_interp = 3;
    for row in top + pad_g_interp..mrow - pad_g_interp {
        for col in left + pad_g_interp..mcol - pad_g_interp {
            let f = fc(row, col);
            if f == 1 {
                continue;
            }
            let p = idx(row, col);
            let hex = hexmap(row, col);
            let color = {
                let px = |off: isize, c: usize| rgb[0][at_off(p, off)][c];
                let mut color = [0.0f32; 4];
                color[0] = 0.6796875 * (px(hex[1], 1) + px(hex[0], 1))
                    - 0.1796875 * (px(2 * hex[1], 1) + px(2 * hex[0], 1));
                color[1] = 0.87109375 * px(hex[3], 1)
                    + px(hex[2], 1) * 0.13
                    + 0.359375 * (px(0, f) - px(-hex[2], f));
                for c in 0..2 {
                    color[2 + c] = 0.640625 * px(hex[4 + c], 1)
                        + 0.359375 * px(-2 * hex[4 + c], 1)
                        + 0.12890625
                            * (2.0 * px(0, f) - px(3 * hex[4 + c], f) - px(-3 * hex[4 + c], f));
                }
                color
            };
            let flip = usize::from(third(row) == 0);
            for (c, value) in color.iter().enumerate() {
                rgb[c ^ flip][p][1] = clamps(*value, gmin[p], gmax[p]);
            }
        }
    }

    // Red and blue at the solitary greens.
    let pad_rb_g = 6;
    let mut row = (top - sgrow + pad_rb_g + 2) / 3 * 3 + sgrow;
    while row < mrow - pad_rb_g {
        let mut col = (left - sgcol + pad_rb_g + 2) / 3 * 3 + sgcol;
        while col < mcol - pad_rb_g {
            let p = idx(row, col);
            let mut k = 0;
            let mut h = fc(row, col + 1);
            let mut diff = [0.0f32; 6];
            let mut color = [[0.0f32; 6]; 2];
            let mut i: isize = 1;
            for d in 0..6 {
                for c in 0..2 {
                    let o = i << c;
                    let b = &rgb[k];
                    let g = 2.0 * b[p][1] - b[at_off(p, o)][1] - b[at_off(p, -o)][1];
                    color[usize::from(h != 0)][d] = g + b[at_off(p, o)][h] + b[at_off(p, -o)][h];
                    if d > 1 {
                        diff[d] += sqr(b[at_off(p, o)][1]
                            - b[at_off(p, -o)][1]
                            - b[at_off(p, o)][h]
                            + b[at_off(p, -o)][h])
                            + sqr(g);
                    }
                    h ^= 2;
                }
                if d < 2 || d & 1 == 1 {
                    let d_out = d - usize::from(d > 1 && diff[d - 1] < diff[d]);
                    rgb[k][p][0] = color[0][d_out] / 2.0;
                    rgb[k][p][2] = color[1][d_out] / 2.0;
                    k += 1;
                }
                i ^= ts ^ 1;
                h ^= 2;
            }
            col += 3;
        }
        row += 3;
    }

    // Red at blue photosites and blue at red ones.
    let pad_rb_br = 6;
    for row in top + pad_rb_br..mrow - pad_rb_br {
        for col in left + pad_rb_br..mcol - pad_rb_br {
            let f = 2 - fc(row, col);
            if f == 1 {
                continue;
            }
            let p = idx(row, col);
            let along_column = third(row) != 0;
            let c = if along_column { ts } else { 1 };
            let h = 3 * if along_column { 1 } else { ts };
            // darktable's tile stride is even: "one row" has parity 0.
            let c_parity = usize::from(!along_column);
            for d in 0..NDIR {
                let b = &rgb[d];
                let g0 = b[p][1];
                let i = if d > 1
                    || (d ^ c_parity) & 1 == 1
                    || ((g0 - b[at_off(p, c)][1]).abs() + (g0 - b[at_off(p, -c)][1]).abs())
                        < 2.0 * ((g0 - b[at_off(p, h)][1]).abs() + (g0 - b[at_off(p, -h)][1]).abs())
                {
                    c
                } else {
                    h
                };
                let value = (b[at_off(p, i)][f] + b[at_off(p, -i)][f] + 2.0 * g0
                    - b[at_off(p, i)][1]
                    - b[at_off(p, -i)][1])
                    / 2.0;
                rgb[d][p][f] = value;
            }
        }
    }

    // Red and blue for the 2 x 2 blocks of green.
    let pad_g22 = 8;
    for row in top + pad_g22..mrow - pad_g22 {
        if third(row) == 0 {
            continue;
        }
        for col in left + pad_g22..mcol - pad_g22 {
            if (col - sgcol) % 3 == 0 {
                continue;
            }
            let p = idx(row, col);
            let hex = hexmap(row, col);
            for (k, d) in (0..NDIR).step_by(2).enumerate() {
                let b = &rgb[k];
                let (a, z) = (at_off(p, hex[d]), at_off(p, hex[d + 1]));
                let mut out = b[p];
                if hex[d] + hex[d + 1] != 0 {
                    let g = 3.0 * b[p][1] - 2.0 * b[a][1] - b[z][1];
                    for c in [0, 2] {
                        out[c] = (g + 2.0 * b[a][c] + b[z][c]) / 3.0;
                    }
                } else {
                    let g = 2.0 * b[p][1] - b[a][1] - b[z][1];
                    for c in [0, 2] {
                        out[c] = (g + b[a][c] + b[z][c]) / 2.0;
                    }
                }
                rgb[k][p] = out;
            }
        }
    }

    // From here on, tile coordinates.
    let (rows, cols) = (height as i64, width as i64);
    let tidx = |row: i64, col: i64| (row as isize * ts + col as isize) as usize;
    let mut drv = vec![vec![0.0f32; n]; NDIR];
    let mut yuv = [vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]];
    for d in 0..NDIR {
        let pad_yuv = 8;
        for row in pad_yuv..rows - pad_yuv {
            for col in pad_yuv..cols - pad_yuv {
                let [r, g, b] = rgb[d][tidx(row, col)];
                let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
                yuv[0][tidx(row, col)] = y;
                yuv[1][tidx(row, col)] = (b - y) * 0.56433;
                yuv[2][tidx(row, col)] = (r - y) * 0.67815;
            }
        }
        let f = dir[d & 3];
        let pad_drv = 9;
        for row in pad_drv..rows - pad_drv {
            for col in pad_drv..cols - pad_drv {
                let p = tidx(row, col);
                let mut v = 0.0;
                for plane in &yuv {
                    v += sqr(2.0 * plane[p] - plane[at_off(p, f)] - plane[at_off(p, -f)]);
                }
                drv[d][p] = v;
            }
        }
    }

    // Homogeneity maps from the derivatives.
    let mut homo = vec![vec![0u8; n]; NDIR];
    let pad_homo = 10;
    for row in pad_homo..rows - pad_homo {
        for col in pad_homo..cols - pad_homo {
            let p = tidx(row, col);
            let mut tr = f32::MAX;
            for d in &drv {
                if tr > d[p] {
                    tr = d[p];
                }
            }
            tr *= 8.0;
            for d in 0..NDIR {
                let mut count = 0u8;
                for v in -1..=1i64 {
                    for h in -1..=1i64 {
                        count += u8::from(drv[d][tidx(row + v, col + h)] <= tr);
                    }
                }
                homo[d][p] = count;
            }
        }
    }

    // 5 x 5 sums of homogeneity, by rolling column sums.
    let pad = i64::from(BORDER);
    let mut homosum = vec![vec![0u8; n]; NDIR];
    for d in 0..NDIR {
        for row in pad..rows - pad {
            let mut col = pad - 5;
            let mut v5sum = [0u8; 5];
            homosum[d][tidx(row, col)] = 0;
            col += 1;
            while col < cols - pad {
                let mut colsum = 0u8;
                for v in -2..=2i64 {
                    colsum += homo[d][tidx(row + v, col + 2)];
                }
                let prev = homosum[d][tidx(row, col - 1)];
                homosum[d][tidx(row, col)] = prev - v5sum[(col % 5) as usize] + colsum;
                v5sum[(col % 5) as usize] = colsum;
                col += 1;
            }
        }
    }

    // The most homogeneous directions, averaged.
    let mut out = vec![[0.0f32; 3]; n];
    for row in pad..rows - pad {
        for col in pad..cols - pad {
            let p = tidx(row, col);
            let mut hm = [0u8; NDIR];
            let mut maxval = 0u8;
            for d in 0..NDIR {
                hm[d] = homosum[d][p];
                maxval = maxval.max(hm[d]);
            }
            maxval -= maxval >> 3;
            let mut avg = [0.0f32; 4];
            for d in 0..NDIR {
                if hm[d] >= maxval {
                    for c in 0..3 {
                        avg[c] += rgb[d][p][c];
                    }
                    avg[3] += 1.0;
                }
            }
            out[p] = [0, 1, 2].map(|c| (avg[c] / avg[3]).max(0.0));
        }
    }
    out
}
