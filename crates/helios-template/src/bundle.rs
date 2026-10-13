//! The `.helios-template` bundle on disk: read, written, checked against
//! a catalogue and built.

use std::collections::BTreeSet;
use std::path::Path;

use egui_pop_model::ids::PanelId;
use egui_pop_model::layout::Layout;
use helios_core::Image;
use serde_json::Value as Json;

use crate::build::{self, Built};
use crate::catalogue::Catalogue;
use crate::format::{
    COMPOSITE_EXTENSION, CompositeFile, CompositeRef, FORMAT, Template, slug, valid_id, valid_name,
};

/// The template's description in a bundle.
pub const TEMPLATE_FILE: &str = "template.json";
/// Its layout: egui-pop's layout document.
pub const LAYOUT_FILE: &str = "layout.json";
/// Where its translations are: `i18n/<locale>/*.ftl`.
pub const I18N_DIR: &str = "i18n";
/// Where the composites it uses are bundled.
pub const COMPOSITES_DIR: &str = "composites";
/// A bundle directory's name ends so.
pub const EXTENSION: &str = ".helios-template";

/// One Fluent file of a template: its locale, its name and its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub locale: String,
    pub name: String,
    pub source: String,
}

/// A template with everything its bundle holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    pub template: Template,
    pub layout: Layout,
    pub texts: Vec<Text>,
    /// The composites bundled with it (they join the catalogue it is
    /// checked and built against).
    pub composites: Vec<CompositeFile>,
    /// The bytes of `meta.preview`, when the bundle has it.
    pub preview: Option<Vec<u8>>,
}

/// A composite pinned at a version older than the catalogue's newest:
/// offered, never taken silently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// The template's node that pins it.
    pub node: String,
    pub composite: CompositeRef,
    pub available: u32,
}

/// What checking a template found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Each problem with where it is; the template is refused while there
    /// is one.
    pub problems: Vec<String>,
    /// Newer composites on offer.
    pub updates: Vec<Update>,
    /// What building it changed (a panel docked for a composite's hint).
    pub notes: Vec<String>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Reads `template.json`'s text: a format newer than this Helios reads is
/// refused before its shape is.
pub fn parse_template(text: &str) -> Result<Template, String> {
    let json = versioned(text, "template")?;
    serde_json::from_value(json).map_err(|e| format!("template: {e}"))
}

/// Reads a composite file's text, as [`parse_template`].
pub fn parse_composite(text: &str) -> Result<CompositeFile, String> {
    let json = versioned(text, "composite")?;
    serde_json::from_value(json).map_err(|e| format!("composite: {e}"))
}

fn versioned(text: &str, what: &str) -> Result<Json, String> {
    let json: Json = serde_json::from_str(text).map_err(|e| format!("{what}: {e}"))?;
    match json.get("format").and_then(Json::as_u64) {
        Some(f) if f > u64::from(FORMAT) => Err(format!(
            "{what}: format {f} is newer than this Helios reads ({FORMAT})"
        )),
        Some(_) => Ok(json),
        None => Err(format!("{what}: no `format`")),
    }
}

impl Bundle {
    /// A bundle of a template and its layout, without texts.
    pub fn new(template: Template, layout: Layout) -> Self {
        Self {
            template,
            layout,
            texts: Vec::new(),
            composites: Vec::new(),
            preview: None,
        }
    }

