//! From a sidecar's history to Helios modules and parameters, with a report
//! of everything not carried over.
//!
//! An item Helios cannot apply (a module it does not have, a params
//! version it cannot read, a blend it does not do, an item it cannot read
//! at all) is never dropped and never aborts the import: it is reported in
//! [`Report::not_applied`] with its name, version and reason, and kept
//! with its raw params in the edit ([`Imported::darktable`]), so a later
//! Helios can apply it and the sidecar can be written back as it was.
//!
//! An item asking for a mode Helios does not have (a highlights
//! reconstruction, a demosaic method, sigmoid's RGB ratio) is applied with
//! the mode Helios has in its place, and reported in
//! [`Report::substituted`] and in its history item: the picture stays
//! close to darktable's, and the difference is never silent.
//!
//! A module blending through a drawn mask gets its mask as a sub-network
//! ([`NodeMask`]): the shapes and groups of its form, its inversion and
//! refinement, and a `mask_blend` with its opacity. That needs the image's
//! size ([`Frame`]): without one, or when the mask would differ from
//! darktable's, the item is not applied and the report says why.

use helios_core::{Arg, Message, Params, Value as Param};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::blend::{self, Blend};
use crate::drawn::{self, Frame, MaskNet, Problem};
use crate::layout::{self, Layouts};
use crate::legacy;
use crate::sidecar::{HistoryItem, Sidecar};

/// A Helios module instance an edit asks for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    /// The node's name in the network, unique in the edit: the operation,
    /// then the instance's name (see [`HistoryItem::instance_name`]), or
    /// its number when it has no name and is not the first.
    pub name: String,
    /// darktable's operation and instance.
    pub operation: String,
    pub multi_name: String,
    /// darktable's instance number (`multi_priority`), 0 for the first.
    pub instance: u32,
    /// The Helios module.
    pub module: String,
    pub params: Params,
    /// The mask it blends through, if any.
    pub mask: Option<NodeMask>,
}

/// A module's blending: its mask as a sub-network and its opacity, mixed
/// by a `mask_blend` named after the module (`<node> blend`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NodeMask {
    pub net: MaskNet,
    /// The global opacity, 0 to 1.
    pub opacity: f64,
}

/// A blend Helios does not do yet.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlendIssue {
    /// A blend mode other than normal (its darktable number).
    Mode { mode: u32 },
    /// Mixing in another colour space than scene-referred RGB.
    Space { space: i32 },
    /// A parametric mask (darktable's blend if).
    Parametric,
    /// Another module's raster mask.
    Raster,
    /// The details threshold of the mask refinement.
    Details,
    /// The guided-filter feathering of the mask refinement.
    Feathering,
}

impl BlendIssue {
    /// In a few words, for people, by key.
    pub fn describe(&self) -> Message {
        match self {
            BlendIssue::Mode { mode } => {
                Message::new("xmp-blend-mode", "blend mode { $mode }").with("mode", *mode)
            }
            BlendIssue::Space { space } => {
                Message::new("xmp-blend-space", "blending in colour space { $space }")
                    .with("space", *space)
            }
            BlendIssue::Parametric => Message::new("xmp-blend-parametric", "a parametric mask"),
            BlendIssue::Raster => Message::new("xmp-blend-raster", "a raster mask"),
            BlendIssue::Details => {
                Message::new("xmp-blend-details", "the mask's details threshold")
            }
            BlendIssue::Feathering => Message::new("xmp-blend-feathering", "the mask's feathering"),
        }
    }
}

/// A node's name for an item, unique among `taken`.
fn node_name(item: &HistoryItem, taken: &[Node]) -> String {
    let base = if !item.instance_name().is_empty() {
        format!("{} {}", item.operation, item.instance_name())
    } else if item.multi_priority > 0 {
        format!("{} {}", item.operation, item.multi_priority)
    } else {
        item.operation.clone()
    };
    // Two instances may share a name in darktable.
    let mut name = base.clone();
    let mut n = 1;
    while taken.iter().any(|t| t.name == name) {
        name = format!("{base} #{n}");
        n += 1;
    }
    name
}

