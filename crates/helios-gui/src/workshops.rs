//! Workshops, an experimental surface on macros (see
//! `docs/design/workshops.md`): a workshop is a macro whose file carries
//! an intent and relative targets, found by what one wants, applied after
//! the node the image is shown from and aimed on the open image.
//!
//! - Behind a switch, `interface.experimental.workshops` ([`SETTING`],
//!   user taste, off by default), read once when the desktop starts: on,
//!   the Workshops panel ([`PANEL`]) is declared
//!   ([`crate::helios_panels_with`]) and the `workshop.*` commands are
//!   registered in the "Experimental" category ([`CATEGORY`]); off,
//!   neither exists. Changing it takes effect at the next start.
//! - Off, a composite with a workshop block still loads and renders: it
//!   is the macro it is.
//! - The panel says it is experimental in its header.

use egui_pop::core::ui::UiPop as _;
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use helios_core::{Message, Value};
use helios_shell::commands::Origin as CommandOrigin;
use helios_template::format::{CompositeFile, CompositeRef, Measure};
use helios_template::workshop::{Matched, suggest};
use helios_values::{Change, Set, Store};
use serde_json::json;

use crate::dialogs::{Request, request};
use crate::params::Params;

/// The switch.
pub const SETTING: &str = "interface.experimental.workshops";

/// The panel's id.
pub const PANEL: &str = "helios.workshops";

/// The command-palette category of the experimental commands, so the
/// palette shows the word.
pub const CATEGORY: &str = "Experimental";

/// Whether workshops are on, from the resolved settings
/// ([`crate::config`]: the user's layer, then `--set`).
pub fn enabled(config: &helios_config::Config) -> bool {
    config.toggle(SETTING)
}

/// The `--set KEY=VALUE` pairs of a launch's arguments: one invocation's
/// settings (the run layer).
pub fn run_layer(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut sets = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        if a == SET_FLAG {
            let pair = args
                .next()
                .ok_or_else(|| format!("{SET_FLAG} takes KEY=VALUE"))?;
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("{SET_FLAG} {pair}: not KEY=VALUE"))?;
            sets.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok(sets)
}

/// The flag giving one setting for this run (`--set KEY=VALUE`).
pub const SET_FLAG: &str = "--set";

/// Where a workshop of the list comes from: its group in the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// Helios ships it.
    Shipped,
    /// The user's library (`<config>/composites/`).
    Library,
    /// The workspace's own composites.
    Workspace,
}

/// A workshop of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub file: CompositeFile,
    pub origin: Origin,
}

/// Every workshop of the workspace's catalogue, the newest version of
/// each id, from where that version comes first: the shipped ones, the
/// user's library, the workspace's own (which carries a copy of every
/// composite its network uses). Sorted by origin, then name.
pub fn entries(
    shipped: &[CompositeFile],
    library: &[CompositeFile],
    own: &[CompositeFile],
) -> Vec<Entry> {
    let all = [
        (Origin::Shipped, shipped),
        (Origin::Library, library),
        (Origin::Workspace, own),
    ];
    let mut newest: BTreeMap<&str, (u32, Origin, &CompositeFile)> = BTreeMap::new();
    for (origin, files) in all {
        for f in files.iter().filter(|f| f.workshop.is_some()) {
            match newest.get(f.id.as_str()) {
                Some((v, ..)) if *v >= f.version => {}
                _ => {
                    newest.insert(&f.id, (f.version, origin, f));
                }
            }
        }
    }
    let mut out: Vec<Entry> = newest
        .into_values()
        .map(|(_, origin, f)| Entry {
            file: f.clone(),
            origin,
        })
        .collect();
    out.sort_by(|a, b| {
        a.origin
            .cmp(&b.origin)
            .then_with(|| a.file.name.cmp(&b.file.name))
    });
    out
}

/// What the panel reads of the workspace's state (the store's
/// `WORKSPACE_STATE`), parsed again only when it changes.
#[derive(Default)]
struct Seen {
    text: String,
    /// The workspace's own composites.
    own: Vec<CompositeFile>,
    /// Its network's composite nodes, by name.
    nodes: BTreeMap<String, CompositeRef>,
}

