//! The charts Helios knows: their reference values and their layout.
//!
//! The values are ported from darktable's `src/common/colorchecker.h`
//! (GPL-3.0-or-later, as Helios), whose source is X-Rite's published
//! measurements: CIE Lab, D50 white, 2° observer. darktable stores each
//! patch with its centre on the chart; Helios stores the patches row by
//! row and derives the centres from the grid, which the tests check
//! against darktable's.
//!
//! A chart's coordinates are in **patch pitches**: the centre of the patch
//! in column `i` and row `j` (from 0, the first row at the top when the
//! chart is upright, as its maker prints it) is at `(i, j)`.

/// One patch: its name on the chart (row letter, column number) and its
/// reference colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    pub name: &'static str,
    /// CIE Lab, D50 white, 2° observer.
    pub lab: [f64; 3],
}

/// A colour chart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chart {
    /// Stable identifier, stored in profiles (`xrite.cc24.2014`).
    pub id: &'static str,
    /// What the interface calls it.
    pub name: &'static str,
    /// Where the reference values come from.
    pub source: &'static str,
    /// The date of the reference values, as their source gives it.
    pub date: &'static str,
    pub columns: usize,
    pub rows: usize,
    /// A patch's side as a fraction of the pitch (the distance between two
    /// neighbouring patches' centres).
    pub patch_side: f64,
    /// The patches row by row: `patches[j * columns + i]`.
    pub patches: &'static [Patch],
    /// The neutral patches, from the lightest to the darkest.
    pub neutrals: &'static [usize],
    /// The patch closest to a middle grey, for exposure.
    pub middle_grey: usize,
}

impl Chart {
    /// The number of patches.
    pub fn len(&self) -> usize {
        self.patches.len()
    }

    /// Whether the chart has no patch (never, for the charts shipped).
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }

    /// Patch `k`'s grid position `(column, row)`.
    pub fn position(&self, k: usize) -> (usize, usize) {
        (k % self.columns, k / self.columns)
    }

    /// The index of the patch at `(column, row)`.
    pub fn index(&self, column: usize, row: usize) -> usize {
        row * self.columns + column
    }

    /// The patch called `name` (`"D4"`), with its index.
    pub fn patch(&self, name: &str) -> Option<(usize, &Patch)> {
        self.patches
            .iter()
            .enumerate()
            .find(|(_, p)| p.name == name)
    }

    /// The chart coordinates of the four corner patches' centres, in the
    /// order the handles are given: first patch of the first row, last of
    /// the first row, last of the last row, first of the last row.
    pub fn corner_centres(&self) -> [[f64; 2]; 4] {
        let (c, r) = ((self.columns - 1) as f64, (self.rows - 1) as f64);
        [[0.0, 0.0], [c, 0.0], [c, r], [0.0, r]]
    }
}

/// darktable's sampling square for the X-Rite charts: a half side of
/// `radius * hypot(1, ratio)` of the chart's width (`_extract_patches` in
/// `src/iop/channelmixerrgb.c`), with `radius = 0.055` (of the diagonal)
/// and `ratio = 2/3` (height over width) from `colorchecker.h`, six
/// pitches to the width: `2 × 0.055 × hypot(1, 2/3) × 6 = 0.7932` of the
/// pitch. It is the patch's side, darktable sampling all of it at a
/// safety margin of 1.
const XRITE_CC24_PATCH_SIDE: f64 = 0.7932;

/// The neutral row of the X-Rite ColorChecker 24: white to black.
const XRITE_CC24_NEUTRALS: [usize; 6] = [18, 19, 20, 21, 22, 23];

const fn p(name: &'static str, l: f64, a: f64, b: f64) -> Patch {
    Patch {
        name,
        lab: [l, a, b],
    }
}

