//! What the image is drawn in: the Wide-gamut preview setting, off by
//! default, and what the viewport's window wants from it.
//!
//! Off, the window wants sRGB, as every surface shows it. On, it wants
//! Rec.2020 primaries with a 2.2 power (the encoding a Wayland compositor
//! with the colour-management protocol maps to the screen itself); the
//! viewport draws it when the window's surface carries it, and sRGB
//! otherwise, with the reason (egui-pop's `display::resolve`). Only the
//! image asks for it: egui's interface stays sRGB.
//!
//! On Wayland the image gets a surface of its own while the setting is
//! on ([`ImageSurfaceSwitch`]): egui-pop's `image_surface`, below the
//! transparent main window, tagged through the compositor's colour
//! management or presented in a colour space the driver declares; the
//! viewport then draws for that surface's target.
//!
//! The debug bridge's `helios.display` collector says what the display
//! offers (`helios_display::probe`: the session, the compositor's colour
//! management) and what the window resolved (wanted, surface, drawn and
//! why); egui-pop's `viewport` collector, registered here too, gives it
//! per viewport.

use std::sync::OnceLock;

use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::core::debug::FnCollector;
use egui_pop::model::ids::{PluginId, SettingId};
use egui_pop::model::values::Descriptor;
use egui_pop::viewport::display::{self as pop_display, DisplayTarget};
use egui_pop::viewport::image_surface;
use serde_json::{Value as Json, json};

use crate::viewport::rec2020_gamma22;

/// The main window is built transparent (`ViewportBuilder::with_transparent`):
/// the image's own surface on Wayland (egui-pop's `image_surface`) sits
/// below the window and shows through a hole the viewport punches in the
/// interface. Everything else stays opaque ([`CLEAR_COLOR`]).
pub const TRANSPARENT_WINDOW: bool = true;

/// What the window is cleared to before the interface is drawn: eframe's
/// own dark grey, opaque, so the transparent window looks exactly as the
/// opaque one did (eframe's default is translucent).
pub const CLEAR_COLOR: egui::Color32 = egui::Color32::from_rgb(12, 12, 12);

pub const PLUGIN: PluginId = PluginId::from_static("helios.display");

/// The Wide-gamut preview setting, at `settings/display.wide-gamut-preview`.
pub const SETTING_WIDE_GAMUT: SettingId = SettingId::from_static("display.wide-gamut-preview");

/// Whether the Wide-gamut preview setting is on.
pub fn wide_gamut(app: &AppHandle) -> bool {
    app.store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&format!("settings/{SETTING_WIDE_GAMUT}"))
        .and_then(|e| e.value.as_bool())
        .unwrap_or(false)
}

/// What the viewport's window wants: Rec.2020 with gamma 2.2 for a
/// wide-gamut preview, sRGB otherwise.
pub fn wanted(wide_gamut: bool) -> DisplayTarget {
    if wide_gamut {
        rec2020_gamma22()
    } else {
        DisplayTarget::srgb()
    }
}

/// The image's own surface (egui-pop's `image_surface`, Wayland) as it
/// follows the Wide-gamut preview setting: installed after the GPU path
/// the first frame the setting is on, taken down when it goes off, and
/// back up when it comes on again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageSurfaceSwitch {
    installed: bool,
    up: bool,
}

/// What a frame does with the image surface ([`ImageSurfaceSwitch::step`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceStep {
    /// Nothing changes.
    Keep,
    /// Install it ([`install_image_surface`]): the setting is on and the
    /// GPU path installed, for the first time.
    Install,
    /// The setting came back on: the installed surface serves again (the
    /// viewport wishes Rec.2020 again and egui-pop negotiates).
    Up,
    /// The setting went off: take it down ([`take_down_image_surface`]).
    Down,
}

impl ImageSurfaceSwitch {
    /// What this frame does, with the setting `wide_gamut` and the
    /// viewport's GPU path installed or not (the CPU path has no image
    /// surface).
    pub fn step(&mut self, wide_gamut: bool, gpu: bool) -> SurfaceStep {
        let wanted = wide_gamut && gpu;
        match (wanted, self.installed, self.up) {
            (true, false, _) => {
                self.installed = true;
                self.up = true;
                SurfaceStep::Install
            }
            (true, true, false) => {
                self.up = true;
                SurfaceStep::Up
            }
            (false, true, true) => {
                self.up = false;
                SurfaceStep::Down
            }
            _ => SurfaceStep::Keep,
        }
    }

    /// Whether the image surface was installed.
    pub fn installed(&self) -> bool {
        self.installed
    }

    /// Whether it serves now (installed, the setting on).
    pub fn up(&self) -> bool {
        self.up
    }
}

