//! darktable's pipe order: the order its modules process an image in,
//! which is not the order they were edited in.
//!
//! A sidecar names its order with `darktable:iop_order_version` (one of
//! darktable's built-in presets) or spells it out with
//! `darktable:iop_order_list` (a custom order, or any order with multiple
//! instances). Sidecars older than `xmp_version` 4 carry an order per
//! history item instead. [`arrange`] reads all of these the way darktable
//! does (`src/common/exif.cc`, `src/common/iop_order.c`) and puts the
//! effective history in pipe order.
//!
//! The preset tables and the migration of older custom lists below are
//! data copied from darktable's `src/common/iop_order.c`, as of darktable
//! 5.7 (development, after the `release-5.7.0` tag). A test checks them
//! against a darktable checkout when one is present.

use serde::Serialize;

use crate::sidecar::{HistoryItem, Sidecar};

/// darktable's built-in orders (`dt_iop_order_t`, without `custom`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Preset {
    /// Up to darktable 2.6.
    Legacy,
    /// darktable 3.0, for raw files.
    V30,
    /// darktable 3.0, for non-linear input (JPEG).
    V30Jpeg,
    /// darktable 5.0, for raw files: darktable's default.
    V50,
    /// darktable 5.0, for non-linear input (JPEG).
    V50Jpeg,
}

impl Preset {
    /// The preset an `iop_order_version` number names; 0 is a custom order.
    pub fn from_version(version: u32) -> Option<Preset> {
        match version {
            1 => Some(Preset::Legacy),
            2 => Some(Preset::V30),
            3 => Some(Preset::V30Jpeg),
            4 => Some(Preset::V50),
            5 => Some(Preset::V50Jpeg),
            _ => None,
        }
    }

    /// The modules in order, with the original order numbers.
    pub fn table(self) -> &'static [(f32, &'static str)] {
        match self {
            Preset::Legacy => LEGACY,
            Preset::V30 => V30,
            Preset::V30Jpeg => V30_JPEG,
            Preset::V50 => V50,
            Preset::V50Jpeg => V50_JPEG,
        }
    }

    /// darktable's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Legacy => "legacy",
            Preset::V30 => "v3.0 RAW",
            Preset::V30Jpeg => "v3.0 JPEG",
            Preset::V50 => "v5.0 RAW",
            Preset::V50Jpeg => "v5.0 JPEG",
        }
    }

    fn entries(self) -> Vec<(String, u32)> {
        self.table()
            .iter()
            .map(|&(_, op)| (op.to_string(), 0))
            .collect()
    }
}

/// darktable's order when nothing else applies (`DT_DEFAULT_IOP_ORDER_RAW`).
pub const DEFAULT: Preset = Preset::V50;

/// Reads `darktable:iop_order_list`: operation and instance pairs,
/// comma-separated. Like darktable, a list that does not start with
/// rawprepare and end with gamma is refused.
pub fn parse_list(text: &str) -> Result<Vec<(String, u32)>, String> {
    let fields: Vec<&str> = text.split(',').collect();
    if !fields.len().is_multiple_of(2) {
        return Err("an operation without an instance".into());
    }
    let list: Vec<(String, u32)> = fields
        .chunks(2)
        .map(|pair| {
            let instance = pair[1]
                .trim()
                .parse()
                .map_err(|_| format!("instance `{}` is not a number", pair[1]))?;
            Ok((pair[0].trim().to_string(), instance))
        })
        .collect::<Result<_, String>>()?;
    let first = list.first().map(|(op, _)| op.as_str());
    let last = list.last().map(|(op, _)| op.as_str());
    if first != Some("rawprepare") || last != Some("gamma") {
        return Err("it does not run from rawprepare to gamma".into());
    }
    Ok(list)
}

/// Adds to an older custom list the modules that came after it, next to
/// their neighbours (`dt_ioppr_migrate_legacy_iop_order_list`).
pub fn migrate_list(list: &mut Vec<(String, u32)>) {
    for &(anchor, module, before) in MIGRATIONS {
        if list.iter().any(|(op, _)| op == module) {
            continue;
        }
        if let Some(at) = list.iter().position(|(op, _)| op == anchor) {
            let at = if before { at } else { at + 1 };
            list.insert(at, (module.to_string(), 0));
        }
    }
}

