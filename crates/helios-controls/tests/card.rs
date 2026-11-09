//! Cards laid out on the slot engine: a text dump at a few widths, the
//! placements mapped back to controls and chrome.

use helios_controls::card::{self, Card, CardItem, ItemKind, Rack, SlotStatus};
use helios_controls::catalogue::EXPOSURE;
use helios_controls::model::{Case, Representation, Surface};
use helios_controls::slot::{Chrome, ChromeKind, Orientation};
use helios_controls::{Registry, implicit};
use helios_core::{ParamKind, ParamSpec};
use helios_values::Descriptor;

/// Panel columns of 240 points, rows of 24.
const COLUMN: f32 = 240.0;
const ROW: f32 = 24.0;

/// The registry with exposure and two implicit controls (a toggle and a
/// slider) registered under their own ids.
fn registry() -> Registry {
    let mut r = Registry::standard();
    let toggle = Descriptor::from_spec(
        &ParamSpec {
            name: "clip".into(),
            description: "clip highlights".into(),
            kind: ParamKind::Bool { default: false },
        },
        None,
    );
    r.register(implicit("node/exp/clip", &toggle)).unwrap();
    let black = Descriptor::from_spec(
        &ParamSpec::float("black", "black level correction", -1.0, 1.0, 0.0),
        None,
    );
    r.register(implicit("node/exp/black", &black)).unwrap();
    r
}

/// The exposure module's card: a legend, exposure, black level, a toggle,
/// a separator and the scrub variant of exposure again.
fn card() -> Card {
    Card::new(vec![
        CardItem::Chrome(Chrome::new(ChromeKind::Legend).text("Exposure")),
        CardItem::control("exposure", EXPOSURE),
        CardItem::control("black", "implicit:node/exp/black"),
        CardItem::control("clip", "implicit:node/exp/clip"),
        CardItem::Chrome(Chrome::new(ChromeKind::Separator)),
        CardItem::Control {
            key: "quick".into(),
            control: EXPOSURE.into(),
            variant: Some("scrub".into()),
            orientation: None,
            pin: None,
        },
    ])
}

fn dump(columns: f32) -> String {
    let rack = Rack::new(Surface::Panel, columns * COLUMN, COLUMN, ROW);
    card::layout(&registry(), &card(), rack, None)
        .unwrap()
        .dump()
}

#[test]
fn the_exposure_card_at_three_widths() {
    let one = dump(1.0);
    let two = dump(2.0);
    let half = dump(0.5);
    insta_like(
        &one,
        "grid 12x45 twelfths, one char = 6x3\n",
        &[
            "a chrome:legend#0 1x1/2 at 0,0",
            "b exposure full horizontal 1x1 at 0,6",
            "c black full horizontal 1x1 at 0,18",
            "d clip full horizontal 1/2x1 at 0,30",
            "e chrome:separator#4 1x1/4 at 0,42",
            "f quick full horizontal 1/2x1 at 6,30",
        ],
    );
    insta_like(
        &two,
        "",
        &[
            "b exposure full horizontal 2x1 at 0,6",
            "e chrome:separator#4 2x1/4 at 0,30",
        ],
    );
    insta_like(
        &half,
        "",
        &[
            "b exposure compact horizontal 1/2x1 at 0,6",
            "f quick full horizontal 1/2x1 at 0,",
        ],
    );
}

/// Checks the dump's first line and that each expected line is in it,
/// printing the whole dump when not.
fn insta_like(dump: &str, header: &str, lines: &[&str]) {
    assert!(dump.starts_with(header), "{dump}");
    for l in lines {
        assert!(
            dump.lines().any(|d| d.starts_with(l)),
            "missing `{l}` in\n{dump}"
        );
    }
}

#[test]
fn placements_map_back_to_items() {
    let r = registry();
    let rack = Rack::new(Surface::Panel, 2.0 * COLUMN, COLUMN, ROW);
    let l = card::layout(&r, &card(), rack, None).unwrap();
    assert_eq!(l.slots.len(), 6);
    let exp = l.get("exposure").unwrap();
    assert_eq!(exp.kind, ItemKind::Control);
    assert_eq!(exp.control.as_deref(), Some(EXPOSURE));
    assert_eq!(exp.variant.as_deref(), Some("slider"));
    assert_eq!(exp.representation, Some(Representation::Full));
    assert_eq!(exp.orientation, Orientation::Horizontal);
    assert_eq!(exp.status, SlotStatus::Placed);
    let rect = exp.rect.unwrap();
    assert_eq!((rect.x, rect.y, rect.w, rect.h), (0.0, 12.0, 480.0, 24.0));
    let legend = l.get("chrome:legend#0").unwrap();
    assert_eq!(legend.kind, ItemKind::Chrome);
    assert_eq!(legend.representation, None);
    assert!(l.height() > 0.0);
    // As data, for agents.
    let j = serde_json::to_value(exp).unwrap();
    assert_eq!(j["status"], "placed");
    assert_eq!(j["representation"], "full");
    assert_eq!(j["integration"]["level"], "managed");
    // The same layout again, with the previous one: nothing moves.
    let again = card::layout(&r, &card(), rack, Some(&l)).unwrap();
    assert_eq!(again.slots, l.slots);
}

#[test]
fn a_case_lays_out_its_parts_and_chrome() {
    let case = Case::default()
        .chrome(Chrome::new(ChromeKind::Legend).text("Tone"))
        .part("exposure", EXPOSURE)
        .chrome(Chrome::new(ChromeKind::Help).text("Brightens the image"));
    assert_eq!(case.parts().count(), 1);
    let card = Card::from_case(&case);
    let rack = Rack::new(Surface::Dock, COLUMN, COLUMN, ROW);
    let l = card::layout(&registry(), &card, rack, None).unwrap();
    let kinds: Vec<_> = l.slots.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [ItemKind::Chrome, ItemKind::Control, ItemKind::Chrome]
    );
    assert!(l.slots.iter().all(|s| s.status == SlotStatus::Placed));
}

#[test]
fn bad_cards_are_refused() {
    let r = registry();
    let rack = Rack::new(Surface::Panel, COLUMN, COLUMN, ROW);
    let unknown = Card::new(vec![CardItem::control("x", "feather")]);
    assert!(card::layout(&r, &unknown, rack, None).is_err());
    let bad_variant = Card::new(vec![CardItem::Control {
        key: "x".into(),
        control: EXPOSURE.into(),
        variant: Some("knob".into()),
        orientation: None,
        pin: None,
    }]);
    assert!(card::layout(&r, &bad_variant, rack, None).is_err());
    let hud = Rack::new(Surface::Hud, COLUMN, COLUMN, ROW);
    assert!(
        card::layout(&r, &card(), hud, None).is_err(),
        "no slots in the HUD"
    );
}

#[test]
fn a_too_narrow_rack_overflows_without_panicking() {
    let rack = Rack::new(Surface::Toolbar, 0.2 * COLUMN, COLUMN, ROW);
    let l = card::layout(&registry(), &card(), rack, None).unwrap();
    let exp = l.get("exposure").unwrap();
    assert!(
        matches!(exp.status, SlotStatus::Overflow(_)),
        "{}",
        l.dump()
    );
    assert!(exp.rect.is_none());
}
