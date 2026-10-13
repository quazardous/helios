//! What the display offers Helios: the session (Wayland, X11 or none)
//! and, on Wayland, what the compositor's colour management
//! (`wp_color_management_v1`) supports: its features, named primaries,
//! transfer functions and rendering intents. [`drawn`] says what the
//! desktop's viewport draws on it today, and why.
//!
//! `helios doctor display` prints it; the desktop's debug bridge shows it
//! beside what the viewport actually resolved.

use serde::Serialize;

#[cfg(target_os = "linux")]
mod wayland;

/// The kind of display session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Session {
    Wayland,
    X11,
    /// No display server found (a console, a test box), or a platform
    /// other than Linux.
    None,
}

/// What the compositor's `wp_color_manager_v1` announced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ColorManagement {
    /// The interface's version the compositor offers.
    pub version: u32,
    pub features: Vec<String>,
    /// Named primaries a surface may be tagged with.
    pub primaries: Vec<String>,
    /// Named transfer functions a surface may be tagged with.
    pub transfer_functions: Vec<String>,
    pub intents: Vec<String>,
}

impl ColorManagement {
    /// Whether a surface can be tagged Rec.2020 with gamma 2.2: a
    /// parametric description with the named `bt2020` primaries and the
    /// `gamma22` transfer function.
    pub fn offers_rec2020_gamma22(&self) -> bool {
        let has = |list: &[String], name: &str| list.iter().any(|v| v == name);
        has(&self.features, "parametric")
            && has(&self.primaries, "bt2020")
            && has(&self.transfer_functions, "gamma22")
    }
}

/// What the display offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Offer {
    pub session: Session,
    /// The compositor's colour management, when it announces it.
    pub color_management: Option<ColorManagement>,
    /// Why the compositor could not be asked, if it could not.
    pub error: Option<String>,
}

/// The session from the environment's `WAYLAND_DISPLAY` and `DISPLAY`.
pub fn session_from(wayland_display: Option<&str>, display: Option<&str>) -> Session {
    let set = |v: Option<&str>| v.is_some_and(|v| !v.is_empty());
    if !cfg!(target_os = "linux") {
        Session::None
    } else if set(wayland_display) {
        Session::Wayland
    } else if set(display) {
        Session::X11
    } else {
        Session::None
    }
}

/// Asks the display what it offers: on Wayland, one connection to the
/// compositor, its colour manager bound, its announcements read, then
/// closed.
pub fn probe() -> Offer {
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    let display = std::env::var("DISPLAY").ok();
    let session = session_from(wayland_display.as_deref(), display.as_deref());
    let mut offer = Offer {
        session,
        color_management: None,
        error: None,
    };
    if session == Session::Wayland {
        match color_management() {
            Ok(cm) => offer.color_management = cm,
            Err(e) => offer.error = Some(e),
        }
    }
    offer
}

#[cfg(target_os = "linux")]
fn color_management() -> Result<Option<ColorManagement>, String> {
    wayland::color_management()
}

#[cfg(not(target_os = "linux"))]
fn color_management() -> Result<Option<ColorManagement>, String> {
    Ok(None)
}

impl ColorManagement {
    /// Whether the compositor announces the `srgb` primaries with the
    /// `srgb` transfer function: a Vulkan driver (Mesa 25.1 and later)
    /// then holds the colour of every surface it presents to, so the
    /// image surface is not tagged by Helios but presented in a colour
    /// space the driver declares (GNOME, Hyprland).
    pub fn driver_holds_srgb_surfaces(&self) -> bool {
        self.primaries.iter().any(|p| p == "srgb")
            && self.transfer_functions.iter().any(|t| t == "srgb")
    }
}

/// Where the desktop's image goes on a display, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Drawn {
    /// `tagged` (a surface of the image's own, tagged by Helios through
    /// the compositor), `wsi` (one presented in a colour space the GPU
    /// driver declares) or `srgb` (the window's surface).
    pub route: &'static str,
    /// The encoding the image is drawn in.
    pub encoding: String,
    pub reason: String,
}

/// Where the desktop's image goes on this display with the Wide-gamut
/// preview setting on, predicted from the compositor's announcements
/// alone (the GPU driver's colour spaces are known only to the running
/// desktop, which reports what it negotiated through its debug bridge).
pub fn drawn(offer: &Offer) -> Drawn {
    let srgb = |reason: String| Drawn {
        route: "srgb",
        encoding: "sRGB".into(),
        reason,
    };
    match (offer.session, &offer.color_management) {
        (Session::None, _) => srgb("no display session (or not Linux): sRGB".into()),
        (Session::X11, _) => srgb("X11 has no colour management protocol: sRGB".into()),
        (Session::Wayland, None) => srgb(match &offer.error {
            Some(e) => format!("the compositor could not be asked ({e}): sRGB"),
            None => "the compositor does not offer wp_color_management_v1: sRGB".into(),
        }),
        (Session::Wayland, Some(cm)) if cm.driver_holds_srgb_surfaces() => Drawn {
            route: "wsi",
            encoding: "a colour space the GPU driver declares (Display P3 with Mesa)".into(),
            reason: "the compositor announces sRGB with the sRGB transfer, so the GPU driver \
                     holds the image surface's colour: the image is presented in a colour space \
                     it declares, or sRGB if it declares none wider"
                .into(),
        },
        (Session::Wayland, Some(cm)) if cm.offers_rec2020_gamma22() => Drawn {
            route: "tagged",
            encoding: "BT.2020 primaries, gamma 2.2 transfer, SDR".into(),
            reason: "the compositor takes Rec.2020 with a 2.2 power: the image surface is \
                     tagged with it"
                .into(),
        },
        (Session::Wayland, Some(_)) => srgb(
            "the compositor's colour management offers no Rec.2020 with gamma 2.2: sRGB, \
             unless the GPU driver declares a wider colour space for the image surface"
                .into(),
        ),
    }
}