/// Why a history item is kept without being applied.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reason {
    /// Helios has no module doing what this darktable module does.
    UnknownModule,
    /// Helios cannot read params of this version; it reads `known`.
    UnsupportedVersion { known: Vec<u32> },
    /// The item, or its params, could not be read.
    Unreadable { error: String },
    /// The item blends its result (a mask, an opacity, a blend mode),
    /// which Helios does not do yet: applied without it, it would change
    /// the whole image instead of a part.
    Blending { blendop_version: u32 },
    /// Blend params of a version Helios does not know how to read.
    UnknownBlendVersion { blendop_version: u32 },
    /// Another instance of a module darktable runs only once.
    ExtraInstance,
    /// A blend Helios does not do yet.
    Blend { issue: BlendIssue },
    /// A drawn mask that cannot be carried over.
    Mask { problem: Problem },
    /// A drawn mask, and the import was not given the image's size.
    MaskWithoutImage,
}

impl Reason {
    /// In a few words, for people, by key.
    pub fn describe(&self) -> Message {
        match self {
            Reason::UnknownModule => Message::new("xmp-reason-unknown-module", "no Helios module"),
            Reason::UnsupportedVersion { known } => {
                let known: Vec<String> = known.iter().map(|v| format!("v{v}")).collect();
                Message::new(
                    "xmp-reason-unsupported-version",
                    "version not supported (reads { $known })",
                )
                .with("known", known.join(", "))
            }
            Reason::Unreadable { error } => {
                Message::new("xmp-reason-unreadable", "unreadable: { $error }").with("error", error)
            }
            Reason::Blending { .. } => {
                Message::new("xmp-reason-blending", "blending not supported")
            }
            Reason::UnknownBlendVersion { blendop_version } => Message::new(
                "xmp-reason-unknown-blend-version",
                "blend params v{ $version } not supported",
            )
            .with("version", *blendop_version),
            Reason::ExtraInstance => Message::new(
                "xmp-reason-extra-instance",
                "a second instance of a single-instance module",
            ),
            Reason::Blend { issue } => {
                Message::new("xmp-reason-blend", "blending not supported: { $what }")
                    .with("what", issue.describe())
            }
            Reason::Mask { problem } => {
                Message::new("xmp-reason-mask", "drawn mask not supported: { $why }")
                    .with("why", problem.describe())
            }
            Reason::MaskWithoutImage => Message::new(
                "xmp-reason-mask-without-image",
                "a drawn mask, read without its image",
            ),
        }
    }
}

/// A history item in force that Helios kept without applying.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NotApplied {
    pub operation: String,
    /// darktable's instance number (`multi_priority`) and name.
    pub instance: u32,
    pub multi_name: String,
    /// The version of its params.
    pub version: u32,
    /// Its name for people: operation, instance and version.
    pub label: String,
    pub reason: Reason,
}

/// A mode darktable's module was set to, which Helios does not have,
/// and the one Helios runs in its place, both by darktable's names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Swap {
    /// The darktable field holding the mode.
    pub field: String,
    pub from: String,
    pub to: String,
}

impl Swap {
    /// A darktable mode's name for people: `DT_IOP_HIGHLIGHTS_OPPOSED`
    /// reads `opposed`.
    fn short(name: &str) -> String {
        let rest = [
            "DT_IOP_HIGHLIGHTS_",
            "DT_IOP_DEMOSAIC_",
            "DT_SIGMOID_METHOD_",
        ]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name);
        rest.to_lowercase().replace('_', " ")
    }
}

/// A history item applied with a mode Helios has in place of the one the
/// sidecar asks for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Substituted {
    pub operation: String,
    /// darktable's instance number (`multi_priority`).
    pub instance: u32,
    /// Its name for people: operation, instance and version.
    pub label: String,
    #[serde(flatten)]
    pub swap: Swap,
}

