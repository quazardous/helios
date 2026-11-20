//! What each screen shows, as the system describes it: on Wayland, each
//! output's image description (`wp_color_management_output_v1`), read
//! with [`screens`]; [`ScreenDescription::shows`] sorts it into sRGB,
//! wide gamut or HDR. Other systems describe none yet.

use serde::Serialize;

/// Chromaticities as the protocol gives them: red, green, blue and white
/// `x, y`, each times one million.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Chromaticities {
    pub red: (i32, i32),
    pub green: (i32, i32),
    pub blue: (i32, i32),
    pub white: (i32, i32),
}

impl Chromaticities {
    /// sRGB's (BT.709) primaries with a D65 white.
    pub const SRGB: Chromaticities = Chromaticities {
        red: (640_000, 330_000),
        green: (300_000, 600_000),
        blue: (150_000, 60_000),
        white: (312_700, 329_000),
    };

    /// The area of the gamut's triangle in the CIE 1931 xy plane.
    pub fn area(&self) -> f64 {
        let p = |(x, y): (i32, i32)| (f64::from(x) / 1e6, f64::from(y) / 1e6);
        let (r, g, b) = (p(self.red), p(self.green), p(self.blue));
        ((g.0 - r.0) * (b.1 - r.1) - (b.0 - r.0) * (g.1 - r.1)).abs() / 2.0
    }
}

/// What a screen shows, sorted from its description.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shows {
    /// sRGB, or close to it.
    Srgb,
    /// A gamut wider than sRGB (Display P3, Adobe RGB, Rec.2020), SDR.
    WideGamut,
    /// A high dynamic range transfer (PQ, HLG).
    Hdr,
}

/// A gamut this much larger than sRGB's counts as wide: Display P3 is
/// about 1.36 times sRGB's area, a panel a little over sRGB is not wide.
pub const WIDE_GAMUT_AREA: f64 = 1.15;

/// One screen, as the system describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ScreenDescription {
    /// The screen's name as the system gives it (`wl_output.name`, the
    /// name winit gives the monitor too: `DP-1`, `eDP-1`); none when the
    /// compositor's `wl_output` is older than version 4.
    pub name: Option<String>,
    /// The encoding the screen expects: named primaries, if named.
    pub primaries_named: Option<String>,
    /// The encoding's primaries.
    pub primaries: Option<Chromaticities>,
    /// The primaries of the display itself (its target colour volume).
    pub target_primaries: Option<Chromaticities>,
    /// The transfer function: a protocol name (`gamma22`, `st2084_pq`) or
    /// `power <exponent>`.
    pub transfer: Option<String>,
    /// The encoding's luminances (cd/m²): reference white and maximum.
    pub reference_luminance: Option<u32>,
    pub max_luminance: Option<u32>,
    /// The display's own maximum luminance (cd/m²).
    pub target_max_luminance: Option<u32>,
    /// Whether the description carries an ICC profile (not read).
    pub icc: bool,
    /// Why the description could not be read, if it could not.
    pub error: Option<String>,
}

impl ScreenDescription {
    /// What the screen shows, from its description: HDR with a PQ or HLG
    /// transfer; else wide gamut when the display's own primaries (the
    /// encoding's when it gives none) cover clearly more than sRGB
    /// ([`WIDE_GAMUT_AREA`]); else sRGB. None when the description gives
    /// neither (unread, or an ICC profile only).
    pub fn shows(&self) -> Option<Shows> {
        if self.error.is_some() {
            return None;
        }
        if matches!(self.transfer.as_deref(), Some("st2084_pq" | "hlg")) {
            return Some(Shows::Hdr);
        }
        let gamut = self.target_primaries.or(self.primaries)?;
        if gamut.area() > Chromaticities::SRGB.area() * WIDE_GAMUT_AREA {
            Some(Shows::WideGamut)
        } else {
            Some(Shows::Srgb)
        }
    }
}

/// Every screen the system describes: on Wayland with the compositor's
/// colour management, one connection, each output's image description
/// read, then closed. Empty elsewhere (X11, another system, a compositor
/// without colour management).
///
/// # Errors
/// The compositor could not be asked.
pub fn screens() -> Result<Vec<ScreenDescription>, String> {
    if crate::session_from(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
    ) != crate::Session::Wayland
    {
        return Ok(Vec::new());
    }
    read()
}

#[cfg(target_os = "linux")]
fn read() -> Result<Vec<ScreenDescription>, String> {
    crate::wayland::screens()
}

#[cfg(not(target_os = "linux"))]
fn read() -> Result<Vec<ScreenDescription>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY_P3: Chromaticities = Chromaticities {
        red: (680_000, 320_000),
        green: (265_000, 690_000),
        blue: (150_000, 60_000),
        white: (312_700, 329_000),
    };

    const BT2020: Chromaticities = Chromaticities {
        red: (708_000, 292_000),
        green: (170_000, 797_000),
        blue: (131_000, 46_000),
        white: (312_700, 329_000),
    };

    fn sdr(target: Chromaticities) -> ScreenDescription {
        ScreenDescription {
            name: Some("DP-1".into()),
            primaries_named: Some("srgb".into()),
            primaries: Some(Chromaticities::SRGB),
            target_primaries: Some(target),
            transfer: Some("gamma22".into()),
            ..ScreenDescription::default()
        }
    }

    #[test]
    fn an_srgb_panel_shows_srgb() {
        assert_eq!(sdr(Chromaticities::SRGB).shows(), Some(Shows::Srgb));
    }

    #[test]
    fn the_display_s_own_gamut_decides_not_the_encoding() {
        assert_eq!(sdr(DISPLAY_P3).shows(), Some(Shows::WideGamut));
        // Encoded Rec.2020 on an sRGB panel: sRGB.
        let mut d = sdr(Chromaticities::SRGB);
        d.primaries = Some(BT2020);
        assert_eq!(d.shows(), Some(Shows::Srgb));
        // No target given: the encoding's.
        d.target_primaries = None;
        assert_eq!(d.shows(), Some(Shows::WideGamut));
    }

    #[test]
    fn a_pq_or_hlg_transfer_is_hdr() {
        for tf in ["st2084_pq", "hlg"] {
            let mut d = sdr(BT2020);
            d.transfer = Some(tf.into());
            assert_eq!(d.shows(), Some(Shows::Hdr));
        }
    }

    #[test]
    fn a_description_without_primaries_or_with_an_error_says_nothing() {
        let icc_only = ScreenDescription {
            icc: true,
            ..ScreenDescription::default()
        };
        assert_eq!(icc_only.shows(), None);
        let mut failed = sdr(DISPLAY_P3);
        failed.error = Some("low_version".into());
        assert_eq!(failed.shows(), None);
    }

    #[test]
    fn display_p3_is_about_a_third_wider_than_srgb() {
        let ratio = DISPLAY_P3.area() / Chromaticities::SRGB.area();
        assert!((1.3..1.4).contains(&ratio), "{ratio}");
    }
}
