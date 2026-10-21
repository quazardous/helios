//! The mask display: a mask node shown on the image (see
//! `docs/design/mask-display.md`).
//!
//! - **What.** The row selected in the Masks panel (a member as it
//!   reaches its group, after its refinements; a group; a refinement),
//!   else the module's final mask: what its blend reads ([`target`]).
//! - **How.** [`Mode::Overlay`] lays the theme's `mask.overlay.color` over
//!   the image by the mask's value, at `mask.overlay.opacity`, the image
//!   under it turned grey by `mask.overlay.desaturate`; [`Mode::Grey`]
//!   shows the mask alone, 0 black and 1 white ([`composite`]).
//! - **One value.** [`MODE`] (`off`, `overlay`, `grey`) is a view value:
//!   transient, never an undo step, never saved with the edit. The
//!   command [`COMMAND`] cycles it (or sets its `mode`), [`COMMAND_OFF`]
//!   turns it off, `M` with the image focused and the Masks panel's
//!   button run the same cycle.
//! - **Rendered** by the viewport's engine like the image: the mask's
//!   node at the image's tiles' regions and scale, usually from the cache
//!   the image's render filled (the blend read it), and composited on the
//!   viewport's render thread before the display transform
//!   ([`crate::engine_source`]).

use helios_core::{Graph, Image, Message, NodeId, Value};
use helios_photo::color;
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, Set, Store};

use crate::mask_tree::{self, Network};

/// The mask display's value.
pub const MODE: &str = "view/viewport/mask_display";
/// Its modes, in the order the command cycles them.
pub const MODES: [&str; 3] = ["off", "overlay", "grey"];
/// Cycles the mask display (off, overlay, grey), or sets `mode`.
pub const COMMAND: &str = "viewport.mask.display";
/// Turns the mask display off.
pub const COMMAND_OFF: &str = "viewport.mask.display.off";

/// How the mask is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The image alone.
    #[default]
    Off,
    /// A colour laid over the image by the mask's value.
    Overlay,
    /// The mask alone, in grey: 0 black, 1 white.
    Grey,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => MODES[0],
            Mode::Overlay => MODES[1],
            Mode::Grey => MODES[2],
        }
    }

    /// A mode by its name (`off`, `overlay`, `grey`; `gray` too).
    pub fn parse(name: &str) -> Option<Mode> {
        match name {
            "off" => Some(Mode::Off),
            "overlay" => Some(Mode::Overlay),
            "grey" | "gray" => Some(Mode::Grey),
            _ => None,
        }
    }

    /// The next one in the cycle: off, overlay, grey, off.
    pub fn next(self) -> Mode {
        match self {
            Mode::Off => Mode::Overlay,
            Mode::Overlay => Mode::Grey,
            Mode::Grey => Mode::Off,
        }
    }

    /// The mode in words, for a hint.
    pub fn words(self) -> Message {
        match self {
            Mode::Off => Message::new("gui-mask-display-off", "off"),
            Mode::Overlay => Message::new("gui-mask-display-overlay", "a coloured overlay"),
            Mode::Grey => Message::new("gui-mask-display-grey", "the mask alone in grey"),
        }
    }
}

/// Declares the mask display's value: transient, as the zoom.
pub fn declare(store: &mut Store) {
    store.declare(
        MODE,
        Descriptor::choice("mask display", &MODES, "off").transient(),
    );
}

/// The mode the store holds.
pub fn mode(store: &Store) -> Mode {
    store
        .get(MODE)
        .and_then(|e| e.value.as_choice().and_then(Mode::parse))
        .unwrap_or_default()
}

/// Sets the mode.
pub fn set(store: &mut Store, mode: Mode, origin: &Origin) -> Result<Mode, String> {
    store
        .set(Set::new(
            MODE,
            Change::To(Value::Choice(mode.name().into())),
            origin.clone(),
        ))
        .map_err(|e| e.to_string())?;
    Ok(mode)
}

/// Moves the mode one step along the cycle: off, overlay, grey, off.
pub fn cycle(store: &mut Store, origin: &Origin) -> Result<Mode, String> {
    let next = mode(store).next();
    set(store, next, origin)
}

/// The value of `--mask-display MODE` among a launch's arguments.
pub fn flag(args: &[String]) -> Result<Option<Mode>, String> {
    let Some(i) = args.iter().position(|a| a == FLAG) else {
        return Ok(None);
    };
    let value = args
        .get(i + 1)
        .ok_or_else(|| format!("{FLAG} takes off, overlay or grey"))?;
    Mode::parse(value)
        .map(Some)
        .ok_or_else(|| format!("{FLAG} {value}: off, overlay or grey"))
}

/// The launch flag setting the mask display (for pictures and benches).
pub const FLAG: &str = "--mask-display";

/// The row of the Masks panel's network the display shows: the selected
/// one, else the module's mask (the root).
pub fn shown_key(net: &Network, store: &Store) -> Option<String> {
    mask_tree::selected(net, store).or_else(|| net.root.clone())
}