/// X-Rite ColorChecker 24 (Classic, and the Passport's 24-patch page),
/// the formulation sold since November 2014.
pub const XRITE_CC24_2014: Chart = Chart {
    id: "xrite.cc24.2014",
    name: "ColorChecker 24 (2014 formulation)",
    source: "X-Rite, post-2014 formulation, via darktable src/common/colorchecker.h (xrite_24_2014)",
    date: "2015-03-28",
    columns: 6,
    rows: 4,
    patch_side: XRITE_CC24_PATCH_SIDE,
    patches: &[
        p("A1", 37.54, 14.37, 14.92),
        p("A2", 64.66, 19.27, 17.50),
        p("A3", 49.32, -3.82, -22.54),
        p("A4", 43.46, -12.74, 22.72),
        p("A5", 54.94, 9.61, -24.79),
        p("A6", 70.48, -32.26, -0.37),
        p("B1", 62.73, 35.83, 56.50),
        p("B2", 39.43, 10.75, -45.17),
        p("B3", 50.57, 48.64, 16.67),
        p("B4", 30.10, 22.54, -20.87),
        p("B5", 71.77, -24.13, 58.19),
        p("B6", 71.51, 18.24, 67.37),
        p("C1", 28.37, 15.42, -49.80),
        p("C2", 54.38, -39.72, 32.27),
        p("C3", 42.43, 51.05, 28.62),
        p("C4", 81.80, 2.67, 80.41),
        p("C5", 50.63, 51.28, -14.12),
        p("C6", 49.57, -29.71, -28.32),
        p("D1", 95.19, -1.03, 2.93),
        p("D2", 81.29, -0.57, 0.44),
        p("D3", 66.89, -0.75, -0.06),
        p("D4", 50.76, -0.13, 0.14),
        p("D5", 35.63, -0.46, -0.48),
        p("D6", 20.64, 0.07, -0.46),
    ],
    neutrals: &XRITE_CC24_NEUTRALS,
    middle_grey: 21,
};

/// X-Rite ColorChecker 24 made before November 2014 (GretagMacbeth and
/// early X-Rite charts).
pub const XRITE_CC24_2000: Chart = Chart {
    id: "xrite.cc24.2000",
    name: "ColorChecker 24 (before 2014)",
    source: "X-Rite, pre-2014 formulation, via darktable src/common/colorchecker.h (xrite_24_2000)",
    date: "2000-03-27",
    columns: 6,
    rows: 4,
    patch_side: XRITE_CC24_PATCH_SIDE,
    patches: &[
        p("A1", 37.986, 13.555, 14.059),
        p("A2", 65.711, 18.13, 17.81),
        p("A3", 49.927, -4.88, -21.905),
        p("A4", 43.139, -13.095, 21.905),
        p("A5", 55.112, 8.844, -25.399),
        p("A6", 70.719, -33.397, -0.199),
        p("B1", 62.661, 36.067, 57.096),
        p("B2", 40.02, 10.41, -45.964),
        p("B3", 51.124, 48.239, 16.248),
        p("B4", 30.325, 22.976, -21.587),
        p("B5", 72.532, -23.709, 57.255),
        p("B6", 71.941, 19.363, 67.857),
        p("C1", 28.778, 14.179, -50.297),
        p("C2", 55.261, -38.342, 31.37),
        p("C3", 42.101, 53.378, 28.19),
        p("C4", 81.733, 4.039, 79.819),
        p("C5", 51.935, 49.986, -14.574),
        p("C6", 51.038, -28.631, -28.638),
        p("D1", 96.539, -0.425, 1.186),
        p("D2", 81.257, -0.638, -0.335),
        p("D3", 66.766, -0.734, -0.504),
        p("D4", 50.867, -0.153, -0.27),
        p("D5", 35.656, -0.421, -1.231),
        p("D6", 20.461, -0.079, -0.973),
    ],
    neutrals: &XRITE_CC24_NEUTRALS,
    middle_grey: 21,
};