/// The order a sidecar's modules run in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PipeOrder {
    /// Operation and instance (`multi_priority`), first to last.
    pub entries: Vec<(String, u32)>,
    /// Where it comes from: darktable's preset name, or `custom`, and why
    /// a list was not used when it was refused.
    pub source: String,
    /// The preset that ranks modules missing from `entries`.
    pub preset: Preset,
}

/// The pipe order of a sidecar, read as darktable reads it.
pub fn pipe_order(sidecar: &Sidecar) -> PipeOrder {
    let xmp_version = sidecar.xmp_version.unwrap_or(0);
    if xmp_version >= 4 {
        // Without a version, darktable takes its legacy order.
        let version = sidecar.iop_order_version.unwrap_or(1);
        let preset = Preset::from_version(version);
        let fallback = preset.unwrap_or(DEFAULT);
        let (mut entries, source) = match &sidecar.iop_order_list {
            Some(text) => match parse_list(text) {
                Ok(list) => (
                    list,
                    preset.map_or_else(|| "custom".to_string(), |p| p.name().to_string()),
                ),
                Err(e) => (
                    fallback.entries(),
                    format!("{} (iop_order_list refused: {e})", fallback.name()),
                ),
            },
            None => (fallback.entries(), fallback.name().to_string()),
        };
        migrate_list(&mut entries);
        return PipeOrder {
            entries,
            source,
            preset: fallback,
        };
    }
    // Older sidecars: an order number per history item, on top of a preset.
    let preset = if xmp_version == 3 && sidecar.iop_order_version.is_some_and(|v| v >= 3) {
        DEFAULT
    } else {
        Preset::Legacy
    };
    let mut ordered: Vec<(String, u32, f64)> = preset
        .table()
        .iter()
        .map(|&(f, op)| (op.to_string(), 0, f64::from(f)))
        .collect();
    for item in &sidecar.history {
        let order = if xmp_version == 3 {
            item.iop_order
        } else {
            // Before version 3, instances were grouped before their module.
            preset
                .table()
                .iter()
                .find(|(_, op)| *op == item.operation)
                .map(|&(f, _)| f64::from(f) - f64::from(item.multi_priority) / 100.0)
        };
        let Some(order) = order else { continue };
        ordered.retain(|(op, i, _)| !(op == &item.operation && *i == item.multi_priority));
        ordered.push((item.operation.clone(), item.multi_priority, order));
    }
    ordered.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut entries = ordered.into_iter().map(|(op, i, _)| (op, i)).collect();
    migrate_list(&mut entries);
    PipeOrder {
        entries,
        source: format!("{} (xmp_version {xmp_version})", preset.name()),
        preset,
    }
}

/// A sidecar's effective history in pipe order.
#[derive(Clone, Debug, PartialEq)]
pub struct Arranged<'a> {
    pub items: Vec<&'a HistoryItem>,
    pub order: PipeOrder,
    /// Items the order does not place, `(operation, multi_priority)`: they
    /// are put after the instance of their module just below theirs, or
    /// after the modules their preset puts before them.
    pub unordered: Vec<(String, u32)>,
}