/// What was not carried over, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    /// Items in force that Helios kept but did not apply, in pipe order.
    pub not_applied: Vec<NotApplied>,
    /// Items applied with a substitute for a mode Helios does not have,
    /// in pipe order.
    pub substituted: Vec<Substituted>,
    /// Older versions of modules taken for their presence only: their
    /// params were not read. `(operation, version)`.
    pub params_not_read: Vec<(String, u32)>,
    /// Items migrated from an older version: `(operation, from, to)`.
    pub migrated: Vec<(String, u32, u32)>,
    /// Values without a Helios counterpart: `(operation, field, value)`.
    pub dropped_fields: Vec<(String, String, Value)>,
    /// Disabled items, kept out of the network.
    pub disabled: Vec<String>,
    /// The pipe order the modules were put in: darktable's preset name, or
    /// `custom` for an order the sidecar spells out.
    pub pipe_order: String,
    /// Items the sidecar's order does not place, `(operation, instance)`:
    /// placed next to their module's other instances, or by the rank
    /// darktable's preset gives them.
    pub unordered: Vec<(String, u32)>,
    /// History items in force, disabled ones included.
    pub items: usize,
    /// Items darktable adds when it reads an older sidecar, which the file
    /// does not hold (see [`HistoryItem::inserted`]), by label.
    pub inserted: Vec<String>,
}

impl Report {
    /// One line saying what was not applied, grouped by reason, and what
    /// was applied with a substitute, or `None` when everything in force
    /// was applied as asked: what an import warns once. By key
    /// (`xmp-not-applied`, `xmp-substituted`, both joined by
    /// `xmp-not-applied-and-substituted`), each reason too.
    pub fn summary(&self) -> Option<Message> {
        match (self.not_applied_summary(), self.substituted_summary()) {
            (Some(not_applied), Some(substituted)) => Some(
                Message::new(
                    "xmp-not-applied-and-substituted",
                    "{ $not_applied }; { $substituted }",
                )
                .with("not_applied", not_applied)
                .with("substituted", substituted),
            ),
            (one, other) => one.or(other),
        }
    }

    fn substituted_summary(&self) -> Option<Message> {
        if self.substituted.is_empty() {
            return None;
        }
        let items = self.substituted.iter().map(|s| {
            Message::new(
                "xmp-substituted-item",
                "{ $label } ({ $to } in place of { $from })",
            )
            .with("label", s.label.clone())
            .with("from", Swap::short(&s.swap.from))
            .with("to", Swap::short(&s.swap.to))
        });
        Some(
            Message::new(
                "xmp-substituted",
                "{ $count } applied with a substitute: { $items }",
            )
            .with("count", self.substituted.len())
            .with("items", Arg::list(items, "; ")),
        )
    }

    fn not_applied_summary(&self) -> Option<Message> {
        if self.not_applied.is_empty() {
            return None;
        }
        let mut groups: Vec<(Message, Vec<&str>)> = Vec::new();
        for item in &self.not_applied {
            let reason = item.reason.describe();
            match groups.iter_mut().find(|(r, _)| *r == reason) {
                Some((_, labels)) => labels.push(&item.label),
                None => groups.push((reason, vec![&item.label])),
            }
        }
        let groups = groups.into_iter().map(|(reason, labels)| {
            Message::new("xmp-not-applied-group", "{ $labels } ({ $reason })")
                .with("labels", labels.join(", "))
                .with("reason", reason)
        });
        Some(
            Message::new(
                "xmp-not-applied",
                "{ $count } of { $items } history items not applied, kept for later: { $groups }",
            )
            .with("count", self.not_applied.len())
            .with("items", self.items)
            .with("groups", Arg::list(groups, "; ")),
        )
    }
}

/// An edit read from a sidecar.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Imported {
    /// In darktable's pipe order.
    pub nodes: Vec<Node>,
    pub report: Report,
    /// The darktable edit as it came: the sidecar's text, and each history
    /// item with its raw params and what became of it (see
    /// [`Imported::edit`]). `Null` for an edit not read from a sidecar.
    pub darktable: Value,
}

fn float(fields: &Map<String, Value>, name: &str) -> Option<f64> {
    fields.get(name).and_then(Value::as_f64)
}