/// Every chart Helios knows, the default first.
pub fn charts() -> [&'static Chart; 2] {
    [&XRITE_CC24_2014, &XRITE_CC24_2000]
}

/// The chart with this identifier.
pub fn chart_by_id(id: &str) -> Option<&'static Chart> {
    charts().into_iter().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spot checks against darktable's `colorchecker.h`: (name, L, a, b,
    /// x, y), x and y being darktable's patch centre as a fraction of the
    /// chart's width and height.
    const DARKTABLE_2014: [(&str, f64, f64, f64, f64, f64); 6] = [
        ("A1", 37.54, 14.37, 14.92, 0.087, 0.125),
        ("A6", 70.48, -32.26, -0.37, 0.918, 0.125),
        ("B3", 50.57, 48.64, 16.67, 0.417, 0.375),
        ("C4", 81.80, 2.67, 80.41, 0.584, 0.625),
        ("D1", 95.19, -1.03, 2.93, 0.087, 0.875),
        ("D6", 20.64, 0.07, -0.46, 0.918, 0.875),
    ];
    const DARKTABLE_2000: [(&str, f64, f64, f64, f64, f64); 4] = [
        ("A1", 37.986, 13.555, 14.059, 0.087, 0.125),
        ("B5", 72.532, -23.709, 57.255, 0.751, 0.375),
        ("C2", 55.261, -38.342, 31.37, 0.250, 0.625),
        ("D4", 50.867, -0.153, -0.27, 0.584, 0.875),
    ];

    fn check(chart: &Chart, expected: &[(&str, f64, f64, f64, f64, f64)]) {
        for &(name, l, a, b, x, y) in expected {
            let (k, patch) = chart.patch(name).unwrap();
            assert_eq!(patch.lab, [l, a, b], "{} {name}", chart.id);
            let (i, j) = chart.position(k);
            let cx = (i as f64 + 0.5) / chart.columns as f64;
            let cy = (j as f64 + 0.5) / chart.rows as f64;
            assert!((cx - x).abs() < 0.005, "{name}: column {i}");
            assert!((cy - y).abs() < 0.005, "{name}: row {j}");
        }
    }

    #[test]
    fn reference_values_match_darktable() {
        check(&XRITE_CC24_2014, &DARKTABLE_2014);
        check(&XRITE_CC24_2000, &DARKTABLE_2000);
    }

    #[test]
    fn charts_are_whole_and_named_by_their_grid() {
        for chart in charts() {
            assert_eq!(chart.len(), chart.columns * chart.rows);
            for (k, patch) in chart.patches.iter().enumerate() {
                let (i, j) = chart.position(k);
                let name = format!("{}{}", char::from(b'A' + j as u8), i + 1);
                assert_eq!(patch.name, name);
                assert_eq!(chart.index(i, j), k);
            }
            assert_eq!(chart_by_id(chart.id), Some(chart));
        }
        assert_eq!(chart_by_id("nope"), None);
    }

    #[test]
    fn neutrals_go_from_white_to_black() {
        for chart in charts() {
            let l: Vec<f64> = chart
                .neutrals
                .iter()
                .map(|&k| chart.patches[k].lab[0])
                .collect();
            assert!(l.windows(2).all(|w| w[0] > w[1]), "{l:?}");
            for &k in chart.neutrals {
                let [_, a, b] = chart.patches[k].lab;
                assert!(a.hypot(b) < 3.5, "{}", chart.patches[k].name);
            }
            // darktable's middle grey (index 21) is D4.
            assert_eq!(chart.patches[chart.middle_grey].name, "D4");
        }
    }

    #[test]
    fn patch_side_is_darktables_sampling_square() {
        let (radius, ratio, columns) = (0.055_f64, 2.0_f64 / 3.0, 6.0);
        let side = 2.0 * radius * 1.0_f64.hypot(ratio) * columns;
        assert!((side - XRITE_CC24_PATCH_SIDE).abs() < 1e-4, "{side}");
    }
}