impl Seen {
    fn follow(&mut self, store: &Store) {
        let Some(Value::Text(text)) = store.get(crate::shell::WORKSPACE_STATE).map(|e| &e.value)
        else {
            *self = Seen::default();
            return;
        };
        if *text == self.text {
            return;
        }
        let json: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
        self.text.clone_from(text);
        self.own = serde_json::from_value(json["composites"].clone()).unwrap_or_default();
        self.nodes = json["template"]["network"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| {
                let r: CompositeRef = serde_json::from_value(n["composite"].clone()).ok()?;
                Some((n["name"].as_str()?.to_string(), r))
            })
            .collect();
    }
}

/// The Workshops panel: find, the list, the applied workshop.
pub struct Panel {
    find: String,
    library: Vec<CompositeFile>,
    shipped: Vec<CompositeFile>,
    seen: Seen,
}

impl Default for Panel {
    fn default() -> Self {
        Panel {
            find: String::new(),
            library: Vec::new(),
            shipped: helios_template::shipped::composites(),
            seen: Seen::default(),
        }
    }
}

/// The words a measure is shown with: what it measures and its unit.
pub fn measure_words(m: Measure) -> (Message, Message) {
    match m {
        Measure::LumaMedian => (
            Message::new("gui-workshops-measure-median", "midtones"),
            Message::new("gui-workshops-unit-stops", "stops"),
        ),
        Measure::LumaHigh => (
            Message::new("gui-workshops-measure-high", "highlights"),
            Message::new("gui-workshops-unit-stops", "stops"),
        ),
        Measure::ClipHigh => (
            Message::new("gui-workshops-measure-clip", "clipping"),
            Message::new("gui-workshops-unit-percent", "%"),
        ),
        Measure::ZoneContrast => (
            Message::new("gui-workshops-measure-contrast", "contrast"),
            Message::new("gui-workshops-unit-stops", "stops"),
        ),
        Measure::ZonesRatio => (
            Message::new("gui-workshops-measure-ratio", "gap between zones"),
            Message::new("gui-workshops-unit-stops", "stops"),
        ),
    }
}

impl Panel {
    /// The user's library, read again by the shell.
    pub fn set_library(&mut self, library: Vec<CompositeFile>) {
        self.library = library;
    }