fn set(p: &mut Params, name: &str, v: Option<f64>) {
    if let Some(v) = v {
        p.set(name, Param::Float(v));
    }
}

/// One darktable module's decoded params turned into a Helios module's.
struct Mapped {
    module: &'static str,
    params: Params,
    /// Fields read into `params`, or knowingly replaced by something else
    /// (the camera's description).
    used: Vec<&'static str>,
    /// Modes Helios does not have, run with the one it has.
    swaps: Vec<Swap>,
}

/// The darktable modes Helios has.
const CLIP: &str = "DT_IOP_HIGHLIGHTS_CLIP";
const RCD: &str = "DT_IOP_DEMOSAIC_RCD";
const MARKESTEIJN: &str = "DT_IOP_DEMOSAIC_MARKESTEIJN";
const PER_CHANNEL: &str = "DT_SIGMOID_METHOD_PER_CHANNEL";

/// darktable's demosaic methods for X-Trans sensors
/// (`DT_DEMOSAIC_XTRANS` in their value).
const XTRANS_METHODS: &[&str] = &[
    "DT_IOP_DEMOSAIC_VNG",
    "DT_IOP_DEMOSAIC_MARKESTEIJN",
    "DT_IOP_DEMOSAIC_MARKESTEIJN_3",
    "DT_IOP_DEMOSAIC_FDC",
    "DT_IOP_DEMOSAIC_MARKEST3_DUAL",
    "DT_IOP_DEMOSAIC_PASSTHR_MONOX",
    "DT_IOP_DEMOSAIC_PASSTHR_COLORX",
];

/// The mode Helios runs for darktable's mode `field` of `fields`, when it
/// is not the one asked: `has` says which darktable modes Helios has,
/// `instead` which it runs in place of another. Nothing when the field is
/// absent (params not read).
fn swap(
    fields: &Map<String, Value>,
    field: &str,
    has: impl Fn(&str) -> bool,
    instead: impl Fn(&str) -> &'static str,
) -> Option<Swap> {
    let from = match fields.get(field)? {
        Value::String(name) => name.clone(),
        other => other.to_string(),
    };
    if has(&from) {
        return None;
    }
    Some(Swap {
        field: field.into(),
        to: instead(&from).into(),
        from,
    })
}

/// Fields whose value carries nothing when it is its default.
fn is_neutral(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::Bool(b) => !b,
        Value::String(s) => s.is_empty(),
        Value::Array(items) => items.iter().all(is_neutral),
        _ => false,
    }
}

fn map(operation: &str, f: &Map<String, Value>) -> Option<Mapped> {
    let mut p = Params::new();
    let mut swaps = Vec::new();
    let (module, used): (&'static str, Vec<&'static str>) = match operation {
        "rawprepare" => (
            "rawprepare",
            // The crop and levels come from the raw's description in Helios.
            vec![
                "left",
                "top",
                "right",
                "bottom",
                "raw_black_level_separate",
                "raw_white_point",
            ],
        ),
        "temperature" => {
            p.set("mode", Param::Choice("user".into()));
            set(&mut p, "red", float(f, "red"));
            set(&mut p, "green", float(f, "green"));
            set(&mut p, "blue", float(f, "blue"));
            (
                "whitebalance",
                vec!["red", "green", "blue", "various", "preset"],
            )
        }
        "highlights" => {
            swaps.extend(swap(f, "mode", |m| m == CLIP, |_| CLIP));
            set(&mut p, "clip", float(f, "clip"));
            ("highlights", vec!["mode", "clip"])
        }
        "demosaic" => {
            // Helios picks RCD on Bayer sensors, Markesteijn (one pass) on
            // X-Trans: the method asked tells which sensor it was for.
            swaps.extend(swap(
                f,
                "demosaicing_method",
                |m| m == RCD || m == MARKESTEIJN,
                |m| {
                    if XTRANS_METHODS.contains(&m) {
                        MARKESTEIJN
                    } else {
                        RCD
                    }
                },
            ));
            ("demosaic", vec!["demosaicing_method"])
        }
        "colorin" => ("colorin", vec!["type_work", "filename_work"]),
        "exposure" => {
            set(&mut p, "exposure", float(f, "exposure"));
            set(&mut p, "black", float(f, "black"));
            if let Some(b) = f.get("compensate_exposure_bias").and_then(Value::as_bool) {
                p.set("compensate_bias", Param::Bool(b));
            }
            (
                "exposure",
                vec!["mode", "exposure", "black", "compensate_exposure_bias"],
            )
        }
        "sigmoid" => {
            set(&mut p, "contrast", float(f, "middle_grey_contrast"));
            set(&mut p, "skew", float(f, "contrast_skewness"));
            set(&mut p, "white_target", float(f, "display_white_target"));
            set(&mut p, "black_target", float(f, "display_black_target"));
            set(&mut p, "preserve_hue", float(f, "hue_preservation"));
            swaps.extend(swap(
                f,
                "color_processing",
                |m| m == PER_CHANNEL,
                |_| PER_CHANNEL,
            ));
            (
                "sigmoid",
                vec![
                    "middle_grey_contrast",
                    "contrast_skewness",
                    "display_white_target",
                    "display_black_target",
                    "hue_preservation",
                    "color_processing",
                ],
            )
        }
        _ => return None,
    };
    Some(Mapped {
        module,
        params: p,
        used,
        swaps,
    })
}

