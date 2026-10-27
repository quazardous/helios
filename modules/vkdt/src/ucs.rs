// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/dtucs.glsl, src/pipe/modules/colourspaces.glsl,
// src/pipe/modules/matrices.h. vkdt: copyright 2019 johannes hanika, under
// the BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// darktable Uniform Color Space 2022: Aurélien Pierre, from darktable,
// under the MIT licence (as vkdt's dtucs.glsl states).
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause AND MIT

//! Colour spaces shared by the ports: vkdt's matrices (row by row), the
//! xyY conversions of linear Rec.2020, and darktable's uniform colour space
//! (dt UCS 2022, by Aurélien Pierre) as vkdt computes it, in `f32` like
//! vkdt's shaders.

// The constants keep vkdt's spelling, digits beyond f32's included, so
// they can be compared with its sources.
#![allow(clippy::excessive_precision)]

/// A 3 x 3 matrix, row by row.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
pub const REC2020_TO_XYZ: Mat3 = [
    [
        0.636958048301290991,
        0.144616903586208406,
        0.168880975164172054,
    ],
    [
        0.26270021201126692,
        0.677998071518871148,
        0.0593017164698619384,
    ],
    [
        4.9999999999999999e-17,
        0.0280726930490874452,
        1.06098505771079066,
    ],
];
pub const XYZ_TO_REC2020: Mat3 = [
    [1.71665119, -0.35567078, -0.25336628],
    [-0.66668435, 1.61648124, 0.01576855],
    [0.01763986, -0.04277061, 0.94210312],
];
pub const REC709_TO_REC2020: Mat3 = [
    [0.62750375, 0.32927542, 0.04330266],
    [0.06910828, 0.91951916, 0.0113596],
    [0.01639406, 0.08801125, 0.89538035],
];
/// CAT16's M, XYZ to cone-like, and its inverse.
pub const CAT16_M: Mat3 = [
    [0.401288, 0.650173, -0.051461],
    [-0.250268, 1.204414, 0.045854],
    [-0.002079, 0.048952, 0.953127],
];
pub const CAT16_MI: Mat3 = [
    [1.86206786, -1.01125463, 0.14918677],
    [0.38752654, 0.62144744, -0.00897398],
    [-0.01584150, -0.03412294, 1.04996444],
];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn apply(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

pub fn apply_f32(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn to_f32(m: &Mat3) -> [[f32; 3]; 3] {
    m.map(|row| row.map(|v| v as f32))
}

/// vkdt's `rec2020_to_xyY`.
pub fn rec2020_to_xyy(rgb: [f32; 3]) -> [f32; 3] {
    let xyz = apply_f32(&to_f32(&REC2020_TO_XYZ), rgb);
    let sum = xyz[0] + xyz[1] + xyz[2];
    [xyz[0] / sum, xyz[1] / sum, xyz[1]]
}

/// vkdt's `xyY_to_rec2020`.
pub fn xyy_to_rec2020(xyy: [f32; 3]) -> [f32; 3] {
    let s = xyy[2] / xyy[1];
    let xyz = [xyy[0] * s, xyy[2], (1.0 - xyy[0] - xyy[1]) * s];
    apply_f32(&to_f32(&XYZ_TO_REC2020), xyz)
}

fn y_to_l_star(y: f32) -> f32 {
    let y_hat = y.powf(0.631651345306265);
    2.098883786377 * y_hat / (y_hat + 1.12426773749357)
}

fn l_star_to_y(l_star: f32) -> f32 {
    (1.12426773749357 * l_star / (2.098883786377 - l_star)).powf(1.5831518565279648)
}

/// A GLSL `mat3(c0, c1, c2)` (columns) times `v`.
fn columns3(c: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| c[0][i] * v[0] + c[1][i] * v[1] + c[2][i] * v[2])
}

/// A GLSL `mat2(a, b, c, d)` (columns `(a, b)`, `(c, d)`) times `v`.
fn columns2(m: [f32; 4], v: [f32; 2]) -> [f32; 2] {
    [m[0] * v[0] + m[2] * v[1], m[1] * v[0] + m[3] * v[1]]
}

const FACTORS: [f32; 2] = [1.39656225667, 1.4513954287];
const HALF_VALUES: [f32; 2] = [1.49217352929, 1.52488637914];

fn xy_to_uv(xy: [f32; 2]) -> [f32; 2] {
    let uvd = columns3(
        [
            [-0.783941002840055, 0.745273540913283, 0.318707282433486],
            [0.277512987809202, -0.205375866083878, 2.16743692732158],
            [0.153836578598858, -0.165478376301988, 0.291320554395942],
        ],
        [xy[0], xy[1], 1.0],
    );
    let uv = [uvd[0] / uvd[2], uvd[1] / uvd[2]];
    let star = [0, 1].map(|i| FACTORS[i] * uv[i] / (uv[i].abs() + HALF_VALUES[i]));
    columns2(
        [
            -1.124983854323892,
            1.86323315098672,
            -0.980483721769325,
            1.971853092390862,
        ],
        star,
    )
}

/// vkdt's `xyY_to_dt_UCS_JCH`: lightness, chroma, hue (radians).
pub fn xyy_to_jch(xyy: [f32; 3], l_white: f32) -> [f32; 3] {
    let uv = xy_to_uv([xyy[0], xyy[1]]);
    let l_star = y_to_l_star(xyy[2]);
    let m2 = uv[0] * uv[0] + uv[1] * uv[1];
    [
        l_star / l_white,
        15.932993652962535 * l_star.powf(0.6523997524738018) * m2.powf(0.6007557017508491)
            / l_white,
        uv[1].atan2(uv[0]),
    ]
}

/// vkdt's `dt_UCS_JCH_to_xyY`.
pub fn jch_to_xyy(jch: [f32; 3], l_white: f32) -> [f32; 3] {
    let l_star = jch[0] * l_white;
    let m = (jch[1] * l_white / (15.932993652962535 * l_star.powf(0.6523997524738018)))
        .powf(0.8322850678616855)
        .clamp(0.0, 0.05);
    let star = columns2(
        [
            -5.037522385190711,
            4.760029407436461,
            -2.504856328185843,
            2.874012963239247,
        ],
        [m * jch[2].cos(), m * jch[2].sin()],
    );
    let uv = [0, 1].map(|i| -HALF_VALUES[i] * star[i] / (star[i].abs() - FACTORS[i]));
    let xyd = columns3(
        [
            [0.167171472114775, -0.150959086409163, 0.940254742367256],
            [0.141299802443708, -0.155185060382272, 1.000000000000000],
            [
                -0.00801531300850582,
                -0.00843312433578007,
                -0.0256325967652889,
            ],
        ],
        [uv[0], uv[1], 1.0],
    );
    [xyd[0] / xyd[2], xyd[1] / xyd[2], l_star_to_y(l_star)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrices_round_trip() {
        let id = mul(&XYZ_TO_REC2020, &REC2020_TO_XYZ);
        let cat = mul(&CAT16_MI, &CAT16_M);
        for i in 0..3 {
            for j in 0..3 {
                let e = if i == j { 1.0 } else { 0.0 };
                assert!((id[i][j] - e).abs() < 1e-6, "{id:?}");
                assert!((cat[i][j] - e).abs() < 1e-6, "{cat:?}");
            }
        }
    }

    #[test]
    fn dt_ucs_round_trips() {
        for rgb in [[0.5f32, 0.3, 0.2], [0.1, 0.4, 0.6], [0.18, 0.18, 0.18]] {
            let xyy = rec2020_to_xyy(rgb);
            let back = xyy_to_rec2020(jch_to_xyy(xyy_to_jch(xyy, 1.0), 1.0));
            for c in 0..3 {
                assert!((back[c] - rgb[c]).abs() < 1e-3, "{rgb:?} -> {back:?}");
            }
        }
    }
}