/// Puts a sidecar's effective history in pipe order.
pub fn arrange(sidecar: &Sidecar) -> Arranged<'_> {
    let order = pipe_order(sidecar);
    let rank = |op: &str| order.preset.table().iter().position(|(_, o)| *o == op);
    let mut unordered = Vec::new();
    // Sort keys: twice the index of the entry an item is at, or after
    // (plus one), then its instance, then its place in the history.
    let mut keyed: Vec<((i64, u32, usize), &HistoryItem)> = Vec::new();
    for (pos, item) in sidecar.effective().into_iter().enumerate() {
        let (op, instance) = (item.operation.as_str(), item.multi_priority);
        let index = |i: usize| i64::try_from(i).unwrap_or(i64::MAX / 2);
        let key = if let Some(at) = order
            .entries
            .iter()
            .position(|(o, i)| o == op && *i == instance)
        {
            (2 * index(at), 0, pos)
        } else {
            unordered.push((op.to_string(), instance));
            let same_module = order
                .entries
                .iter()
                .enumerate()
                .filter(|(_, (o, _))| o == op);
            let below = same_module
                .clone()
                .filter(|(_, (_, i))| *i < instance)
                .max_by_key(|(_, (_, i))| *i)
                .map(|(at, _)| at);
            let anchor = match (below, same_module.map(|(at, _)| at).min()) {
                (Some(at), _) => Some(index(at)),
                (None, Some(first)) => Some(index(first) - 1),
                (None, None) => rank(op).map(|r| {
                    order
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, (o, _))| rank(o).is_some_and(|ro| ro < r))
                        .map(|(at, _)| index(at))
                        .max()
                        .unwrap_or(-1)
                }),
            };
            // A module no preset knows goes last, in history order.
            (anchor.map_or(i64::MAX, |a| 2 * a + 1), instance, pos)
        };
        keyed.push((key, item));
    }
    keyed.sort_by_key(|(key, _)| *key);
    Arranged {
        items: keyed.into_iter().map(|(_, item)| item).collect(),
        order,
        unordered,
    }
}

/// `legacy_order`.
pub const LEGACY: &[(f32, &str)] = &[
    (1.0, "rawprepare"),
    (2.0, "invert"),
    (3.0, "temperature"),
    (3.1, "rasterfile"),
    (4.0, "highlights"),
    (5.0, "cacorrect"),
    (6.0, "hotpixels"),
    (7.0, "rawdenoise"),
    (8.0, "demosaic"),
    (8.5, "demosaicscale"),
    (9.0, "mask_manager"),
    (10.0, "denoiseprofile"),
    (11.0, "tonemap"),
    (12.0, "exposure"),
    (13.0, "spots"),
    (14.0, "retouch"),
    (15.0, "lens"),
    (15.5, "cacorrectrgb"),
    (16.0, "ashift"),
    (17.0, "liquify"),
    (18.0, "rotatepixels"),
    (19.0, "scalepixels"),
    (20.0, "flip"),
    (20.5, "enlargecanvas"),
    (21.0, "clipping"),
    (21.5, "toneequal"),
    (21.7, "crop"),
    (21.9, "overlay"),
    (22.0, "graduatednd"),
    (23.0, "basecurve"),
    (24.0, "bilateral"),
    (25.0, "profile_gamma"),
    (26.0, "hazeremoval"),
    (27.0, "colorin"),
    (27.5, "channelmixerrgb"),
    (27.5, "contrastntexture"),
    (27.5, "diffuse"),
    (27.5, "censorize"),
    (27.5, "negadoctor"),
    (27.5, "blurs"),
    (27.5, "basicadj"),
    (27.5, "primaries"),
    (28.0, "colorreconstruct"),
    (29.0, "colorchecker"),
    (30.0, "defringe"),
    (31.0, "equalizer"),
    (32.0, "vibrance"),
    (32.5, "colorharmonizer"),
    (33.0, "colorbalance"),
    (33.2, "colorequal"),
    (33.5, "colorbalancergb"),
    (33.7, "satcurvergb"),
    (34.0, "colorize"),
    (35.0, "colortransfer"),
    (36.0, "colormapping"),
    (37.0, "bloom"),
    (38.0, "nlmeans"),
    (39.0, "globaltonemap"),
    (40.0, "shadhi"),
    (41.0, "atrous"),
    (42.0, "bilat"),
    (43.0, "colorzones"),
    (44.0, "lowlight"),
    (45.0, "monochrome"),
    (45.3, "sigmoid"),
    (45.5, "agx"),
    (46.0, "filmic"),
    (46.5, "filmicrgb"),
    (46.7, "spektrafilm"),
    (47.0, "colisa"),
    (48.0, "zonesystem"),
    (49.0, "tonecurve"),
    (50.0, "levels"),
    (50.2, "rgblevels"),
    (50.5, "rgbcurve"),
    (51.0, "relight"),
    (52.0, "colorcorrection"),
    (53.0, "sharpen"),
    (54.0, "lowpass"),
    (55.0, "highpass"),
    (56.0, "grain"),
    (56.5, "lut3d"),
    (57.0, "colorcontrast"),
    (58.0, "colorout"),
    (59.0, "channelmixer"),
    (60.0, "soften"),
    (61.0, "vignette"),
    (62.0, "splittoning"),
    (63.0, "velvia"),
    (64.0, "clahe"),
    (65.0, "finalscale"),
    (66.0, "overexposed"),
    (67.0, "rawoverexposed"),
    (67.5, "dither"),
    (68.0, "borders"),
    (69.0, "watermark"),
    (71.0, "gamma"),
];

