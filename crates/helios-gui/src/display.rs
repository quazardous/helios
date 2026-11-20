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
//! The route is chosen per window, from the screen the window is on
//! ([`follow_window`], from the pass of the window the image is drawn
//! in): a window found on another screen is said on the bus
//! (`display.window_screen`, [`WindowScreen`]), and the display's
//! listener ([`ROUTE_LISTENER`], also on `screen.added` and
//! `screen.removed`) places it there. On a screen said to show sRGB only
//! (`display/screens/<name>`, [`ScreenColour`]) the window wants sRGB;
//! elsewhere it wants what the setting asks. What each screen shows is
//! read from the system at start and again on `screen.added`
//! ([`SCREENS_LISTENER`]): on Wayland, each output's image description
//! (`helios_display::screens`), sorted by `helios_display::Shows`;
//! a screen the system does not describe stays as it was (unknown at
//! first). Each window drawing the
//! image gets an image surface of its own, negotiated for what that
//! window wants (egui-pop's `image_surface`, per window): the main
//! window's from its handles at install, another window's (a detached
//! viewer, the Compare window) the first frame it draws a wide image,
//! from the handles Helios's event loop pairs with its viewport
//! (`egui_pop::api::runner::host`, read through
//! `image_surface::set_window_source`); it goes when the window closes.
//! A detached panel's window is built transparent where a surface can
//! sit below it (`AppBuilder::transparent_panel_windows`). Per system:
//!
//! - Wayland with colour management (KDE Plasma, sway, Weston: `tagged`;
//!   GNOME, Hyprland: `wsi`): each window gets its own surface, for the
//!   screen it is on;
//! - Wayland without it: sRGB, "no colour management";
//! - X11, Windows, macOS: panel windows stay opaque, the image draws
//!   sRGB in the window's surface, and the route says why (the image
//!   surface is a Wayland subsurface; there is none there).
//!
//! In every case the window's route says why it is not wider
//! (`helios.display`'s `windows`, egui-pop's `windows.list`).
//!
//! The debug bridge's `helios.display` collector says what the display
//! offers (`helios_display::probe`: the session, the compositor's colour
//! management) and what the window resolved (wanted, surface, drawn and
//! why); egui-pop's `viewport` collector, registered here too, gives it
//! per viewport.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use egui_pop::api::bus::bus_contract::PointId;
use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use egui_pop::api::bus::{ScreenAdded, ScreenChange, ScreenRemoved};
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

/// Installs the image's own surface for the main window, and says where
/// other windows' handles are read for theirs: after egui-pop's
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
    let offer = image_surface::install(
        ctx,
        rs,
        window,
        image_surface::Options {
            transparent_window: TRANSPARENT_WINDOW,
        },
    );
    set_window_source(ctx);
    offer
}

/// Says where every other window's handles are read for its own image
/// surface: egui-pop's runner (`egui_pop::api::runner::window`), which
/// Helios's event loop feeds (`crate::event_loop`). A detached panel's
/// window says how it was built (transparent on Wayland, opaque
/// elsewhere and why), which wins over these options; any other window is
/// taken as opaque. [`install_image_surface`] calls it.
pub fn set_window_source(ctx: &egui::Context) {
    image_surface::set_window_source(
        ctx,
        image_surface::Options::default(),
        egui_pop::api::runner::window,
    );
}

/// Takes the image surface of `window` down (the main window's is
/// `ViewportId::ROOT`): its image goes back to the window's surface at
/// once. The viewport wishes sRGB from then on, so egui-pop negotiates no
/// surface and drops the subsurfaces it made.
pub fn take_down_image_surface(ctx: &egui::Context, window: egui::ViewportId) {
    pop_display::set_image_surface(ctx, window, pop_display::ImageSurface::default());
}

/// What the display offers, asked once per process (one connection to
/// the compositor).
pub fn offer() -> &'static helios_display::Offer {
    static OFFER: OnceLock<helios_display::Offer> = OnceLock::new();
    OFFER.get_or_init(helios_display::probe)
}

/// The `helios.display` snapshot: the setting, what the display offers,
/// what the main window resolved, and each window the image was drawn
/// in (`windows`: its screen, what that screen shows, what the window
/// wants there and its route).
pub fn snapshot(ctx: &egui::Context, wide_gamut: bool) -> Json {
    let offer = offer();
    let r = pop_display::resolution(ctx);
    let windows: Vec<Json> = placed(ctx)
        .into_iter()
        .map(|(window, p)| {
            json!({
                "window": window_name(window),
                "screen": p.screen,
                "screen_colour": p.colour,
                "wanted": wanted_on(wide_gamut, p.colour).describe(),
                "route": route_of(ctx, window, wide_gamut, &p),
            })
        })
        .collect();
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
        "route": route(ctx, wide_gamut),
        "windows": windows,
        "expected": helios_display::drawn(offer),
    })
}