/// Installs the image's own surface for the main window: after egui-pop's
/// `gpu::install`, on the same render state, with the window's handles
/// (eframe's frame). Returns what the platform offers; why there is none
/// in `unavailable` (X11, no colour management, another system).
pub fn install_image_surface(
    ctx: &egui::Context,
    rs: &eframe::egui_wgpu::RenderState,
    window: &(
         impl image_surface::raw_window_handle::HasWindowHandle
         + image_surface::raw_window_handle::HasDisplayHandle
     ),
) -> image_surface::Offer {
    image_surface::install(
        ctx,
        rs,
        window,
        image_surface::Options {
            transparent_window: TRANSPARENT_WINDOW,
        },
    )
}

/// Takes the image surface down: the main window's image goes back to
/// the window's surface at once. The viewport wishes sRGB from then on,
/// so egui-pop negotiates no surface and drops the subsurfaces it made.
pub fn take_down_image_surface(ctx: &egui::Context) {
    pop_display::set_image_surface(
        ctx,
        egui::ViewportId::ROOT,
        pop_display::ImageSurface::default(),
    );
}

/// What the display offers, asked once per process (one connection to
/// the compositor).
pub fn offer() -> &'static helios_display::Offer {
    static OFFER: OnceLock<helios_display::Offer> = OnceLock::new();
    OFFER.get_or_init(helios_display::probe)
}

/// The `helios.display` snapshot: the setting, what the display offers,
/// and what the main window resolved.
pub fn snapshot(ctx: &egui::Context, wide_gamut: bool) -> Json {
    let offer = offer();
    let r = pop_display::resolution(ctx);
    json!({
        "wide_gamut_preview": wide_gamut,
        "session": offer.session,
        "color_management": offer.color_management,
        "error": offer.error,
        "resolution": {
            "wanted": r.wanted.describe(),
            "surface": r.surface.describe(),
            "drawn": r.drawn.describe(),
            "faithful": r.faithful(),
            "reason": r.reason(),
        },
        "expected": helios_display::drawn(offer),
    })
}

fn collector(app: AppHandle) -> FnCollector {
    FnCollector::new(
        "helios.display",
        "What the display offers (session, the compositor's colour management) and what the image is drawn in.",
    )
    .snapshot(move |ctx| snapshot(ctx, wide_gamut(&app)))
}

/// Declares the display's settings and its debug collectors.
pub struct Display;