/// `v30_order`.
pub const V30: &[(f32, &str)] = &[
    (1.0, "rawprepare"),
    (2.0, "invert"),
    (3.0, "temperature"),
    (3.1, "rasterfile"),
    (4.0, "highlights"),
    (5.0, "cacorrect"),
    (6.0, "hotpixels"),
    (7.0, "rawdenoise"),
    (8.0, "demosaic"),
    (8.5, "demosaicscale"),
    (9.0, "denoiseprofile"),
    (10.0, "bilateral"),
    (11.0, "rotatepixels"),
    (12.0, "scalepixels"),
    (13.0, "lens"),
    (13.5, "cacorrectrgb"),
    (14.0, "hazeremoval"),
    (15.0, "ashift"),
    (16.0, "flip"),
    (16.5, "enlargecanvas"),
    (16.7, "overlay"),
    (17.0, "clipping"),
    (18.0, "liquify"),
    (19.0, "spots"),
    (20.0, "retouch"),
    (21.0, "exposure"),
    (22.0, "mask_manager"),
    (23.0, "tonemap"),
    (24.0, "toneequal"),
    (24.5, "crop"),
    (25.0, "graduatednd"),
    (26.0, "profile_gamma"),
    (27.0, "equalizer"),
    (28.0, "colorin"),
    (28.5, "channelmixerrgb"),
    (28.5, "contrastntexture"),
    (28.5, "diffuse"),
    (28.5, "censorize"),
    (28.5, "negadoctor"),
    (28.5, "blurs"),
    (28.5, "primaries"),
    (29.0, "nlmeans"),
    (30.0, "colorchecker"),
    (31.0, "defringe"),
    (32.0, "atrous"),
    (33.0, "lowpass"),
    (34.0, "highpass"),
    (35.0, "sharpen"),
    (37.0, "colortransfer"),
    (38.0, "colormapping"),
    (39.0, "channelmixer"),
    (40.0, "basicadj"),
    (40.5, "colorharmonizer"),
    (41.0, "colorbalance"),
    (41.2, "colorequal"),
    (41.5, "colorbalancergb"),
    (41.7, "satcurvergb"),
    (42.0, "rgbcurve"),
    (43.0, "rgblevels"),
    (44.0, "basecurve"),
    (45.0, "filmic"),
    (45.3, "sigmoid"),
    (45.5, "agx"),
    (46.0, "filmicrgb"),
    (46.7, "spektrafilm"),
    (36.0, "lut3d"),
    (47.0, "colisa"),
    (48.0, "tonecurve"),
    (49.0, "levels"),
    (50.0, "shadhi"),
    (51.0, "zonesystem"),
    (52.0, "globaltonemap"),
    (53.0, "relight"),
    (54.0, "bilat"),
    (55.0, "colorcorrection"),
    (56.0, "colorcontrast"),
    (57.0, "velvia"),
    (58.0, "vibrance"),
    (60.0, "colorzones"),
    (61.0, "bloom"),
    (62.0, "colorize"),
    (63.0, "lowlight"),
    (64.0, "monochrome"),
    (65.0, "grain"),
    (66.0, "soften"),
    (67.0, "splittoning"),
    (68.0, "vignette"),
    (69.0, "colorreconstruct"),
    (70.0, "colorout"),
    (71.0, "clahe"),
    (72.0, "finalscale"),
    (73.0, "overexposed"),
    (74.0, "rawoverexposed"),
    (75.0, "dither"),
    (76.0, "borders"),
    (77.0, "watermark"),
    (78.0, "gamma"),
];

