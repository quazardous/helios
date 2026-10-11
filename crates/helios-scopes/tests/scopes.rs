//! Scopes counted on images whose answer is known.

use helios_core::{Image, Region};
use helios_scopes::*;

#[test]
fn stops_place_middle_grey_and_count_clipping_apart() {
    let s = Scale::Stops {
        min: -8.0,
        max: 0.0,
    };
    assert_eq!(s.bin(1.0, 8), Bin::At(7));
    assert_eq!(s.bin(2.0, 8), Bin::Above);
    assert_eq!(s.bin(0.0, 8), Bin::Below);
    assert_eq!(s.bin(-0.1, 8), Bin::Below);
    assert_eq!(s.bin(f32::NAN, 8), Bin::Below);
    // 0.18 is about -2.47 stops: bin 5 of 8 over [-8, 0].
    assert_eq!(s.bin(0.18, 8), Bin::At(5));
}

#[test]
fn a_histogram_counts_each_channel() {
    // Two pixels: one red clipped above, one blue below zero.
    let image = Image::new(
        Region::sized(2, 1),
        4,
        vec![2.0, 0.5, 0.5, 1.0, 0.25, 0.5, -0.1, 1.0],
    );
    let h = histogram(&image, 4, Scale::Linear { max: 1.0 });
    assert_eq!(h.channels[0].above, 1);
    assert_eq!(h.channels[0].bins, [0, 1, 0, 0]);
    assert_eq!(h.channels[1].bins, [0, 0, 2, 0]);
    assert_eq!(h.channels[1].peak(), 2);
    assert_eq!(h.channels[2].below, 1);
}

#[test]
fn a_waveform_follows_a_horizontal_ramp() {
    // Grey from 0 to 1 across 8 columns: each band sits one level higher.
    let image = Image::from_fn(Region::sized(8, 3), 4, |x, _, c| {
        if c == 3 { 1.0 } else { x as f32 / 8.0 + 0.01 }
    });
    let w = waveform(&image, 4, 4, Scale::Linear { max: 1.0 });
    for (band, counts) in w.columns.iter().enumerate() {
        assert_eq!(counts.bins[band], 6, "band {band}: {:?}", counts.bins);
    }
}

#[test]
fn a_parade_separates_the_channels() {
    let image = Image::from_fn(Region::sized(4, 2), 4, |_, _, c| {
        [0.9, 0.5, 0.1, 1.0][c as usize]
    });
    let [r, g, b] = parade(&image, 2, 10, Scale::Linear { max: 1.0 });
    assert_eq!(r.columns[0].bins[9], 4);
    assert_eq!(g.columns[1].bins[5], 4);
    assert_eq!(b.columns[0].bins[1], 4);
}