/// The mapped modules darktable runs once (`IOP_FLAGS_ONE_INSTANCE`).
fn single_instance(operation: &str) -> bool {
    matches!(
        operation,
        "rawprepare" | "temperature" | "highlights" | "demosaic" | "colorin"
    )
}

/// The mapped modules whose output darktable blends
/// (`IOP_FLAGS_SUPPORTS_BLENDING`); the others ignore their blend params.
fn blends(operation: &str) -> bool {
    matches!(operation, "highlights" | "exposure" | "sigmoid")
}

/// An item's blend params: `None` when they leave its output as it is,
/// else the blend to build, or why Helios cannot.
fn check_blend(item: &HistoryItem) -> Result<Option<Blend>, Reason> {
    let p = &item.blendop_params;
    // darktable reads all-zero blend params as its defaults: no blending.
    if !blends(&item.operation) || p.iter().all(|b| *b == 0) {
        return Ok(None);
    }
    let blendop_version = item.blendop_version.unwrap_or(0);
    if !(1..=*blend::VERSIONS.end()).contains(&blendop_version) {
        return Err(Reason::UnknownBlendVersion { blendop_version });
    }
    // The first word is the blend mode up to version 4, the mask mode
    // since; zero means off in both (`DEVELOP_MASK_DISABLED`).
    let first = p
        .get(..4)
        .map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes")))
        .ok_or_else(|| Reason::Unreadable {
            error: format!("blend params of {} bytes", p.len()),
        })?;
    if first == 0 {
        return Ok(None);
    }
    if !blend::VERSIONS.contains(&blendop_version) {
        return Err(Reason::Blending { blendop_version });
    }
    let b = Blend::read(blendop_version, p).map_err(|error| Reason::Unreadable { error })?;
    let issue = |issue| Err(Reason::Blend { issue });
    let mode = b.mask_mode;
    let normal = b.blend_mode == blend::NORMAL;
    // Uniformly at full opacity in normal mode: the output as it is, in
    // any colour space.
    if mode & !blend::mode::ENABLED == 0 && b.opacity() >= 1.0 && normal {
        return Ok(None);
    }
    if !normal {
        return issue(BlendIssue::Mode { mode: b.blend_mode });
    }
    if b.blend_cst != blend::RGB_SCENE {
        return issue(BlendIssue::Space { space: b.blend_cst });
    }
    if mode & blend::mode::RASTER != 0 {
        return issue(BlendIssue::Raster);
    }
    if mode & blend::mode::CONDITIONAL != 0 {
        return issue(BlendIssue::Parametric);
    }
    if mode & (blend::mode::MASK | blend::mode::FLEXI) != 0 {
        if b.details != 0.0 {
            return issue(BlendIssue::Details);
        }
        if b.feathering_radius > 0.1 {
            return issue(BlendIssue::Feathering);
        }
    }
    Ok(Some(b))
}