    /// A bundle from its files' text, by their paths in the bundle
    /// (`template.json`, `layout.json`, `i18n/fr/template.ftl`,
    /// `composites/<file>`): what Helios ships built in.
    pub fn from_files(files: &[(&str, &str)]) -> Result<Self, String> {
        let file = |name: &str| {
            files
                .iter()
                .find(|(p, _)| *p == name)
                .map(|(_, t)| *t)
                .ok_or_else(|| format!("no {name}"))
        };
        let mut bundle = Self::new(
            parse_template(file(TEMPLATE_FILE)?)?,
            Layout::from_json(file(LAYOUT_FILE)?)?,
        );
        for (path, text) in files {
            let parts: Vec<&str> = path.split('/').collect();
            match parts.as_slice() {
                [I18N_DIR, locale, name]
                    if Path::new(name).extension().is_some_and(|e| e == "ftl") =>
                {
                    bundle.texts.push(Text {
                        locale: (*locale).into(),
                        name: (*name).into(),
                        source: (*text).into(),
                    });
                }
                [COMPOSITES_DIR, name] if name.ends_with(COMPOSITE_EXTENSION) => bundle
                    .composites
                    .push(parse_composite(text).map_err(|e| format!("{path}: {e}"))?),
                _ => {}
            }
        }
        bundle
            .texts
            .sort_by(|a, b| (&a.locale, &a.name).cmp(&(&b.locale, &b.name)));
        Ok(bundle)
    }

    /// Reads a bundle directory.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let read =
            |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
        let template = parse_template(&read(&dir.join(TEMPLATE_FILE))?)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        let layout = Layout::from_json(&read(&dir.join(LAYOUT_FILE))?)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut bundle = Self::new(template, layout);
        for locale_dir in sorted(&dir.join(I18N_DIR)) {
            if !locale_dir.is_dir() {
                continue;
            }
            let locale = file_name(&locale_dir);
            for file in sorted(&locale_dir) {
                if file.extension().is_some_and(|e| e == "ftl") {
                    bundle.texts.push(Text {
                        locale: locale.clone(),
                        name: file_name(&file),
                        source: read(&file)?,
                    });
                }
            }
        }
        for file in sorted(&dir.join(COMPOSITES_DIR)) {
            if file_name(&file).ends_with(COMPOSITE_EXTENSION) {
                bundle.composites.push(
                    parse_composite(&read(&file)?)
                        .map_err(|e| format!("{}: {e}", file.display()))?,
                );
            }
        }
        if let Some(name) = &bundle.template.meta.preview {
            let path = dir.join(name);
            bundle.preview =
                Some(std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        }
        Ok(bundle)
    }

