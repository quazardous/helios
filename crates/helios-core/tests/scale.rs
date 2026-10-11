//! Renders at a scale: sources reduced by the framework, a plain check of
//! the scale, and previews cached apart from exact renders.

use std::sync::Arc;

use helios_core::graph::SOURCE_PORT;
use helios_core::*;

#[test]
fn a_source_at_half_scale_is_the_mean_of_what_each_pixel_covers() {
    let image = Image::from_fn(Region::sized(4, 2), 1, |x, y, _| (x + 10 * y) as f32);
    let half = image.sample_at(Region::sized(2, 1), 0.5);
    // (0 + 1 + 10 + 11) / 4 and (2 + 3 + 12 + 13) / 4.
    assert_eq!(half.data(), &[5.5, 7.5]);
    assert_eq!(image.sample_at(image.region(), 1.0).data(), image.data());
    // Past the edge, the edge pixels are replicated, as at full scale.
    let past = image.sample_at(Region::new(2, 0, 1, 1), 0.5);
    assert_eq!(past.data(), &[3.0 * 0.5 + 13.0 * 0.5]);
}

#[test]
fn a_render_at_a_scale_reads_the_reduced_source_and_is_cached_apart() {
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(Region::sized(8, 8), 1, |x, _, _| x as f32),
    );
    let e = Engine::new(Arc::new(Bus::new()));
    let r = Region::sized(4, 4);
    let preview = e.render_at(&g, src, SOURCE_PORT, r, 0.5).unwrap();
    assert_eq!(&preview.data()[..4], &[0.5, 2.5, 4.5, 6.5]);
    assert_ne!(
        e.key_at(&g, src, r, 0.5).unwrap(),
        e.key(&g, src, r).unwrap()
    );
    for bad in [0.0, -1.0, 1.5, f64::NAN] {
        let err = e.render_at(&g, src, SOURCE_PORT, r, bad).unwrap_err();
        assert!(matches!(err, Error::Scale(_)), "{bad}: {err}");
    }
}