/// The mask network of a module blending with `b`, named after the node
/// `name`: its drawn mask (or 1, uniformly), inverted, blurred and through
/// its tone curve, in darktable's order (`dt_develop_blend_process`); the
/// opacity goes to the blend.
fn mask_net(
    b: &Blend,
    forms: &[&drawn::Form],
    frame: Option<&Frame>,
    name: &str,
) -> Result<NodeMask, Reason> {
    let constant = |value: f64| {
        // A group without members gives 1, scaled to the value.
        let mut net = MaskNet::default();
        net.nodes.push(drawn::MaskNode {
            name: format!("{name} mask"),
            module: "mask_combine",
            params: Params::new().with("group_opacity", Param::Float(value)),
        });
        net.mask = format!("{name} mask");
        net
    };
    let flexi = b.mask_mode & blend::mode::FLEXI != 0;
    if b.mask_mode & blend::mode::MASK == 0 && !flexi {
        return Ok(NodeMask {
            net: constant(1.0),
            opacity: b.opacity(),
        });
    }
    let root = i64::from(b.mask_id);
    let mut net = if root > 0 && forms.iter().any(|f| f.id == root) {
        let frame = frame.ok_or(Reason::MaskWithoutImage)?;
        let mut net = if flexi {
            drawn::network_flexi(forms, root, frame, name)
        } else {
            drawn::network(forms, root, frame, name)
        }
        .map_err(|problem| Reason::Mask { problem })?;
        if b.inverted() {
            net.invert(format!("{name} mask invert"));
        }
        net
    } else {
        // A drawn mask without its form: darktable fills 1, or 0 inverted.
        constant(if b.inverted() { 0.0 } else { 1.0 })
    };
    if b.blur_radius > 0.1 {
        net.then(
            format!("{name} mask blur"),
            "mask_blur",
            Params::new().with("radius", Param::Float(f64::from(b.blur_radius))),
        );
    }
    if b.contrast.abs() >= 0.01 || b.brightness.abs() >= 0.01 {
        net.then(
            format!("{name} mask levels"),
            "mask_levels",
            Params::new()
                .with("contrast", Param::Float(f64::from(b.contrast)))
                .with("brightness", Param::Float(f64::from(b.brightness))),
        );
    }
    Ok(NodeMask {
        net,
        opacity: b.opacity(),
    })
}

/// An item in force turned into a Helios node, or why it cannot be.
fn apply(
    item: &HistoryItem,
    layouts: &Layouts,
    report: &mut Report,
) -> Result<(Mapped, Option<Blend>), Reason> {
    if let Some(error) = &item.unreadable {
        return Err(Reason::Unreadable {
            error: error.clone(),
        });
    }
    let known = layouts.versions(&item.operation);
    let Some(&current) = known.iter().max() else {
        return Err(Reason::UnknownModule);
    };
    if map(&item.operation, &Map::new()).is_none() {
        return Err(Reason::UnknownModule);
    }
    if item.multi_priority > 0 && single_instance(&item.operation) {
        return Err(Reason::ExtraInstance);
    }
    let blend = check_blend(item)?;
    let layout = layouts
        .get(&item.operation, current)
        .expect("a known version");
    let params = if item.modversion == current {
        item.params.clone()
    } else if legacy::presence_only(&item.operation) {
        report
            .params_not_read
            .push((item.operation.clone(), item.modversion));
        // An empty decode: the mapping reads nothing from these modules.
        return Ok((map(&item.operation, &Map::new()).expect("mapped"), blend));
    } else {
        match legacy::upgrade(&item.operation, item.modversion, &item.params, current) {
            Some(Ok(bytes)) => {
                report
                    .migrated
                    .push((item.operation.clone(), item.modversion, current));
                bytes
            }
            Some(Err(error)) => return Err(Reason::Unreadable { error }),
            None => return Err(Reason::UnsupportedVersion { known }),
        }
    };
    let decoded = match layout::decode(layout, &params) {
        Ok(Value::Object(fields)) => fields,
        Ok(other) => {
            return Err(Reason::Unreadable {
                error: format!("params decode to {other}, not fields"),
            });
        }
        Err(error) => return Err(Reason::Unreadable { error }),
    };
    let mapped = map(&item.operation, &decoded).expect("mapped");
    for (name, value) in &decoded {
        if !mapped.used.contains(&name.as_str()) && !is_neutral(value) {
            report
                .dropped_fields
                .push((item.operation.clone(), name.clone(), value.clone()));
        }
    }
    Ok((mapped, blend))
}