/// `v30_jpg_order`.
pub const V30_JPEG: &[(f32, &str)] = &[
    (1.0, "rawprepare"),
    (2.0, "invert"),
    (3.0, "temperature"),
    (3.1, "rasterfile"),
    (4.0, "highlights"),
    (5.0, "cacorrect"),
    (6.0, "hotpixels"),
    (7.0, "rawdenoise"),
    (8.0, "demosaic"),
    (8.5, "demosaicscale"),
    (28.0, "colorin"),
    (28.0, "denoiseprofile"),
    (28.0, "bilateral"),
    (28.0, "rotatepixels"),
    (28.0, "scalepixels"),
    (28.0, "lens"),
    (28.0, "cacorrectrgb"),
    (28.0, "hazeremoval"),
    (28.0, "ashift"),
    (28.0, "flip"),
    (28.0, "enlargecanvas"),
    (28.0, "overlay"),
    (28.0, "clipping"),
    (28.0, "liquify"),
    (28.0, "spots"),
    (28.0, "retouch"),
    (28.0, "exposure"),
    (28.0, "mask_manager"),
    (28.0, "tonemap"),
    (28.0, "toneequal"),
    (28.0, "crop"),
    (28.0, "graduatednd"),
    (28.0, "profile_gamma"),
    (28.0, "equalizer"),
    (28.5, "channelmixerrgb"),
    (28.5, "contrastntexture"),
    (28.5, "diffuse"),
    (28.5, "censorize"),
    (28.5, "negadoctor"),
    (28.5, "blurs"),
    (28.5, "primaries"),
    (29.0, "nlmeans"),
    (30.0, "colorchecker"),
    (31.0, "defringe"),
    (32.0, "atrous"),
    (33.0, "lowpass"),
    (34.0, "highpass"),
    (35.0, "sharpen"),
    (37.0, "colortransfer"),
    (38.0, "colormapping"),
    (39.0, "channelmixer"),
    (40.0, "basicadj"),
    (40.5, "colorharmonizer"),
    (41.0, "colorbalance"),
    (41.2, "colorequal"),
    (41.5, "colorbalancergb"),
    (41.7, "satcurvergb"),
    (42.0, "rgbcurve"),
    (43.0, "rgblevels"),
    (44.0, "basecurve"),
    (45.0, "filmic"),
    (45.5, "agx"),
    (45.3, "sigmoid"),
    (46.0, "filmicrgb"),
    (46.7, "spektrafilm"),
    (36.0, "lut3d"),
    (47.0, "colisa"),
    (48.0, "tonecurve"),
    (49.0, "levels"),
    (50.0, "shadhi"),
    (51.0, "zonesystem"),
    (52.0, "globaltonemap"),
    (53.0, "relight"),
    (54.0, "bilat"),
    (55.0, "colorcorrection"),
    (56.0, "colorcontrast"),
    (57.0, "velvia"),
    (58.0, "vibrance"),
    (60.0, "colorzones"),
    (61.0, "bloom"),
    (62.0, "colorize"),
    (63.0, "lowlight"),
    (64.0, "monochrome"),
    (65.0, "grain"),
    (66.0, "soften"),
    (67.0, "splittoning"),
    (68.0, "vignette"),
    (69.0, "colorreconstruct"),
    (70.0, "colorout"),
    (71.0, "clahe"),
    (72.0, "finalscale"),
    (73.0, "overexposed"),
    (74.0, "rawoverexposed"),
    (75.0, "dither"),
    (76.0, "borders"),
    (77.0, "watermark"),
    (78.0, "gamma"),
];