/// Where the main window's image goes, as the debug bridge and the
/// doctor say it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ImageRoute {
    /// `tagged` (an image surface Helios tags through the compositor),
    /// `wsi` (an image surface in a colour space the GPU driver declares)
    /// or `srgb` (the window's surface).
    pub route: &'static str,
    /// The encoding the image is drawn in.
    pub encoding: String,
    /// Why not wider, or why less than wanted.
    pub reason: Option<String>,
}

/// Where the main window's image goes now, with the Wide-gamut preview
/// setting `wide_gamut`: what egui-pop's image surface negotiated for the
/// main window, and why.
pub fn route(ctx: &egui::Context, wide_gamut: bool) -> ImageRoute {
    let root = placed(ctx)
        .into_iter()
        .find(|(w, _)| *w == egui::ViewportId::ROOT)
        .map(|(_, p)| p)
        .unwrap_or_default();
    route_of(ctx, egui::ViewportId::ROOT, wide_gamut, &root)
}

/// Where the image of `window` goes now, placed as it is: on a screen
/// said to show sRGB only, sRGB; else what egui-pop's image surface
/// negotiated for that window, and why.
fn route_of(
    ctx: &egui::Context,
    window: egui::ViewportId,
    wide_gamut: bool,
    placed: &Placed,
) -> ImageRoute {
    let srgb = |reason: String| ImageRoute {
        route: "srgb",
        encoding: DisplayTarget::srgb().describe(),
        reason: Some(reason),
    };
    if !wide_gamut {
        return srgb("the Wide-gamut preview setting is off".into());
    }
    if placed.colour == Some(ScreenColour::Srgb) {
        let name = placed.screen.as_deref().unwrap_or("?");
        return srgb(format!("the screen `{name}` shows sRGB only"));
    }
    let image = pop_display::image_surface_of(ctx, window);
    if let Some(t) = image.as_ref().and_then(|i| i.target) {
        return ImageRoute {
            route: if t.source == pop_display::TargetSource::Tagged {
                "tagged"
            } else {
                "wsi"
            },
            encoding: t.describe(),
            reason: image.and_then(|i| i.why),
        };
    }
    // The window's own offer first (a detached viewer's), then the main
    // window's (the platform has none: X11, another system).
    let why = image
        .and_then(|i| i.why)
        .or_else(|| image_surface::offer_of(ctx, window).and_then(|o| o.unavailable))
        .or_else(|| image_surface::offer(ctx).and_then(|o| o.unavailable));
    srgb(why.unwrap_or_else(|| {
        match image_surface::offer(ctx) {
            None => "no image surface: the viewport draws on the CPU, or the GPU path is not \
                 installed yet"
                .into(),
            Some(_) => "the viewport has not drawn the image in this window yet".into(),
        }
    }))
}

// --- Per window: the screen it is on, and the route chosen there ---

/// What a screen shows, as far as Helios is told ([`tell_screen_colour`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenColour {
    /// sRGB only: a wider image is mapped down by the compositor anyway.
    Srgb,
    /// A wide gamut (Display P3, Rec.2020), SDR.
    WideGamut,
    /// High dynamic range (PQ or HLG), wide gamut.
    Hdr,
}

impl ScreenColour {
    /// Its name in `display/screens/<name>`.
    pub fn name(self) -> &'static str {
        match self {
            ScreenColour::Srgb => "srgb",
            ScreenColour::WideGamut => "wide-gamut",
            ScreenColour::Hdr => "hdr",
        }
    }

    /// Parses what `display/screens/<name>` holds.
    pub fn parse(s: &str) -> Option<ScreenColour> {
        match s {
            "srgb" => Some(ScreenColour::Srgb),
            "wide-gamut" => Some(ScreenColour::WideGamut),
            "hdr" => Some(ScreenColour::Hdr),
            _ => None,
        }
    }
}

/// Where what each screen shows is kept: `display/screens/<screen>`, a
/// choice of [`SCREEN_COLOURS`], the screen named as the system names it
/// ([`screen_key`]).
pub const SCREENS_KEY: &str = "display/screens";

/// What a screen may be said to show; `unknown` until told.
pub const SCREEN_COLOURS: [&str; 4] = ["unknown", "srgb", "wide-gamut", "hdr"];