/// Where an imported edit comes from, as a version records it.
pub const ORIGIN: &str = "import:xmp";

impl Imported {
    /// The edit as a network after a source named `src`, the modules
    /// chained in pipe order (output to input), in the shape of a
    /// `helios-node` network description: what the vault keeps as a
    /// version (`Version::new(image, parents, imported.edit(), ORIGIN, now)`).
    /// The darktable edit it came from, items not applied included, is
    /// kept under `darktable` (see [`Imported::darktable`]).
    pub fn edit(&self) -> Value {
        let (modules, connections, _) = chain(&self.nodes, ("src", "image"));
        let mut edit = json!({ "modules": modules, "connections": connections });
        if !self.darktable.is_null() {
            edit["darktable"] = self.darktable.clone();
        }
        edit
    }
}

/// `nodes` chained in their order after the output `port` of node `from`,
/// as a network description's modules and connections, and the output the
/// chain ends with. A masked node is followed by its blend (`<node>
/// blend`), mixing its input and output through its mask's nodes.
pub fn chain(nodes: &[Node], from: (&str, &str)) -> (Vec<Value>, Vec<Value>, (String, String)) {
    let mut modules = Vec::new();
    let mut connections = Vec::new();
    let mut previous = (from.0.to_string(), from.1.to_string());
    let mut connect = |from: &(String, String), to: &str, input: &str| {
        connections.push(json!({ "from": from.0, "port": from.1, "to": to, "input": input }));
    };
    for n in nodes {
        modules.push(json!({ "name": n.name, "module": n.module, "params": n.params }));
        connect(&previous, &n.name, "input");
        let output = (n.name.clone(), "output".to_string());
        let Some(mask) = &n.mask else {
            previous = output;
            continue;
        };
        for m in &mask.net.nodes {
            modules.push(json!({ "name": m.name, "module": m.module, "params": m.params }));
        }
        for l in &mask.net.links {
            let from = match l.from.as_str() {
                drawn::INPUT => previous.clone(),
                drawn::OUTPUT => output.clone(),
                other => (other.to_string(), l.port.clone()),
            };
            connect(&from, &l.to, &l.input);
        }
        let blend = format!("{} blend", n.name);
        modules.push(json!({
            "name": blend,
            "module": "mask_blend",
            "params": Params::new().with("opacity", Param::Float(mask.opacity)),
        }));
        connect(&previous, &blend, "input");
        connect(&output, &blend, "processed");
        connect(&(mask.net.mask.clone(), "mask".to_string()), &blend, "mask");
        previous = (blend, "output".to_string());
    }
    (modules, connections, previous)
}

/// What became of a history item.
fn item_json(item: &HistoryItem, status: &Value) -> Value {
    let attributes: Map<String, Value> = item
        .attributes
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let mut out = json!({
        "num": item.num,
        "operation": item.operation,
        "instance": item.multi_priority,
        "multi_name": item.multi_name,
        "version": item.modversion,
        "enabled": item.enabled,
        "inserted": item.inserted,
        "attributes": attributes,
    });
    if let (Some(out), Some(status)) = (out.as_object_mut(), status.as_object()) {
        out.extend(status.clone());
    }
    out
}

/// Reads a sidecar's text and imports it (see [`import`]), the text kept
/// whole under `darktable.sidecar` so the edit can be written back or
/// read again by a later Helios.
pub fn import_text(xml: &str, layouts: &Layouts) -> Result<(Sidecar, Imported), String> {
    let sidecar = Sidecar::parse(xml)?;
    let mut imported = import(&sidecar, layouts);
    imported.darktable["sidecar"] = Value::String(xml.to_string());
    Ok((sidecar, imported))
}