/// What the display shows: a row of the mask network and the output
/// port that gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The row's key in the Masks panel.
    pub key: String,
    /// What the panel calls it.
    pub name: String,
    pub node: NodeId,
    pub port: String,
}

/// The mask node shown for the store's selection: the selected row of the
/// selected module's mask network (a member as its group reads it, after
/// its refinements), else that module's final mask (what its blend reads,
/// after the root's refinements). `None` when the module has no mask.
pub fn target(graph: &Graph, store: &Store) -> Option<Target> {
    let net = mask_tree::network(graph, store, None);
    let key = shown_key(&net, store)?;
    let item = net.items.get(&key)?;
    let (node, port) = if net.root.as_deref() == Some(key.as_str()) {
        net.blend
            .as_deref()
            .and_then(|b| graph.find(b))
            .and_then(|b| mask_tree::input_of(graph, b, "mask"))
            .or_else(|| Some((graph.find(&item.node)?, "mask".to_string())))?
    } else if item.is_refinement() {
        (graph.find(&item.node)?, "mask".to_string())
    } else {
        let group = graph.find(item.group.as_deref()?)?;
        graph
            .input_ports(group)
            .ok()?
            .get(item.port)
            .cloned()
            .flatten()?
    };
    Some(Target {
        key,
        name: item.name().to_string(),
        node,
        port,
    })
}

/// How the overlay looks: the theme's `mask.overlay.*` tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Encoded sRGB, 0 to 1.
    pub color: [f32; 3],
    /// The colour's share where the mask is full.
    pub opacity: f32,
    /// How far the image under the overlay turns grey.
    pub desaturate: f32,
}

impl Default for Style {
    /// The defaults of Helios's dark theme, its accent a sun gold.
    fn default() -> Self {
        Style {
            color: [0.96, 0.72, 0.22],
            opacity: 0.6,
            desaturate: 1.0,
        }
    }
}

impl Style {
    /// The style of a theme's tokens.
    pub fn of(extra: &crate::theme::Extra) -> Style {
        let c = extra.mask_overlay;
        Style {
            color: [c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0),
            opacity: extra.mask_overlay_opacity,
            desaturate: extra.mask_overlay_desaturate,
        }
    }
}

/// What the viewport composites over the image: a mask node, a mode
/// (never off) and the overlay's style.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub mode: Mode,
    pub target: Target,
    pub style: Style,
}

/// The working space (linear Rec.2020) to linear sRGB and back.
struct Spaces {
    to_srgb: color::Mat3,
    from_srgb: color::Mat3,
}

impl Spaces {
    fn new() -> Spaces {
        let to_xyz_inv = color::inverse(&color::SRGB_TO_XYZ).expect("invertible primaries");
        let to_srgb = color::mul(&to_xyz_inv, &color::REC2020_TO_XYZ);
        let from_srgb = color::inverse(&to_srgb).expect("invertible primaries");
        Spaces { to_srgb, from_srgb }
    }
}

/// Rec.2020's luminance weights.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// A pixel of the working space with the overlay laid by `mask`: the
/// pixel turned grey by the style's desaturation, then mixed in encoded
/// sRGB (as darktable mixes its yellow) towards the colour by the
/// mask's value times the opacity, back to the working space.
fn overlay_pixel(s: &Spaces, rgb: [f32; 3], mask: f32, style: &Style) -> [f32; 3] {
    let y = LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2];
    let base = rgb.map(|v| f64::from(v + (y - v) * style.desaturate));
    let encoded = color::apply(&s.to_srgb, base).map(|v| color::srgb_encode(v.clamp(0.0, 1.0)));
    let a = f64::from(mask.clamp(0.0, 1.0) * style.opacity);
    let mixed: [f64; 3] = std::array::from_fn(|c| {
        let e = encoded[c];
        color::srgb_decode(e + (f64::from(style.color[c]) - e) * a)
    });
    color::apply(&s.from_srgb, mixed).map(|v| v as f32)
}

/// The grey of a mask value in the working space: on an sRGB display, 0
/// shows black, 1 white and 0.5 the middle grey of the screen's scale.
fn grey_pixel(mask: f32) -> [f32; 3] {
    let v = color::srgb_decode(f64::from(mask.clamp(0.0, 1.0))) as f32;
    [v; 3]
}

/// Lays the mask over RGBA pixels of the working space (four floats a
/// pixel, the alpha kept), `mask` covering the same pixels (its first
/// channel read).
pub fn composite(rgba: &mut [f32], mask: &Image, mode: Mode, style: &Style) {
    if mode == Mode::Off {
        return;
    }
    let spaces = Spaces::new();
    let n = mask.channels() as usize;
    let r = mask.region();
    let values = (r.y..r.bottom()).flat_map(|y| mask.row(y).chunks_exact(n).map(|p| p[0]));
    for (px, m) in rgba.chunks_exact_mut(4).zip(values) {
        let rgb = match mode {
            Mode::Grey => grey_pixel(m),
            _ => overlay_pixel(&spaces, [px[0], px[1], px[2]], m, style),
        };
        px[..3].copy_from_slice(&rgb);
    }
}