    /// Draws the panel; `params` draws the applied workshop's sliders as
    /// the Parameters panel draws a macro's.
    pub fn ui(&mut self, ui: &mut egui::Ui, values: &Mutex<Store>, params: &mut Params) {
        let ctx = ui.ctx().clone();
        crate::name_rect(&ctx, PANEL, ui.max_rect());
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let selection = {
            let store = values.lock().unwrap_or_else(PoisonError::into_inner);
            self.seen.follow(&store);
            crate::modules::selected(&store)
        };
        ui.weak(text(Message::new(
            "gui-workshops-experimental",
            "Experimental: may change; what it saves stays a macro",
        )));
        ui.separator();
        let mut run: Option<(&str, serde_json::Value)> = None;
        let mut select: Option<String> = None;
        let find = ui
            .horizontal(|ui| {
                let l = ui.label(text(Message::new("gui-workshops-find", "Find")));
                ui.add(
                    egui::TextEdit::singleline(&mut self.find)
                        .hint_text(text(Message::new(
                            "gui-workshops-find-hint",
                            "what you want: brighter, less contrast…",
                        )))
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(l.id)
            })
            .inner;
        let entries = entries(&self.shipped, &self.library, &self.seen.own);
        let applied = |e: &Entry| -> Option<String> {
            self.seen
                .nodes
                .iter()
                .find(|(_, r)| r.id == e.file.id)
                .map(|(n, _)| n.clone())
        };
        // Found: ranked; nothing asked: grouped by where they come from.
        let shown: Vec<(&Entry, Option<Matched>)> = if self.find.trim().is_empty() {
            entries.iter().map(|e| (e, None)).collect()
        } else {
            suggest(entries.iter().map(|e| &e.file), &self.find)
                .into_iter()
                .filter_map(|s| {
                    let e = entries.iter().find(|e| e.file.reference() == s.workshop)?;
                    Some((e, Some(s.matched)))
                })
                .collect()
        };
        if find.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some((e, _)) = shown.first()
        {
            match applied(e) {
                Some(node) => select = Some(node),
                None => {
                    run = Some((
                        "workshop.apply",
                        json!({ "workshop": e.file.reference().to_string() }),
                    ));
                }
            }
        }
        if entries.is_empty() {
            ui.weak(text(Message::new(
                "gui-workshops-none",
                "No workshops in this workspace yet",
            )));
        } else if shown.is_empty() {
            ui.weak(text(Message::new(
                "gui-workshops-none-found",
                "No workshop found for these words",
            )));
        }
        let applied_now = selection.as_deref().and_then(|node| {
            let r = self.seen.nodes.get(node)?;
            let file = self
                .seen
                .own
                .iter()
                .chain(&self.library)
                .chain(&self.shipped)
                .find(|f| f.reference() == *r)?;
            file.workshop
                .is_some()
                .then(|| (node.to_string(), file.clone()))
        });
        ui.scroll_area(
            egui::ScrollArea::vertical()
                .id_salt(PANEL)
                .auto_shrink([false, true]),
            |ui| {
                let mut group = None;
                for (e, matched) in &shown {
                    if matched.is_none() && group != Some(e.origin) {
                        group = Some(e.origin);
                        ui.add_space(4.0);
                        ui.strong(text(match e.origin {
                            Origin::Shipped => {
                                Message::new("gui-workshops-shipped", "Shipped with Helios")
                            }
                            Origin::Library => {
                                Message::new("gui-workshops-library", "Your library")
                            }
                            Origin::Workspace => {
                                Message::new("gui-workshops-workspace", "This workspace")
                            }
                        }));
                    }
                    Self::row(ui, e, applied(e), &mut run, &mut select);
                }
                if let Some((node, file)) = &applied_now {
                    ui.separator();
                    applied_ui(ui, values, params, node, file, &mut run);
                }
            },
        );
        if let Some((id, args)) = run {
            request(ui.ctx(), Request::Run(id.into(), args, CommandOrigin::User));
        }
        if let Some(node) = select {
            let mut store = values.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = store.set(Set::new(
                crate::params::SELECTION,
                Change::To(Value::Text(node)),
                CommandOrigin::User,
            ));
        }
    }

    /// One workshop's row: its name and version, Apply (or, applied, its
    /// node to select), its intent on a second line.
    fn row(
        ui: &mut egui::Ui,
        e: &Entry,
        applied: Option<String>,
        run: &mut Option<(&str, serde_json::Value)>,
        select: &mut Option<String>,
    ) {
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let r = e.file.reference();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&e.file.name).strong());
            ui.weak(r.version.to_string());
            match &applied {
                Some(node) => {
                    if ui
                        .small_button(text(
                            Message::new("gui-workshops-applied", "Applied: { $node }")
                                .with("node", node),
                        ))
                        .on_hover_text(text(Message::new(
                            "gui-workshops-applied-hint",
                            "Select its node, to tune it below",
                        )))
                        .clicked()
                    {
                        *select = Some(node.clone());
                    }
                }
                None => {
                    if ui
                        .small_button(text(Message::new("gui-workshops-apply", "Apply")))
                        .on_hover_text(text(
                            Message::new(
                                "gui-workshops-apply-hint",
                                "Insert { $name } after the node the image is shown from, aimed on the open image (one undo step)",
                            )
                            .with("name", &e.file.name),
                        ))
                        .clicked()
                    {
                        *run = Some(("workshop.apply", json!({ "workshop": r.to_string() })));
                    }
                }
            }
        });
        if let Some(w) = &e.file.workshop {
            ui.weak(&w.intent);
        }
    }
}