    /// Writes the bundle into `dir` (created), every file it holds.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let write = |path: &Path, bytes: &[u8]| {
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(path, bytes))
                .map_err(|e| format!("{}: {e}", path.display()))
        };
        let template =
            serde_json::to_string_pretty(&self.template).map_err(|e| format!("template: {e}"))?;
        write(&dir.join(TEMPLATE_FILE), format!("{template}\n").as_bytes())?;
        write(
            &dir.join(LAYOUT_FILE),
            format!("{}\n", self.layout.to_json()).as_bytes(),
        )?;
        for t in &self.texts {
            let path = dir.join(I18N_DIR).join(&t.locale).join(&t.name);
            write(&path, t.source.as_bytes())?;
        }
        for c in &self.composites {
            let text = serde_json::to_string_pretty(c).map_err(|e| format!("composite: {e}"))?;
            let path = dir.join(COMPOSITES_DIR).join(c.file_name());
            write(&path, format!("{text}\n").as_bytes())?;
        }
        if let (Some(name), Some(bytes)) = (&self.template.meta.preview, &self.preview) {
            write(&dir.join(name), bytes)?;
        }
        Ok(())
    }

    /// The message key of its translated name.
    pub fn name_key(&self) -> String {
        format!("{}-name", slug(&self.template.id))
    }

    /// The message key of its translated scenario.
    pub fn scenario_key(&self) -> String {
        format!("{}-scenario", slug(&self.template.id))
    }

    /// The message key of a parameter's translated name.
    pub fn parameter_key(&self, id: &str) -> String {
        format!("{}-parameter-{}", slug(&self.template.id), slug(id))
    }

    /// `base` with the bundled composites a catalogue lacks.
    pub fn catalogue(&self, base: &Catalogue) -> Catalogue {
        let mut c = base.clone();
        for file in &self.composites {
            if c.get_composite(&file.reference()).is_none() {
                c.add_composite(file.clone());
            }
        }
        c
    }

    /// Bundles every composite its network uses, inner ones included, as
    /// the catalogue has them: the bundle then builds anywhere.
    pub fn bundle_composites(&mut self, catalogue: &Catalogue) -> Result<(), String> {
        let catalogue = self.catalogue(catalogue);
        let mut wanted: Vec<CompositeRef> = self
            .template
            .network
            .nodes
            .iter()
            .filter_map(|n| n.composite.clone())
            .collect();
        let mut seen = BTreeSet::new();
        let mut files = Vec::new();
        while let Some(r) = wanted.pop() {
            if !seen.insert(r.clone()) {
                continue;
            }
            let file = catalogue
                .get_composite(&r)
                .ok_or_else(|| format!("unknown composite `{r}`"))?;
            wanted.extend(file.nodes.iter().filter_map(|n| n.composite.clone()));
            files.push(file.clone());
        }
        files.sort_by_key(CompositeFile::reference);
        self.composites = files;
        Ok(())
    }

    /// Pins every composite to the catalogue's newest version: what the
    /// desktop or an agent does when someone takes the updates on offer.
    /// Returns what moved.
    pub fn upgrade_composites(&mut self, catalogue: &Catalogue) -> Vec<Update> {
        let catalogue = self.catalogue(catalogue);
        let mut moved = Vec::new();
        for n in &mut self.template.network.nodes {
            if let Some(r) = &mut n.composite
                && let Some(v) = catalogue.newer(r)
            {
                moved.push(Update {
                    node: n.name.clone(),
                    composite: r.clone(),
                    available: v,
                });
                r.version = v;
            }
        }
        moved
    }

    /// Checks the template against what `catalogue` knows (its bundled
    /// composites added), then builds it headless.
    pub fn validate(&self, catalogue: &Catalogue) -> Report {
        let catalogue = self.catalogue(catalogue);
        let mut r = Report::default();
        let t = &self.template;
        let p = &mut r.problems;
        if t.format != FORMAT {
            p.push(format!("format: {} is not {FORMAT}", t.format));
        }
        if !valid_id(&t.id) {
            p.push(format!("id: `{}` is not an id", t.id));
        }
        if t.version == 0 {
            p.push("version: starts at 1".into());
        }
        if t.meta.name.trim().is_empty() {
            p.push("meta.name: empty".into());
        }
        if t.meta.scenario.trim().is_empty() {
            p.push("meta.scenario: empty".into());
        }
        let net = &t.network;
        let mut names = BTreeSet::new();
        for (i, s) in net.sources.iter().enumerate() {
            if !valid_name(s) || !names.insert(s.as_str()) {
                p.push(format!(
                    "network.sources[{i}]: `{s}` is not a name, or not the only one"
                ));
            }
        }
        for (i, n) in net.nodes.iter().enumerate() {
            let at = format!("network.nodes[{i}] ({})", n.name);
            if !valid_name(&n.name) || !names.insert(n.name.as_str()) {
                p.push(format!("{at}: not a name, or not the only one"));
            }
            match (&n.module, &n.composite) {
                (Some(m), None) => {
                    if catalogue.get_module(m).is_none() {
                        p.push(format!("{at}: unknown module `{m}`"));
                    }
                    if n.open || n.panel.is_some() {
                        p.push(format!("{at}: only a composite opens or has a panel hint"));
                    }
                }
                (None, Some(c)) => {
                    if catalogue.get_composite(c).is_none() {
                        p.push(format!("{at}: unknown composite `{c}`"));
                    }
                    if let Some(v) = catalogue.newer(c) {
                        r.updates.push(Update {
                            node: n.name.clone(),
                            composite: c.clone(),
                            available: v,
                        });
                    }
                }
                _ => p.push(format!("{at}: names a module or a composite, one of them")),
            }
            if let Some(h) = &n.panel {
                check_panel(
                    &catalogue,
                    &h.panel,
                    Some(h.area),
                    &format!("{at}.panel"),
                    p,
                );
            }
        }
        for (i, c) in net.connections.iter().enumerate() {
            for end in [&c.from, &c.to] {
                if !names.contains(end.as_str()) {
                    p.push(format!("network.connections[{i}]: no node `{end}`"));
                }
            }
        }
        if !names.contains(net.view.node.as_str()) {
            p.push(format!("network.view: no node `{}`", net.view.node));
        }
        let mut presets = BTreeSet::new();
        for (i, preset) in net.presets.iter().enumerate() {
            if !presets.insert(preset.id.as_str()) {
                p.push(format!(
                    "network.presets[{i}]: a second preset `{}`",
                    preset.id
                ));
            }
            for node in preset.values.keys() {
                if !names.contains(node.as_str()) {
                    p.push(format!(
                        "network.presets[{i}] ({}): no node `{node}`",
                        preset.id
                    ));
                }
            }
        }
        let tools = &t.tools;
        for (i, c) in tools.toolbox.iter().enumerate() {
            if !catalogue.knows_command(c) {
                p.push(format!("tools.toolbox[{i}]: unknown command `{c}`"));
            }
        }
        if let Some(d) = &tools.default
            && !tools.toolbox.contains(d)
        {
            p.push(format!("tools.default: `{d}` is not in the toolbox"));
        }
        for (i, b) in tools.keymap.iter().enumerate() {
            if b.keys.trim().is_empty() {
                p.push(format!("tools.keymap[{i}]: no keys"));
            }
            if !catalogue.knows_command(&b.command) {
                p.push(format!(
                    "tools.keymap[{i}]: unknown command `{}`",
                    b.command
                ));
            }
        }
        if let Some(import) = t.library.as_ref().and_then(|l| l.import.as_ref()) {
            check_import(import, p);
        }
        check_parameters(t, p);
        for (area, node) in &self.layout.areas {
            for panel in node.panels() {
                check_panel(&catalogue, panel.as_str(), Some(*area), "layout", p);
            }
        }
        for a in &self.layout.auxiliary {
            check_panel(&catalogue, a.panel.as_str(), None, "layout.auxiliary", p);
        }
        for h in &self.layout.hidden {
            check_panel(&catalogue, h.as_str(), None, "layout.hidden", p);
        }
        for f in &self.layout.floating {
            check_panel(&catalogue, f.panel.as_str(), None, "layout.floating", p);
            let specs = catalogue.panel_specs();
            if let Some(spec) = specs.iter().find(|s| s.id == f.panel)
                && !spec.floatable
            {
                p.push(format!("layout.floating: `{}` does not float", f.panel));
            }
        }
        let own = format!("{}-", slug(&t.id));
        for text in &self.texts {
            for key in keys(&text.source) {
                if !key.starts_with('-') && !key.starts_with(&own) {
                    p.push(format!(
                        "i18n/{}/{}: `{key}` is not the template's (keys start with `{own}`)",
                        text.locale, text.name
                    ));
                }
            }
        }
        if r.problems.is_empty() {
            let source = build::placeholder();
            match build::build(t, &self.layout, &catalogue, &source, None) {
                Ok(built) => r.notes = built.notes,
                Err(e) => r.problems.push(e),
            }
            for preset in &net.presets {
                let built = build::build(
                    t,
                    &self.layout,
                    &catalogue,
                    &source,
                    Some(preset.id.as_str()),
                );
                if let Err(e) = built {
                    r.problems.push(format!("preset `{}`: {e}", preset.id));
                }
            }
        }
        r
    }

    /// Builds the template headless, its sources a small placeholder.
    pub fn build(&self, catalogue: &Catalogue) -> Result<Built, String> {
        self.build_with(catalogue, &build::placeholder(), None)
    }

    /// Builds the template, each source given `source`, with a preset's
    /// values over the nodes' own.
    pub fn build_with(
        &self,
        catalogue: &Catalogue,
        source: &Image,
        preset: Option<&str>,
    ) -> Result<Built, String> {
        build::build(
            &self.template,
            &self.layout,
            &self.catalogue(catalogue),
            source,
            preset,
        )
    }
}