#[cfg(test)]
mod tests {
    use helios_core::Region;

    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], what: &str) {
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 1e-4, "{what}: {a:?} against {b:?}");
        }
    }

    #[test]
    fn the_cycle_is_off_overlay_grey_and_names_round_trip() {
        assert_eq!(Mode::Off.next(), Mode::Overlay);
        assert_eq!(Mode::Overlay.next(), Mode::Grey);
        assert_eq!(Mode::Grey.next(), Mode::Off);
        for m in [Mode::Off, Mode::Overlay, Mode::Grey] {
            assert_eq!(Mode::parse(m.name()), Some(m));
        }
        assert_eq!(Mode::parse("gray"), Some(Mode::Grey));
        assert_eq!(Mode::parse("yellow"), None);
    }

    #[test]
    fn the_launch_flag_takes_a_mode() {
        let args = |a: &[&str]| a.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(flag(&args(&["--image", "a.dng"])), Ok(None));
        assert_eq!(
            flag(&args(&["--mask-display", "grey", "--image", "a.dng"])),
            Ok(Some(Mode::Grey))
        );
        assert!(flag(&args(&["--mask-display"])).is_err());
        assert!(flag(&args(&["--mask-display", "loud"])).is_err());
    }

    #[test]
    fn the_value_is_transient_and_cycles() {
        let mut store = Store::new();
        declare(&mut store);
        assert_eq!(mode(&store), Mode::Off);
        assert_eq!(cycle(&mut store, &Origin::User), Ok(Mode::Overlay));
        assert_eq!(cycle(&mut store, &Origin::User), Ok(Mode::Grey));
        assert_eq!(mode(&store), Mode::Grey);
        assert!(store.undo_steps().is_empty(), "never an undo step");
        assert!(store.get(MODE).unwrap().descriptor.transient);
    }

    #[test]
    fn outside_the_mask_the_overlay_leaves_a_grey_image() {
        let s = Spaces::new();
        let style = Style::default();
        // Inside the sRGB gamut, so nothing is clipped on the way.
        let rgb = [0.3, 0.25, 0.2];
        let out = overlay_pixel(&s, rgb, 0.0, &style);
        // Neutral: the image turned grey, its luminance kept.
        let y = LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2];
        close(out, [y; 3], "grey outside");
        // Without desaturation, the pixel as it is.
        let keep = Style {
            desaturate: 0.0,
            ..style
        };
        close(overlay_pixel(&s, rgb, 0.0, &keep), rgb, "kept outside");
    }

    #[test]
    fn inside_the_mask_the_colour_takes_its_opacity() {
        let s = Spaces::new();
        let full = Style {
            opacity: 1.0,
            ..Style::default()
        };
        // At full opacity the colour replaces the pixel: the token's
        // colour, linear and in the working space.
        let want = color::apply(
            &s.from_srgb,
            full.color.map(|v| color::srgb_decode(f64::from(v))),
        )
        .map(|v| v as f32);
        close(overlay_pixel(&s, [0.3, 0.3, 0.3], 1.0, &full), want, "full");
        // Half the mask, half the opacity: in between, encoded.
        let half = overlay_pixel(&s, [0.0; 3], 0.5, &full);
        let back = color::apply(&s.to_srgb, half.map(f64::from));
        for (b, want) in back.iter().zip(full.color) {
            let e = color::srgb_encode(*b);
            assert!((e - f64::from(want) * 0.5).abs() < 1e-4, "{e}");
        }
    }

    #[test]
    fn grey_shows_the_mask_value_on_an_srgb_display() {
        for v in [0.0_f32, 0.25, 0.5, 1.0] {
            let g = grey_pixel(v);
            let shown = crate::viewport::display(g);
            let want = (v * 255.0).round() as u8;
            for c in shown {
                assert!(c.abs_diff(want) <= 1, "{v}: {shown:?}");
            }
        }
        // Out of range values are clamped.
        assert_eq!(grey_pixel(1.5), grey_pixel(1.0));
        assert_eq!(grey_pixel(-0.5), [0.0; 3]);
    }

    #[test]
    fn a_tile_is_composited_pixel_by_pixel_keeping_alpha() {
        let mask = Image::from_fn(Region::sized(2, 1), 1, |x, _, _| x as f32);
        let mut rgba = vec![0.2, 0.2, 0.2, 0.5, 0.2, 0.2, 0.2, 0.5];
        composite(&mut rgba, &mask, Mode::Grey, &Style::default());
        assert_eq!(&rgba[..4], &[0.0, 0.0, 0.0, 0.5]);
        close([rgba[4], rgba[5], rgba[6]], [1.0; 3], "white");
        assert!((rgba[7] - 0.5).abs() < f32::EPSILON);
        // Off leaves the pixels alone.
        let mut same = vec![0.2; 8];
        composite(&mut same, &mask, Mode::Off, &Style::default());
        assert_eq!(same, vec![0.2; 8]);
    }
}
