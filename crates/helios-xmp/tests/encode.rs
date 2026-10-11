//! Encoding params is the inverse of decoding them.

use helios_xmp::{Layouts, decode, encode};
use serde_json::json;

#[test]
fn every_layout_round_trips_its_defaults() {
    let layouts = Layouts::built_in();
    let mut checked = 0;
    for (module, version) in layouts.all() {
        let layout = layouts.get(module, version).unwrap();
        let Ok(bytes) = encode(layout, &json!({})) else {
            continue;
        };
        let values = decode(layout, &bytes).unwrap();
        assert_eq!(
            encode(layout, &values).unwrap(),
            bytes,
            "{module} v{version}"
        );
        checked += 1;
    }
    assert!(checked > 80, "only {checked} layouts encode");
}

#[test]
fn named_values_land_at_their_offsets() {
    let layouts = Layouts::built_in();
    let version = *layouts.versions("exposure").last().unwrap();
    let layout = layouts.get("exposure", version).unwrap();
    let bytes = encode(
        layout,
        &json!({"exposure": 1.5, "black": 0.01, "mode": "EXPOSURE_MODE_MANUAL"}),
    )
    .unwrap();
    let back = decode(layout, &bytes).unwrap();
    assert_eq!(back["exposure"], json!(1.5));
    assert_eq!(back["mode"], json!("EXPOSURE_MODE_MANUAL"));
    // Left out: darktable's default.
    assert_eq!(back["deflicker_percentile"], json!(50.0));
    assert!((back["black"].as_f64().unwrap() - 0.01).abs() < 1e-7);
}