impl Plugin for Display {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Display".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        ctx.setting(
            SETTING_WIDE_GAMUT,
            Descriptor::bool("Wide-gamut preview", false).description(
                "Shows the image in Rec.2020 where the window can carry it (Wayland with colour management), so colours outside sRGB are not clipped. Elsewhere the image stays sRGB.",
            ),
        )?;
        ctx.collector(collector(ctx.app()))?;
        ctx.collector(egui_pop::viewport::debug::collector())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_says_what_is_drawn_and_why() {
        let ctx = egui::Context::default();
        pop_display::want(&ctx, egui::ViewportId::ROOT, wanted(true));
        let s = snapshot(&ctx, true);
        assert_eq!(s["wide_gamut_preview"], true);
        // No surface configured for it: sRGB, with the reason.
        assert_eq!(s["resolution"]["faithful"], false);
        assert!(s["resolution"]["drawn"].as_str().unwrap().contains("sRGB"));
        assert!(s["resolution"]["reason"].is_string());
        assert_eq!(s["expected"]["encoding"], "sRGB");
    }

    #[test]
    fn the_transparent_window_is_cleared_opaque() {
        const { assert!(TRANSPARENT_WINDOW) };
        assert_eq!(CLEAR_COLOR.a(), 255);
        assert!((CLEAR_COLOR.to_normalized_gamma_f32()[3] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn off_wants_srgb_and_on_wants_rec2020() {
        assert_eq!(wanted(false), DisplayTarget::srgb());
        assert_eq!(wanted(true), rec2020_gamma22());
    }

    // --- The image surface, with fake compositors ---

    use egui::ViewportId;
    use egui_pop::viewport::display::{ColorSpace, DisplayTransform, Primaries, SurfaceFormat};
    use egui_pop::viewport::image_surface::{Offer, Route};
    use egui_pop_wayland::{Capabilities, Feature, NamedPrimaries as P, NamedTransfer as T};

    use crate::viewport::{HeliosDisplay, display_stage};

    /// A compositor's announcements and what a Mesa driver offers a
    /// subsurface on it (8 and 10 bits and half floats, in sRGB and
    /// `wsi`).
    fn compositor(primaries: &[P], transfers: &[T], wsi: &[ColorSpace]) -> Offer {
        let mut pairs = Vec::new();
        for f in [
            SurfaceFormat::Rgba8Unorm,
            SurfaceFormat::Rgb10a2Unorm,
            SurfaceFormat::Rgba16Float,
        ] {
            pairs.push((f, ColorSpace::Srgb));
            pairs.extend(wsi.iter().map(|&s| (f, s)));
        }
        Offer {
            unavailable: None,
            colour: Some(Capabilities {
                version: 1,
                features: vec![Feature::Parametric],
                primaries: primaries.to_vec(),
                transfers: transfers.to_vec(),
                intents: vec![0],
            }),
            wsi: pairs,
            may_tag: true,
        }
    }

    /// A `KWin`-like compositor: no sRGB transfer announced, so the image surface is tagged.
    fn kwin() -> Offer {
        compositor(
            &[P::Srgb, P::Bt2020, P::DisplayP3],
            &[T::Gamma22, T::St2084Pq],
            &[ColorSpace::ExtendedSrgbLinear],
        )
    }

    /// GNOME: sRGB with the sRGB transfer announced, so the driver holds
    /// the surface's colour and declares Display P3.
    fn gnome() -> Offer {
        compositor(
            &[P::Srgb, P::Bt2020, P::DisplayP3],
            &[T::Gamma22, T::Srgb, T::St2084Pq],
            &[ColorSpace::DisplayP3],
        )
    }

    /// What egui-pop's viewport does each frame once the image surface is
    /// installed: what the window wishes, negotiated against the offer,
    /// and the image surface's target recorded.
    fn negotiated(ctx: &egui::Context, offer: &Offer) -> Route {
        let n = image_surface::negotiate(&pop_display::wanted(ctx), offer);
        pop_display::set_image_surface(
            ctx,
            ViewportId::ROOT,
            pop_display::ImageSurface {
                target: n.route.target(),
                why: n.why,
            },
        );
        n.route
    }

    fn is_srgb(t: &DisplayTarget) -> bool {
        t.primaries == Primaries::Srgb && !t.transfer.is_hdr() && t.headroom <= 1.0
    }

    #[test]
    fn the_switch_installs_once_after_the_gpu_path() {
        let mut s = ImageSurfaceSwitch::default();
        // On, but the viewport draws on the CPU for now.
        assert_eq!(s.step(true, false), SurfaceStep::Keep);
        assert!(!s.installed());
        assert_eq!(s.step(true, true), SurfaceStep::Install);
        assert_eq!(s.step(true, true), SurfaceStep::Keep);
        assert_eq!(s.step(false, true), SurfaceStep::Down);
        assert_eq!(s.step(false, true), SurfaceStep::Keep);
        assert!(s.installed() && !s.up());
        assert_eq!(s.step(true, true), SurfaceStep::Up);
        assert!(s.up());
    }

    #[test]
    fn with_the_setting_off_the_target_stays_srgb_and_no_surface_is_installed() {
        let ctx = egui::Context::default();
        let mut s = ImageSurfaceSwitch::default();
        for gpu in [false, true, true] {
            assert_eq!(s.step(false, gpu), SurfaceStep::Keep);
        }
        assert!(!s.installed());
        pop_display::want(&ctx, ViewportId::ROOT, wanted(false));
        assert!(image_surface::offer(&ctx).is_none());
        assert!(pop_display::image_surface_of(&ctx, ViewportId::ROOT).is_none());
        assert!(is_srgb(&pop_display::target(&ctx)));
        // An sRGB wish stays on the window even where the compositor
        // would tag a surface.
        assert_eq!(
            image_surface::negotiate(&wanted(false), &kwin()).route,
            Route::Window
        );
    }

    #[test]
    fn with_the_setting_on_the_target_follows_the_route() {
        for (offer, route, primaries) in [
            (kwin(), "tagged", Primaries::Bt2020),
            (gnome(), "wsi", Primaries::DisplayP3),
        ] {
            let ctx = egui::Context::default();
            pop_display::want(&ctx, ViewportId::ROOT, wanted(true));
            let r = negotiated(&ctx, &offer);
            assert_eq!(r.name(), route, "{r:?}");
            let t = pop_display::target(&ctx);
            assert_eq!(Some(t), r.target());
            assert_eq!(t.primaries, primaries);
            assert!(t.is_supported(), "{t:?}");
            // Helios's transform encodes for that target, not sRGB.
            assert_eq!(HeliosDisplay.stage(&t), display_stage(&t));

            // The setting goes off: the image is back on the window.
            take_down_image_surface(&ctx);
            pop_display::want(&ctx, ViewportId::ROOT, wanted(false));
            assert!(is_srgb(&pop_display::target(&ctx)));
            assert_eq!(negotiated(&ctx, &offer), Route::Window);
            assert!(is_srgb(&pop_display::target(&ctx)));
        }
    }

    #[test]
    fn without_colour_management_the_image_stays_srgb_and_says_why() {
        let mut no_cm = kwin();
        no_cm.colour = None;
        no_cm.wsi.retain(|(_, s)| *s == ColorSpace::Srgb);
        for (offer, why) in [
            (no_cm, "colour management"),
            (
                Offer::unavailable("not a Wayland window (X11 or another system)"),
                "not a Wayland window",
            ),
        ] {
            let ctx = egui::Context::default();
            pop_display::want(&ctx, ViewportId::ROOT, wanted(true));
            assert_eq!(negotiated(&ctx, &offer), Route::Window);
            assert!(is_srgb(&pop_display::target(&ctx)));
            let image = pop_display::image_surface_of(&ctx, ViewportId::ROOT).unwrap();
            assert!(image.why.unwrap().contains(why));
        }
    }
}