/// The store's path for the screen `name`: its letters and digits, the
/// rest as `_`.
pub fn screen_key(name: &str) -> String {
    let segment: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{SCREENS_KEY}/{segment}")
}

/// Says what the screen `name` shows (none: unknown again), for the
/// windows on it: the platform's probe when it knows, the person when it
/// is wrong. Each window on it follows at its next pass.
///
/// # Errors
/// The store refused the path or the value.
pub fn tell_screen_colour(
    app: &AppHandle,
    name: &str,
    colour: Option<ScreenColour>,
) -> Result<(), String> {
    use egui_pop::model::commands::Origin;
    use egui_pop::model::ids::OwnerId;
    use egui_pop::model::values::{Change, Set, Value};
    let path = screen_key(name);
    let store = app.store();
    let mut store = store
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if store.get(&path).is_none() {
        store
            .declare(
                &OwnerId::from_static("helios.display"),
                &path,
                Descriptor::choice(
                    &format!("What the screen {name} shows"),
                    &SCREEN_COLOURS,
                    "unknown",
                ),
            )
            .map_err(|e| e.to_string())?;
    }
    let value = colour.map_or("unknown", ScreenColour::name);
    store
        .set(Set::new(&path, Change::To(Value::Choice(value.into())), Origin::User).untracked())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

impl From<helios_display::Shows> for ScreenColour {
    fn from(shows: helios_display::Shows) -> ScreenColour {
        match shows {
            helios_display::Shows::Srgb => ScreenColour::Srgb,
            helios_display::Shows::WideGamut => ScreenColour::WideGamut,
            helios_display::Shows::Hdr => ScreenColour::Hdr,
        }
    }
}

/// Says what each screen the system `described` shows
/// ([`tell_screen_colour`]): those with a name and enough of a
/// description to tell ([`helios_display::ScreenDescription::shows`]);
/// the others stay as they were. Returns the screens told, by name.
pub fn tell_described_screens(
    app: &AppHandle,
    described: &[helios_display::ScreenDescription],
) -> Vec<String> {
    let mut told = Vec::new();
    for d in described {
        let (Some(name), Some(shows)) = (d.name.as_deref(), d.shows()) else {
            log::debug!(target: "helios", "display: a screen not told: {d:?}");
            continue;
        };
        match tell_screen_colour(app, name, Some(shows.into())) {
            Ok(()) => told.push(name.to_owned()),
            Err(e) => log::warn!(target: "helios", "display: the screen {name}: {e}"),
        }
    }
    told
}

/// Reads what each screen shows from the system (`read`) and says it
/// ([`tell_described_screens`]); logged when the system could not be
/// asked.
pub fn read_system_screens(
    app: &AppHandle,
    read: fn() -> Result<Vec<helios_display::ScreenDescription>, String>,
) -> Vec<String> {
    match read() {
        Ok(described) => {
            let told = tell_described_screens(app, &described);
            if !told.is_empty() {
                log::info!(target: "helios", "display: the system describes the screens {told:?}");
            }
            told
        }
        Err(e) => {
            log::warn!(target: "helios", "display: the screens' descriptions: {e}");
            Vec::new()
        }
    }
}

/// The display's listener of `screen.added`: what the screens show is
/// read from the system again, a screen plugged in being described.
pub const SCREENS_LISTENER: &str = "helios.display.screen_colour";

/// What the screen `name` shows, when Helios is told; none otherwise
/// (the window's route then follows what the display offers).
pub fn screen_colour(app: &AppHandle, name: &str) -> Option<ScreenColour> {
    app.store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&screen_key(name))
        .and_then(|e| e.value.as_choice().and_then(ScreenColour::parse))
}

/// What a window wants, with the Wide-gamut preview setting `wide_gamut`
/// on a screen showing `colour`: sRGB on a screen said to show sRGB only
/// (a wider image is mapped down there), [`wanted`] otherwise.
pub fn wanted_on(wide_gamut: bool, colour: Option<ScreenColour>) -> DisplayTarget {
    wanted(wide_gamut && colour != Some(ScreenColour::Srgb))
}

/// A window the image is drawn in is now on another screen: said on the
/// bus by the window's own pass, from the place the system gave it
/// (`display.window_screen`); the display's listener ([`ROUTE_LISTENER`])
/// chooses the window's route from it.
#[derive(Debug)]
pub struct WindowScreen;

impl Point for WindowScreen {
    const KIND: PointKind = PointKind::Event;
    const ID: PointId = PointId::new("helios.display.window_screen");
    type Input = OnScreen;
    type Value = ();
}