/// `v50_order`.
pub const V50: &[(f32, &str)] = &[
    (1.0, "rawprepare"),
    (2.0, "invert"),
    (3.0, "temperature"),
    (3.1, "rasterfile"),
    (4.0, "highlights"),
    (5.0, "cacorrect"),
    (6.0, "hotpixels"),
    (7.0, "rawdenoise"),
    (8.0, "demosaic"),
    (8.5, "demosaicscale"),
    (9.0, "denoiseprofile"),
    (10.0, "bilateral"),
    (11.0, "rotatepixels"),
    (12.0, "scalepixels"),
    (13.0, "lens"),
    (13.5, "cacorrectrgb"),
    (14.0, "hazeremoval"),
    (15.0, "ashift"),
    (16.0, "flip"),
    (16.5, "enlargecanvas"),
    (16.7, "overlay"),
    (17.0, "clipping"),
    (18.0, "liquify"),
    (19.0, "spots"),
    (20.0, "retouch"),
    (21.0, "exposure"),
    (22.0, "mask_manager"),
    (23.0, "tonemap"),
    (24.0, "toneequal"),
    (24.5, "crop"),
    (25.0, "graduatednd"),
    (26.0, "profile_gamma"),
    (27.0, "equalizer"),
    (28.0, "colorin"),
    (28.5, "channelmixerrgb"),
    (28.5, "contrastntexture"),
    (28.5, "diffuse"),
    (28.5, "censorize"),
    (28.5, "negadoctor"),
    (28.5, "blurs"),
    (28.5, "primaries"),
    (29.0, "nlmeans"),
    (30.0, "colorchecker"),
    (31.0, "defringe"),
    (32.0, "atrous"),
    (33.0, "lowpass"),
    (34.0, "highpass"),
    (35.0, "sharpen"),
    (37.0, "colortransfer"),
    (38.0, "colormapping"),
    (39.0, "channelmixer"),
    (40.0, "basicadj"),
    (40.5, "colorharmonizer"),
    (41.0, "colorbalance"),
    (41.2, "colorequal"),
    (41.5, "colorbalancergb"),
    (41.7, "satcurvergb"),
    (42.0, "rgbcurve"),
    (43.0, "rgblevels"),
    (44.0, "basecurve"),
    (45.0, "filmic"),
    (45.3, "sigmoid"),
    (45.5, "agx"),
    (46.0, "filmicrgb"),
    (46.7, "spektrafilm"),
    (36.0, "lut3d"),
    (47.0, "colisa"),
    (48.0, "tonecurve"),
    (49.0, "levels"),
    (50.0, "shadhi"),
    (51.0, "zonesystem"),
    (52.0, "globaltonemap"),
    (53.0, "relight"),
    (54.0, "bilat"),
    (55.0, "colorcorrection"),
    (56.0, "colorcontrast"),
    (57.0, "velvia"),
    (58.0, "vibrance"),
    (60.0, "colorzones"),
    (61.0, "bloom"),
    (62.0, "colorize"),
    (63.0, "lowlight"),
    (64.0, "monochrome"),
    (65.0, "grain"),
    (66.0, "soften"),
    (67.0, "splittoning"),
    (68.0, "vignette"),
    (69.0, "colorreconstruct"),
    (69.4, "finalscale"),
    (70.0, "colorout"),
    (71.0, "clahe"),
    (73.0, "overexposed"),
    (74.0, "rawoverexposed"),
    (75.0, "dither"),
    (76.0, "borders"),
    (77.0, "watermark"),
    (78.0, "gamma"),
];