/// The applied workshop `node`: its sliders by section, each targeted
/// one marked with its target; Aim again, Open in graph.
fn applied_ui(
    ui: &mut egui::Ui,
    values: &Mutex<Store>,
    params: &mut Params,
    node: &str,
    file: &CompositeFile,
    run: &mut Option<(&str, serde_json::Value)>,
) {
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let targets = file
        .workshop
        .as_ref()
        .map(|w| w.targets.clone())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.strong(node);
        ui.weak(file.reference().to_string());
    });
    let mut names: Vec<&str> = Vec::new();
    for x in &file.exposed {
        if !names.contains(&x.name.as_str()) {
            names.push(&x.name);
        }
    }
    let rows: Vec<(String, Option<String>)> = {
        let store = values.lock().unwrap_or_else(PoisonError::into_inner);
        names
            .iter()
            .map(|n| format!("node/{node}/{n}"))
            .filter_map(|p| {
                let group = store.get(&p)?.descriptor.group.clone();
                Some((p, group))
            })
            .collect()
    };
    if rows.is_empty() {
        ui.weak(text(Message::new(
            "gui-workshops-no-image",
            "Open an image to tune it",
        )));
    }
    let refs: Vec<(&str, Option<&str>)> = rows
        .iter()
        .map(|(p, g)| (p.as_str(), g.as_deref()))
        .collect();
    for (group, paths) in crate::params::sections(&refs) {
        if let Some(group) = group {
            ui.label(egui::RichText::new(group).strong());
        }
        for path in paths {
            params.param_ui(ui, values, path);
            let name = path.rsplit('/').next().unwrap_or_default();
            if let Some(t) = targets.iter().find(|t| t.param == name) {
                let (what, unit) = measure_words(t.measure);
                // In a zone: "sky midtones", "foreground against sky".
                let what = match (&t.zone, &t.against) {
                    (Some(z), Some(a)) => format!("{} ({z} − {a})", text(what)),
                    (Some(z), None) => format!("{z} {}", text(what)),
                    _ => text(what),
                };
                ui.weak(text(
                    Message::new(
                        "gui-workshops-aimed",
                        "aimed: { $measure } at { $value } { $unit }",
                    )
                    .with("measure", what)
                    .with("value", format!("{:.2}", t.value))
                    .with("unit", text(unit)),
                ));
            }
        }
    }
    ui.horizontal(|ui| {
        if !targets.is_empty()
            && ui
                .button(text(Message::new("gui-workshops-aim", "Aim again")))
                .on_hover_text(text(Message::new(
                    "gui-workshops-aim-hint",
                    "Solve its targets again on the open image (one undo step)",
                )))
                .clicked()
        {
            *run = Some(("workshop.aim", json!({ "node": node })));
        }
        if ui
            .button(text(Message::new("gui-workshops-open", "Open in graph")))
            .clicked()
        {
            *run = Some(("macro.open", json!({ "node": node })));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_on_from_the_user_layer_or_the_run() {
        use crate::config::{Settings, Sources};
        let resolve = |user: Option<&str>, run: &[(String, String)]| {
            let s = Settings::resolve(&Sources {
                user: user.map(String::from),
                run: run.to_vec(),
                ..Sources::default()
            });
            (enabled(s.config()), s.problems().len())
        };
        assert_eq!(resolve(None, &[]), (false, 0));
        let on = "[interface.experimental]\nworkshops = true\n";
        assert_eq!(resolve(Some(on), &[]), (true, 0));
        let run = run_layer(&[
            "--set".into(),
            "interface.experimental.workshops=true".into(),
        ])
        .unwrap();
        assert_eq!(resolve(None, &run), (true, 0));
        // The run layer wins over the user's.
        let off = run_layer(&["--set".into(), format!("{SETTING}=false")]).unwrap();
        assert_eq!(resolve(Some(on), &off), (false, 0));
        // A value that does not check is left out, and said.
        assert_eq!(
            resolve(Some("[interface.experimental]\nworkshops = 3\n"), &[]),
            (false, 1)
        );
        assert!(run_layer(&["--set".into()]).is_err());
        assert!(run_layer(&["--set".into(), "nokey".into()]).is_err());
    }
}