/// A window and the screen it is on now (none when no screen told by
/// name holds it), and the one it was on.
#[derive(Clone, Debug, PartialEq)]
pub struct OnScreen {
    pub window: egui::ViewportId,
    pub screen: Option<String>,
    pub from: Option<String>,
}

/// The display's points, its contract with the application's bus.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<WindowScreen>(
    "A window the image is drawn in is on another screen: its display route follows",
)];

/// The owner of the display's listeners.
pub const BUS_OWNER: &str = "helios.display";
/// The display's listener of `display.window_screen`, `screen.added` and
/// `screen.removed`: each window's screen, which its route follows.
pub const ROUTE_LISTENER: &str = "helios.display.route";

/// Where a window is, as the bus said it, and what its route was chosen
/// from at its last pass.
#[derive(Clone, Debug, Default, PartialEq)]
struct Placed {
    screen: Option<String>,
    colour: Option<ScreenColour>,
    wide_gamut: bool,
}

#[derive(Default)]
struct WindowsState {
    listening: bool,
    /// The screen each window's own pass last found it on.
    seen: HashMap<egui::ViewportId, Option<String>>,
    /// Each window, as the bus said it.
    placed: HashMap<egui::ViewportId, Placed>,
}

type Shared = Arc<Mutex<WindowsState>>;

fn shared(ctx: &egui::Context) -> Shared {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Shared>(egui::Id::unique("helios.display.windows"))
            .clone()
    })
}

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, WindowsState> {
    s.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Declares [`POINTS`] on `bus` (once).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "display: {refused}");
        }
    }
}

/// Registers [`ROUTE_LISTENER`] on the screen points, once per context.
fn listen(ctx: &egui::Context, bus: &dyn Bus, shared: &Shared) {
    if std::mem::replace(&mut lock(shared).listening, true) {
        return;
    }
    declare(bus);
    let opts = || ListenOpts::new().owner(BUS_OWNER);
    let (s, c) = (shared.clone(), ctx.clone());
    let moved = bus.listen_local::<WindowScreen>(ROUTE_LISTENER, opts(), move |on, ()| {
        log::info!(
            target: "helios",
            "display: a window moved from screen {:?} to {:?}, its route follows",
            on.from,
            on.screen
        );
        lock(&s)
            .placed
            .entry(on.window)
            .or_default()
            .screen
            .clone_from(&on.screen);
        c.request_repaint_of(on.window);
        Ok::<(), HookError>(())
    });
    // A screen gone, or come: every window finds its screen again at its
    // next pass (egui-pop moves those left off-screen first).
    let again = |shared: Shared, ctx: egui::Context| {
        move |_: &ScreenChange, (): &mut ()| {
            let mut s = lock(&shared);
            for w in s.seen.keys() {
                ctx.request_repaint_of(*w);
            }
            s.seen.clear();
            Ok::<(), HookError>(())
        }
    };
    let removed = bus.listen_local::<ScreenRemoved>(
        ROUTE_LISTENER,
        opts(),
        again(shared.clone(), ctx.clone()),
    );
    let added =
        bus.listen_local::<ScreenAdded>(ROUTE_LISTENER, opts(), again(shared.clone(), ctx.clone()));
    for l in [moved, removed, added] {
        if let Err(e) = l {
            log::error!(target: "helios", "display: {e}");
        }
    }
}

/// The image's window follows its screen: called from the pass of the
/// window the image is drawn in. A window found on another screen than
/// at its last pass is said on the bus (`display.window_screen`); returns
/// what the window wants on the screen the bus placed it on, with the
/// Wide-gamut preview setting ([`wanted_on`]).
pub fn follow_window(ctx: &egui::Context, app: &AppHandle) -> DisplayTarget {
    let shared = shared(ctx);
    let bus = app.bus();
    listen(ctx, &*bus, &shared);
    let window = ctx.viewport_id();
    let screen = ctx
        .input(|i| i.viewport().outer_rect)
        .and_then(egui_pop::api::window::screen_name_at);
    let from = match lock(&shared).seen.insert(window, screen.clone()) {
        Some(was) if was == screen => None,
        was => Some(was.flatten()),
    };
    if let Some(from) = from {
        let said = bus.alter::<WindowScreen>(
            OnScreen {
                window,
                screen,
                from,
            },
            (),
        );
        for f in &said.failures {
            log::warn!(target: "helios", "{}: {f}", WindowScreen::ID);
        }
    }
    let wide_gamut = wide_gamut(app);
    let mut s = lock(&shared);
    let placed = s.placed.entry(window).or_default();
    placed.colour = placed
        .screen
        .as_deref()
        .and_then(|name| screen_colour(app, name));
    placed.wide_gamut = wide_gamut;
    wanted_on(wide_gamut, placed.colour)
}