/// Reads a sidecar's effective history into Helios modules, in the order
/// darktable runs them. Items Helios cannot apply are reported and kept
/// with their raw params, in their place: the import never stops at one.
/// Without the image's frame, an item with a drawn mask is not applied
/// (see [`import_with`]).
pub fn import(sidecar: &Sidecar, layouts: &Layouts) -> Imported {
    import_with(sidecar, layouts, None)
}

/// [`import`], placing drawn masks in the image `frame` describes.
pub fn import_with(sidecar: &Sidecar, layouts: &Layouts, frame: Option<&Frame>) -> Imported {
    let mut out = Imported::default();
    let forms = drawn::in_force(&sidecar.masks, sidecar.history_end);
    let index = |item: &HistoryItem| {
        sidecar
            .history
            .iter()
            .position(|i| std::ptr::eq(i, item))
            .expect("an item of the history")
    };
    let end = sidecar.history_end.unwrap_or(u32::MAX) as usize;
    // What became of each item, in history order.
    let mut status: Vec<Value> = (0..sidecar.history.len())
        .map(|i| json!({ "status": if i < end { "replaced" } else { "undone" } }))
        .collect();
    let arranged = sidecar.in_pipe_order();
    out.report.pipe_order = arranged.order.source;
    out.report.unordered = arranged.unordered;
    out.report.items = arranged.items.len();
    out.report.inserted = sidecar
        .history
        .iter()
        .filter(|i| i.inserted)
        .map(HistoryItem::label)
        .collect();
    for item in arranged.items {
        let at = index(item);
        if !item.enabled {
            out.report.disabled.push(item.operation.clone());
            status[at] = json!({ "status": "disabled" });
            continue;
        }
        let name = node_name(item, &out.nodes);
        let applied = apply(item, layouts, &mut out.report).and_then(|(mapped, blend)| {
            let mask = match blend {
                Some(b) => {
                    if b.mask_mode & (blend::mode::MASK | blend::mode::FLEXI) != 0
                        && let Some(e) = &sidecar.masks_unreadable
                    {
                        return Err(Reason::Unreadable { error: e.clone() });
                    }
                    Some(mask_net(&b, &forms, frame, &name)?)
                }
                None => None,
            };
            Ok((mapped, mask))
        });
        match applied {
            Ok((mapped, mask)) => {
                status[at] = json!({ "status": "applied", "node": name });
                if !mapped.swaps.is_empty() {
                    status[at]["substituted"] = json!(mapped.swaps);
                }
                out.report
                    .substituted
                    .extend(mapped.swaps.iter().map(|swap| Substituted {
                        operation: item.operation.clone(),
                        instance: item.multi_priority,
                        label: item.label(),
                        swap: swap.clone(),
                    }));
                out.nodes.push(Node {
                    name,
                    operation: item.operation.clone(),
                    multi_name: item.multi_name.clone(),
                    instance: item.multi_priority,
                    module: mapped.module.into(),
                    params: mapped.params,
                    mask,
                });
            }
            Err(reason) => {
                status[at] = json!({ "status": "not_applied", "reason": reason });
                out.report.not_applied.push(NotApplied {
                    operation: item.operation.clone(),
                    instance: item.multi_priority,
                    multi_name: item.multi_name.clone(),
                    version: item.modversion,
                    label: item.label(),
                    reason,
                });
            }
        }
    }
    let items: Vec<Value> = sidecar
        .history
        .iter()
        .zip(&status)
        .map(|(item, status)| item_json(item, status))
        .collect();
    out.darktable = json!({
        "xmp_version": sidecar.xmp_version,
        "history_end": sidecar.history_end,
        "iop_order_version": sidecar.iop_order_version,
        "iop_order_list": sidecar.iop_order_list,
        "pipe_order": out.report.pipe_order,
        "history": items,
    });
    out
}
