//! Culling measures as read-only fields (`docs/design/library.md`, L3 and
//! L4): the focus and clipping an import or the culling backfill keeps in
//! an image's signals document (`helios_library::cull`) are copied to
//! fields whenever that document is written ([`crate::Vault::set_signals`],
//! [`crate::Vault::merge_signals`]), so a filter, a facet, a sort or a
//! smart album's rule reads them in the store, in one query.
//!
//! - `focus`: the signals' focus, higher is sharper;
//! - `clip_white`, `clip_black`: the largest share of one channel's pixels
//!   blown to white or crushed to black (0 to 1).
//!
//! A photo is **soft** below [`SOFT_FOCUS`], **sharp** at or above it,
//! **clipped** past [`CLIPPED_WHITE`] or [`CLIPPED_BLACK`], and
//! **unmeasured** without a focus. The focus depends on what the photo
//! shows: the soft threshold is a first guess, to tune on real photos;
//! within one burst the stack proposal compares focus values directly.

use serde_json::Value;

pub const FOCUS: &str = "focus";
pub const CLIP_WHITE: &str = "clip_white";
pub const CLIP_BLACK: &str = "clip_black";

/// Every culling field.
pub const NAMES: [&str; 3] = [FOCUS, CLIP_WHITE, CLIP_BLACK];

/// A photo with more than this share of one channel blown white is
/// clipped (2 %).
pub const CLIPPED_WHITE: f64 = 0.02;
/// A photo with more than this share of one channel crushed black is
/// clipped (5 %).
pub const CLIPPED_BLACK: f64 = 0.05;
/// A focus below this is soft.
pub const SOFT_FOCUS: f64 = 0.001;

pub const SHARP: &str = "sharp";
pub const SOFT: &str = "soft";
pub const CLIPPED: &str = "clipped";
pub const UNMEASURED: &str = "unmeasured";

/// The culling states a filter and the `cull` facet take.
pub const STATES: [&str; 4] = [SHARP, SOFT, CLIPPED, UNMEASURED];

/// The largest of a signals key's channels (`[r, g, b]`), or the number
/// itself.
fn largest(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_f64)
            .filter(|v| v.is_finite())
            .reduce(f64::max),
        v => v.as_f64().filter(|v| v.is_finite()),
    }
}

/// The culling fields a signals document gives, every one named: `None`
/// clears one the document does not have.
pub fn values(signals: Option<&Value>) -> [(&'static str, Option<Value>); 3] {
    let key = |k: &str| signals.and_then(|s| s.get(k));
    let focus = key(FOCUS).and_then(Value::as_f64).filter(|v| v.is_finite());
    [
        (FOCUS, focus.map(Value::from)),
        (CLIP_WHITE, largest(key(CLIP_WHITE)).map(Value::from)),
        (CLIP_BLACK, largest(key(CLIP_BLACK)).map(Value::from)),
    ]
}

/// Whether a focus is soft.
pub fn soft(focus: f64) -> bool {
    focus < SOFT_FOCUS
}

/// Whether clipping shares make a photo clipped.
pub fn clipped(white: Option<f64>, black: Option<f64>) -> bool {
    white.is_some_and(|w| w > CLIPPED_WHITE) || black.is_some_and(|b| b > CLIPPED_BLACK)
}

/// The culling states of a photo from its fields: `sharp` or `soft` (or
/// `unmeasured` without a focus), and `clipped` beside them.
pub fn states_of(focus: Option<f64>, white: Option<f64>, black: Option<f64>) -> Vec<&'static str> {
    let mut states = vec![match focus {
        None => UNMEASURED,
        Some(f) if soft(f) => SOFT,
        Some(_) => SHARP,
    }];
    if clipped(white, black) {
        states.push(CLIPPED);
    }
    states
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_fields_come_from_the_signals() {
        let doc =
            json!({ "focus": 0.5, "clip_white": [0.01, 0.03, 0.0], "clip_black": [0.0, 0.0, 0.0] });
        let v = values(Some(&doc));
        assert_eq!(v[0], (FOCUS, Some(json!(0.5))));
        assert_eq!(v[1], (CLIP_WHITE, Some(json!(0.03))));
        assert_eq!(v[2], (CLIP_BLACK, Some(json!(0.0))));
        assert!(
            values(Some(&json!({ "sharpness": 1.0 })))
                .iter()
                .all(|(_, v)| v.is_none())
        );
        assert!(values(None).iter().all(|(_, v)| v.is_none()));
    }

    #[test]
    fn states_say_sharp_soft_clipped_or_unmeasured() {
        assert_eq!(states_of(Some(0.5), Some(0.0), Some(0.0)), [SHARP]);
        assert_eq!(states_of(Some(0.0001), None, None), [SOFT]);
        assert_eq!(states_of(Some(0.5), Some(0.03), None), [SHARP, CLIPPED]);
        assert_eq!(states_of(Some(0.5), None, Some(0.06)), [SHARP, CLIPPED]);
        assert_eq!(states_of(None, None, None), [UNMEASURED]);
    }
}