/// `v50_jpg_order`.
pub const V50_JPEG: &[(f32, &str)] = &[
    (1.0, "rawprepare"),
    (2.0, "invert"),
    (3.0, "temperature"),
    (3.1, "rasterfile"),
    (4.0, "highlights"),
    (5.0, "cacorrect"),
    (6.0, "hotpixels"),
    (7.0, "rawdenoise"),
    (8.0, "demosaic"),
    (8.5, "demosaicscale"),
    (28.0, "colorin"),
    (28.0, "denoiseprofile"),
    (28.0, "bilateral"),
    (28.0, "rotatepixels"),
    (28.0, "scalepixels"),
    (28.0, "lens"),
    (28.0, "cacorrectrgb"),
    (28.0, "hazeremoval"),
    (28.0, "ashift"),
    (28.0, "flip"),
    (28.0, "enlargecanvas"),
    (28.0, "overlay"),
    (28.0, "clipping"),
    (28.0, "liquify"),
    (28.0, "spots"),
    (28.0, "retouch"),
    (28.0, "exposure"),
    (28.0, "mask_manager"),
    (28.0, "tonemap"),
    (28.0, "toneequal"),
    (28.0, "crop"),
    (28.0, "graduatednd"),
    (28.0, "profile_gamma"),
    (28.0, "equalizer"),
    (28.5, "channelmixerrgb"),
    (28.5, "contrastntexture"),
    (28.5, "diffuse"),
    (28.5, "censorize"),
    (28.5, "negadoctor"),
    (28.5, "blurs"),
    (28.5, "primaries"),
    (29.0, "nlmeans"),
    (30.0, "colorchecker"),
    (31.0, "defringe"),
    (32.0, "atrous"),
    (33.0, "lowpass"),
    (34.0, "highpass"),
    (35.0, "sharpen"),
    (37.0, "colortransfer"),
    (38.0, "colormapping"),
    (39.0, "channelmixer"),
    (40.0, "basicadj"),
    (40.5, "colorharmonizer"),
    (41.0, "colorbalance"),
    (41.2, "colorequal"),
    (41.5, "colorbalancergb"),
    (41.7, "satcurvergb"),
    (42.0, "rgbcurve"),
    (43.0, "rgblevels"),
    (44.0, "basecurve"),
    (45.0, "filmic"),
    (45.3, "sigmoid"),
    (45.5, "agx"),
    (46.0, "filmicrgb"),
    (46.7, "spektrafilm"),
    (36.0, "lut3d"),
    (47.0, "colisa"),
    (48.0, "tonecurve"),
    (49.0, "levels"),
    (50.0, "shadhi"),
    (51.0, "zonesystem"),
    (52.0, "globaltonemap"),
    (53.0, "relight"),
    (54.0, "bilat"),
    (55.0, "colorcorrection"),
    (56.0, "colorcontrast"),
    (57.0, "velvia"),
    (58.0, "vibrance"),
    (60.0, "colorzones"),
    (61.0, "bloom"),
    (62.0, "colorize"),
    (63.0, "lowlight"),
    (64.0, "monochrome"),
    (65.0, "grain"),
    (66.0, "soften"),
    (67.0, "splittoning"),
    (68.0, "vignette"),
    (69.0, "colorreconstruct"),
    (69.5, "finalscale"),
    (70.0, "colorout"),
    (71.0, "clahe"),
    (73.0, "overexposed"),
    (74.0, "rawoverexposed"),
    (75.0, "dither"),
    (76.0, "borders"),
    (77.0, "watermark"),
    (78.0, "gamma"),
];

/// `dt_ioppr_migrate_legacy_iop_order_list`: `(anchor, module, before)`.
pub const MIGRATIONS: &[(&str, &str, bool)] = &[
    ("nlmeans", "negadoctor", true),
    ("negadoctor", "channelmixerrgb", true),
    ("negadoctor", "contrastntexture", true),
    ("negadoctor", "censorize", true),
    ("negadoctor", "primaries", true),
    ("rgbcurve", "colorbalancergb", true),
    ("rgbcurve", "satcurvergb", true),
    ("ashift", "cacorrectrgb", true),
    ("graduatednd", "crop", true),
    ("flip", "enlargecanvas", true),
    ("enlargecanvas", "overlay", true),
    ("colorbalance", "diffuse", true),
    ("nlmeans", "blurs", true),
    ("filmicrgb", "sigmoid", true),
    ("filmicrgb", "agx", true),
    ("colisa", "spektrafilm", true),
    ("colorbalancergb", "colorequal", true),
    ("highlights", "rasterfile", true),
    ("demosaic", "demosaicscale", false),
    ("colorbalance", "colorharmonizer", true),
];