/// The windows the image was drawn in, and how each is placed; the main
/// window first, always there.
fn placed(ctx: &egui::Context) -> Vec<(egui::ViewportId, Placed)> {
    let s = shared(ctx);
    let s = lock(&s);
    let mut all: Vec<_> = s.placed.iter().map(|(w, p)| (*w, p.clone())).collect();
    if !s.placed.contains_key(&egui::ViewportId::ROOT) {
        all.push((egui::ViewportId::ROOT, Placed::default()));
    }
    all.sort_by_key(|(w, _)| (*w != egui::ViewportId::ROOT, w.0.value()));
    all
}

/// A window as the snapshot names it: `main`, or its id.
fn window_name(window: egui::ViewportId) -> String {
    if window == egui::ViewportId::ROOT {
        "main".into()
    } else {
        window.0.short_debug_format()
    }
}

fn collector(app: AppHandle) -> FnCollector {
    FnCollector::new(
        "helios.display",
        "What the display offers (session, the compositor's colour management) and what the image is drawn in.",
    )
    .snapshot(move |ctx| snapshot(ctx, wide_gamut(&app)))
}

/// Declares the display's settings and its debug collectors, and says
/// what each screen shows as the system describes it (`read`, at start
/// and on `screen.added`).
pub struct Display {
    read: fn() -> Result<Vec<helios_display::ScreenDescription>, String>,
}

impl Default for Display {
    /// The system's descriptions (`helios_display::screens`).
    fn default() -> Display {
        Display::reading(helios_display::screens)
    }
}

impl Display {
    /// The screens' descriptions read by `read` (a test's fake system).
    pub fn reading(
        read: fn() -> Result<Vec<helios_display::ScreenDescription>, String>,
    ) -> Display {
        Display { read }
    }
}

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
        declare(&*ctx.app().bus());
        let (app, read) = (ctx.app(), self.read);
        read_system_screens(&app, read);
        let listening = ctx.app().bus().listen_local::<ScreenAdded>(
            SCREENS_LISTENER,
            ListenOpts::new().owner(BUS_OWNER),
            move |_: &ScreenChange, (): &mut ()| {
                read_system_screens(&app, read);
                Ok::<(), HookError>(())
            },
        );
        if let Err(e) = listening {
            log::error!(target: "helios", "display: {e}");
        }
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
        assert!(s["expected"]["route"].is_string());
        assert_eq!(s["route"]["route"], "srgb");
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
        let r = route(&ctx, false);
        assert_eq!(r.route, "srgb");
        assert!(r.reason.unwrap().contains("setting is off"));
        // An sRGB wish stays on the window even where the compositor
        // would tag a surface.
        assert_eq!(
            image_surface::negotiate(&wanted(false), &kwin()).route,
            Route::Window
        );
    }

    #[test]
    fn with_the_setting_on_the_target_follows_the_route() {
        for (offer, name, primaries) in [
            (kwin(), "tagged", Primaries::Bt2020),
            (gnome(), "wsi", Primaries::DisplayP3),
        ] {
            let ctx = egui::Context::default();
            pop_display::want(&ctx, ViewportId::ROOT, wanted(true));
            let r = negotiated(&ctx, &offer);
            assert_eq!(r.name(), name, "{r:?}");
            let t = pop_display::target(&ctx);
            assert_eq!(Some(t), r.target());
            assert_eq!(t.primaries, primaries);
            assert!(t.is_supported(), "{t:?}");
            // Helios's transform encodes for that target, not sRGB.
            assert_eq!(HeliosDisplay.stage(&t), display_stage(&t));
            // The debug bridge and the doctor say it.
            let shown = route(&ctx, true);
            assert_eq!(shown.route, name);
            assert_eq!(shown.encoding, t.describe());
            assert_eq!(snapshot(&ctx, true)["route"]["route"], name);

            // The setting goes off: the image is back on the window.
            take_down_image_surface(&ctx, ViewportId::ROOT);
            pop_display::want(&ctx, ViewportId::ROOT, wanted(false));
            assert!(is_srgb(&pop_display::target(&ctx)));
            assert_eq!(negotiated(&ctx, &offer), Route::Window);
            assert!(is_srgb(&pop_display::target(&ctx)));
            assert_eq!(route(&ctx, false).route, "srgb");
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
            let shown = route(&ctx, true);
            assert_eq!(shown.route, "srgb");
            assert!(shown.reason.unwrap().contains(why));
        }
    }
}