/// A feature of `wp_color_manager_v1`, by its protocol name.
pub fn feature_name(value: u32) -> String {
    named(
        value,
        &[
            (0, "icc_v2_v4"),
            (1, "parametric"),
            (2, "set_primaries"),
            (3, "set_tf_power"),
            (4, "set_luminances"),
            (5, "set_mastering_display_primaries"),
            (6, "extended_target_volume"),
            (7, "windows_scrgb"),
            (8, "windows_bt2100"),
        ],
    )
}

/// Named primaries of `wp_color_manager_v1`.
pub fn primaries_name(value: u32) -> String {
    named(
        value,
        &[
            (1, "srgb"),
            (2, "pal_m"),
            (3, "pal"),
            (4, "ntsc"),
            (5, "generic_film"),
            (6, "bt2020"),
            (7, "cie1931_xyz"),
            (8, "dci_p3"),
            (9, "display_p3"),
            (10, "adobe_rgb"),
        ],
    )
}

/// A named transfer function of `wp_color_manager_v1`.
pub fn transfer_name(value: u32) -> String {
    named(
        value,
        &[
            (1, "bt1886"),
            (2, "gamma22"),
            (3, "gamma28"),
            (4, "st240"),
            (5, "ext_linear"),
            (6, "log_100"),
            (7, "log_316"),
            (8, "xvycc"),
            (9, "srgb"),
            (10, "ext_srgb"),
            (11, "st2084_pq"),
            (12, "st428"),
            (13, "hlg"),
            (14, "compound_power_2_4"),
        ],
    )
}

/// A rendering intent of `wp_color_manager_v1`.
pub fn intent_name(value: u32) -> String {
    named(
        value,
        &[
            (0, "perceptual"),
            (1, "relative"),
            (2, "saturation"),
            (3, "absolute"),
            (4, "relative_bpc"),
            (5, "absolute_no_adaptation"),
        ],
    )
}

fn named(value: u32, names: &[(u32, &str)]) -> String {
    names
        .iter()
        .find(|(v, _)| *v == value)
        .map_or_else(|| format!("unknown({value})"), |(_, n)| (*n).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(cm: Option<ColorManagement>) -> Offer {
        Offer {
            session: Session::Wayland,
            color_management: cm,
            error: None,
        }
    }

    fn kwin_like() -> ColorManagement {
        ColorManagement {
            version: 1,
            features: vec!["parametric".into(), "set_primaries".into()],
            primaries: vec![primaries_name(1), primaries_name(6)],
            transfer_functions: vec![transfer_name(2), transfer_name(11)],
            intents: vec![intent_name(0)],
        }
    }

    #[test]
    fn the_session_follows_the_environment() {
        if cfg!(target_os = "linux") {
            assert_eq!(
                session_from(Some("wayland-0"), Some(":0")),
                Session::Wayland
            );
            assert_eq!(session_from(Some(""), Some(":0")), Session::X11);
            assert_eq!(session_from(None, None), Session::None);
        } else {
            assert_eq!(session_from(Some("wayland-0"), None), Session::None);
        }
    }

    #[test]
    fn protocol_values_get_their_names() {
        assert_eq!(primaries_name(6), "bt2020");
        assert_eq!(transfer_name(2), "gamma22");
        assert_eq!(transfer_name(11), "st2084_pq");
        assert_eq!(feature_name(1), "parametric");
        assert_eq!(intent_name(1), "relative");
        assert_eq!(primaries_name(99), "unknown(99)");
    }

    #[test]
    fn rec2020_gamma22_needs_parametric_bt2020_and_gamma22() {
        assert!(kwin_like().offers_rec2020_gamma22());
        let mut no_gamma = kwin_like();
        no_gamma.transfer_functions.retain(|t| t != "gamma22");
        assert!(!no_gamma.offers_rec2020_gamma22());
        let mut icc_only = kwin_like();
        icc_only.features = vec!["icc_v2_v4".into()];
        assert!(!icc_only.offers_rec2020_gamma22());
    }

    #[test]
    fn without_colour_management_the_image_stays_srgb_and_says_why() {
        for (o, why) in [
            (offer(None), "does not offer"),
            (
                offer(Some(ColorManagement::default())),
                "offers no Rec.2020",
            ),
            (
                Offer {
                    session: Session::X11,
                    color_management: None,
                    error: None,
                },
                "X11",
            ),
        ] {
            let d = drawn(&o);
            assert_eq!((d.route, d.encoding.as_str()), ("srgb", "sRGB"));
            assert!(d.reason.contains(why), "{}", d.reason);
        }
    }

    #[test]
    fn the_route_follows_the_compositor() {
        // KWin, sway, Weston: no sRGB transfer, the image surface tagged.
        let d = drawn(&offer(Some(kwin_like())));
        assert_eq!(d.route, "tagged");
        assert!(d.encoding.contains("BT.2020"), "{}", d.encoding);
        // GNOME, Hyprland: sRGB with the sRGB transfer, the driver's.
        let mut gnome_like = kwin_like();
        gnome_like.transfer_functions.push(transfer_name(9));
        assert!(gnome_like.driver_holds_srgb_surfaces());
        let d = drawn(&offer(Some(gnome_like)));
        assert_eq!(d.route, "wsi");
        assert!(d.reason.contains("driver"), "{}", d.reason);
    }

    #[test]
    fn an_offer_reads_as_json() {
        let json = serde_json::to_value(offer(Some(kwin_like()))).unwrap();
        assert_eq!(json["session"], "wayland");
        assert_eq!(json["color_management"]["primaries"][1], "bt2020");
    }
}