/// A panel a template names: known, and allowed where it is put.
fn check_panel(
    catalogue: &Catalogue,
    panel: &str,
    area: Option<egui_pop_model::panels::Area>,
    at: &str,
    problems: &mut Vec<String>,
) {
    if PanelId::new(panel).is_err() {
        problems.push(format!("{at}: `{panel}` is not a panel id"));
        return;
    }
    let specs = catalogue.panel_specs();
    if specs.is_empty() {
        return;
    }
    match specs.iter().find(|s| s.id.as_str() == panel) {
        None => problems.push(format!("{at}: unknown panel `{panel}`")),
        Some(spec) => {
            if let Some(why) = area.and_then(|a| spec.refuses(a)) {
                problems.push(format!("{at}: {why}"));
            }
        }
    }
}

/// The message and term ids a Fluent source defines.
fn keys(source: &str) -> Vec<String> {
    source
        .lines()
        .filter(|l| l.starts_with(|c: char| c.is_ascii_alphabetic() || c == '-'))
        .filter_map(|l| l.split('=').next())
        .map(|k| k.trim().to_string())
        .collect()
}

/// A directory's entries, sorted; none when it is missing.
fn sorted(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

/// What a template asks on create: each parameter named once, pointing
/// at a value the template has (never at what identifies it), a choice
/// with its words.
fn check_parameters(t: &Template, p: &mut Vec<String>) {
    let json = serde_json::to_value(t).unwrap_or_default();
    let mut ids = BTreeSet::new();
    for (i, q) in t.parameters.iter().enumerate() {
        let at = format!("parameters[{i}] ({})", q.id);
        if !valid_name(&q.id) || !ids.insert(q.id.as_str()) {
            p.push(format!("{at}: not a name, or not the only one"));
        }
        if q.name.trim().is_empty() {
            p.push(format!("{at}: no name"));
        }
        let fixed = ["/format", "/id", "/version", "/parameters"];
        if !q.at.starts_with('/') || fixed.iter().any(|f| q.at.starts_with(f)) {
            p.push(format!("{at}: `{}` is not a value of the template", q.at));
        } else {
            match json.pointer(&q.at) {
                Some(Json::String(v)) => {
                    if q.kind == crate::format::ParameterKind::Choice && !q.choices.contains(v) {
                        p.push(format!("{at}: `{v}` is not one of its choices"));
                    }
                }
                Some(_) => p.push(format!("{at}: `{}` is not a text value", q.at)),
                None => p.push(format!("{at}: the template has nothing at `{}`", q.at)),
            }
        }
        if (q.kind == crate::format::ParameterKind::Choice) == q.choices.is_empty() {
            p.push(format!(
                "{at}: only a choice has choices, and it needs some"
            ));
        }
    }
}

/// An import source's words: the source, the placement, the duplicates'
/// policy, a renaming only for copies.
fn check_import(import: &crate::format::ImportDesc, p: &mut Vec<String>) {
    use crate::format::{DUPLICATES, IMPORT_SOURCES, PLACEMENTS};
    for (field, value, known) in [
        ("source", &import.source, &IMPORT_SOURCES[..]),
        ("placement", &import.placement, &PLACEMENTS[..]),
        ("duplicates", &import.duplicates, &DUPLICATES[..]),
    ] {
        if !known.contains(&value.as_str()) {
            p.push(format!(
                "library.import.{field}: `{value}` is not one of {}",
                known.join(", ")
            ));
        }
    }
    if let Some(rename) = &import.rename {
        if import.placement != "copy" {
            p.push("library.import.rename: only a copy is renamed".into());
        }
        if rename.trim().is_empty() || rename.contains(['/', '\\']) {
            p.push(format!(
                "library.import.rename: `{rename}` is not a file name pattern"
            ));
        }
    }
}
