//! `cargo xtask deps-worlds`: every workspace crate belongs to a world, and
//! a world says which UI crates and which other worlds its crates may
//! depend on (docs/design/ui-responsibilities.md, section 9). Forms crates
//! reach the screen through helios-ui over egui-pop, visual-module crates
//! through helios-rack and `egui_hud`'s logic; declarations and the engine
//! never touch egui or egui-pop's widgets; only the frameworks, the shell
//! and the developer tools may.
//!
//! The developer tools (`dev-tool`: xtask, the controls gallery, the i18n
//! extractor) have no UI rule, so they must stay out of what ships: a
//! `dev-tool` crate is `publish = false`, and no crate of another world
//! depends on one (a dev-dependency, for tests, may).
//!
//! It also refuses a `Cargo.lock` holding two versions of an egui-family
//! crate (egui, eframe, every `egui-pop-*`, `egui_hud`, `egui_fit`…): two
//! egui-pops in one graph draw with two sets of tokens and two contexts.
//!
//! And it refuses a crate with committed snapshots (`tests/snapshots/*.png`)
//! that resolves a drawing feature ([`RENDER_FEATURES`]: the icon font, the
//! font sets) differently built alone (`cargo test -p`) and in the
//! workspace (`--workspace`, where another crate's features unify in): its
//! images would pass in one run and fail in the other. The crate names the
//! feature itself.
//!
//! The edges that break their world today are listed, each with its
//! reason, in `xtask/allow/deps-worlds.toml`; the list only shrinks (an
//! entry whose edge is gone is reported).

use std::path::Path;
use std::process::{Command, ExitCode};

pub const USAGE: &str = "deps-worlds";

/// The allow-list, relative to the workspace root.
pub const ALLOW_LIST: &str = "xtask/allow/deps-worlds.toml";

/// A crate's world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum World {
    /// No UI: the engine, storage, the node, the shell's GUI-free logic.
    Engine,
    /// Form-like declarations (plugins' settings contracts): reach the
    /// screen through helios-ui only.
    Forms,
    /// Visual-module declarations (modules, the controls catalogue): reach
    /// the screen through helios-rack and `egui_hud`'s logic only.
    Visual,
    /// helios-ui: Helios's form intents over egui-pop.
    FormsFramework,
    /// helios-rack: the frame of a visual module, over `egui_hud`.
    VisualFramework,
    /// helios-gui: the application shell.
    Shell,
    /// Developer tools, benches and generators, never shipped: no UI rule,
    /// but `publish = false` and no crate of another world depends on one
    /// (see [`dev_tool_problems`]).
    DevTool,
}

impl World {
    fn name(self) -> &'static str {
        match self {
            World::Engine => "engine",
            World::Forms => "forms",
            World::Visual => "visual",
            World::FormsFramework => "forms-framework",
            World::VisualFramework => "visual-framework",
            World::Shell => "shell",
            World::DevTool => "dev-tool",
        }
    }

    /// The UI crates (see [`is_ui_crate`]) its crates may depend on.
    fn ui_allowed(self) -> &'static [&'static str] {
        match self {
            // egui-pop-model is egui-free: ids, layouts, the panel model.
            World::Engine | World::Forms => &["egui-pop-model"],
            World::Visual => &["egui-pop-model", "egui_hud", "egui_fit"],
            World::FormsFramework => &["egui-pop", "egui-pop-model", "egui-pop-i18n"],
            World::VisualFramework => &[
                "egui-pop",
                "egui-pop-model",
                "egui-pop-i18n",
                "egui_hud",
                "egui_fit",
            ],
            World::Shell => &[
                "eframe",
                "egui-pop",
                "egui-pop-agent",
                "egui-pop-model",
                "egui-pop-i18n",
            ],
            World::DevTool => &[],
        }
    }

    /// Whether `egui_hud`'s drawing features (`render`, `bench`) are allowed.
    fn hud_render_allowed(self) -> bool {
        matches!(self, World::VisualFramework | World::DevTool)
    }

    /// Whether test harnesses are allowed as dev-dependencies.
    fn harness_allowed(self) -> bool {
        matches!(
            self,
            World::FormsFramework | World::VisualFramework | World::Shell
        )
    }

    /// The worlds whose crates its crates may depend on.
    fn uses(self) -> &'static [World] {
        use World::{Engine, Forms, FormsFramework, Shell, Visual, VisualFramework};
        match self {
            // Forms and visual declarations are UI-free: the engine reads
            // them (the node runs the modules).
            Engine => &[Engine, Forms, Visual],
            Forms | FormsFramework => &[Engine, Forms, FormsFramework],
            Visual | VisualFramework => &[Engine, Visual, VisualFramework],
            Shell => &[
                Engine,
                Forms,
                Visual,
                FormsFramework,
                VisualFramework,
                Shell,
            ],
            World::DevTool => &[],
        }
    }
}

/// Crates whose world is not the default of their folder.
const NAMED: &[(&str, World)] = &[
    ("helios-ui", World::FormsFramework),
    ("helios-rack", World::VisualFramework),
    ("helios-gui", World::Shell),
    ("helios-controls", World::Visual),
    // The i18n extractor: a binary that builds a headless shell to collect
    // every declared text.
    ("helios-i18n-extract", World::DevTool),
    // egui_hud's bench, on Helios's controls.
    ("helios-controls-gallery", World::DevTool),
    ("xtask", World::DevTool),
];

/// A crate's world: named in [`NAMED`], else by folder: `modules/` visual,
/// `plugins/` forms, `crates/` engine. A new UI crate under `crates/` thus
/// fails until it is named here.
pub fn world_of(name: &str, rel_dir: &str) -> World {
    if let Some((_, w)) = NAMED.iter().find(|(n, _)| *n == name) {
        return *w;
    }
    if rel_dir.starts_with("modules/") {
        World::Visual
    } else if rel_dir.starts_with("plugins/") {
        World::Forms
    } else if rel_dir.starts_with("crates/") {
        World::Engine
    } else {
        World::DevTool
    }
}

/// egui and every crate built on it or for it.
pub fn is_ui_crate(name: &str) -> bool {
    name == "eframe" || name.starts_with("egui")
}

/// Test harnesses, allowed as dev-dependencies of the UI worlds.
const HARNESSES: &[&str] = &["egui_kittest", "egui-pop-test", "egui-pop-wayland"];

/// One dependency of a workspace crate, as `cargo metadata` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    /// `None` for a normal dependency, else `dev` or `build`.
    pub kind: Option<String>,
    pub features: Vec<String>,
}

/// A workspace crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Krate {
    pub name: String,
    /// Its folder, relative to the workspace root (`crates/helios-ui`).
    pub dir: String,
    /// Whether it may be published (`publish = false` makes it `false`).
    pub publishable: bool,
    pub deps: Vec<Dep>,
}

/// An edge that breaks its crate's world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub krate: String,
    pub dep: String,
    pub why: String,
}

/// The workspace crates, read from `cargo metadata --no-deps` output.
pub fn parse_metadata(json: &str, root: &Path) -> Result<Vec<Krate>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("cargo metadata: {e}"))?;
    let root = root.to_string_lossy().trim_end_matches('/').to_string() + "/";
    let mut out = Vec::new();
    for p in v["packages"]
        .as_array()
        .ok_or("cargo metadata: no packages")?
    {
        let name = p["name"].as_str().unwrap_or_default().to_string();
        let manifest = p["manifest_path"].as_str().unwrap_or_default();
        let dir = manifest
            .strip_prefix(&root)
            .unwrap_or(manifest)
            .trim_end_matches("Cargo.toml")
            .trim_end_matches('/')
            .to_string();
        let deps = p["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| Dep {
                name: d["name"].as_str().unwrap_or_default().to_string(),
                kind: d["kind"].as_str().map(str::to_string),
                features: d["features"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect(),
            })
            .collect();
        // `publish = false` reads as an empty registry list; absent, `null`.
        let publishable = p["publish"]
            .as_array()
            .is_none_or(|registries| !registries.is_empty());
        out.push(Krate {
            name,
            dir,
            publishable,
            deps,
        });
    }
    Ok(out)
}

/// Every edge of the workspace that breaks its crate's world.
pub fn violations(krates: &[Krate]) -> Vec<Edge> {
    let mut out = Vec::new();
    for k in krates {
        let world = world_of(&k.name, &k.dir);
        if world == World::DevTool {
            continue;
        }
        let mut push = |dep: &str, why: String| {
            let edge = Edge {
                krate: k.name.clone(),
                dep: dep.to_string(),
                why,
            };
            if !out.contains(&edge) {
                out.push(edge);
            }
        };
        for d in &k.deps {
            if let Some(target) = krates.iter().find(|t| t.name == d.name) {
                if target.name == k.name {
                    continue;
                }
                let tw = world_of(&target.name, &target.dir);
                // Edges to a developer tool are judged by `dev_tool_problems`,
                // which no allow-list entry can cover.
                if tw == World::DevTool {
                    continue;
                }
                if !world.uses().contains(&tw) {
                    push(
                        &d.name,
                        format!(
                            "a {} crate may not depend on a {} crate",
                            world.name(),
                            tw.name()
                        ),
                    );
                }
                continue;
            }
            if !is_ui_crate(&d.name) {
                continue;
            }
            let dev = d.kind.as_deref() == Some("dev");
            let allowed = world.ui_allowed().contains(&d.name.as_str())
                || (dev && world.harness_allowed() && HARNESSES.contains(&d.name.as_str()));
            if !allowed {
                push(
                    &d.name,
                    format!(
                        "a {} crate may depend on {} only",
                        world.name(),
                        list_or_none(world.ui_allowed())
                    ),
                );
                continue;
            }
            if d.name == "egui_hud" && !world.hud_render_allowed() {
                let drawing = d
                    .features
                    .iter()
                    .find(|f| matches!(f.as_str(), "render" | "bench"));
                if let Some(f) = drawing {
                    push(
                        &d.name,
                        format!(
                            "egui_hud's `{f}` feature draws: a {} crate goes through helios-rack",
                            world.name()
                        ),
                    );
                }
            }
        }
    }
    out
}

/// The developer tools that could ship: a `dev-tool` crate that is not
/// `publish = false`, and a crate of another world depending on one (a
/// dev-dependency, for its tests, may). Not covered by the allow-list.
pub fn dev_tool_problems(krates: &[Krate]) -> Vec<String> {
    let is_dev_tool = |k: &Krate| world_of(&k.name, &k.dir) == World::DevTool;
    let mut out = Vec::new();
    for k in krates {
        if is_dev_tool(k) {
            if k.publishable {
                out.push(format!(
                    "`{}` is a dev-tool crate: it must be `publish = false`",
                    k.name
                ));
            }
            continue;
        }
        for d in &k.deps {
            if d.kind.as_deref() == Some("dev") {
                continue;
            }
            let on_tool = krates
                .iter()
                .any(|t| t.name == d.name && t.name != k.name && is_dev_tool(t));
            let edge = format!(
                "`{}` depends on `{}`: a {} crate may not depend on a dev-tool crate (a dev-dependency may)",
                k.name,
                d.name,
                world_of(&k.name, &k.dir).name()
            );
            if on_tool && !out.contains(&edge) {
                out.push(edge);
            }
        }
    }
    out
}

fn list_or_none(names: &[&str]) -> String {
    if names.is_empty() {
        "no UI crate".into()
    } else {
        names.join(", ")
    }
}

/// One allow-list entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub krate: String,
    pub dep: String,
    pub reason: String,
}

/// Reads `xtask/allow/deps-worlds.toml`: `[[allow]]` entries with `crate`,
/// `dep` and a non-empty `reason`, all quoted strings.
pub fn parse_allow_list(text: &str) -> Result<Vec<Allowed>, String> {
    let mut list: Vec<Allowed> = Vec::new();
    let mut cur: Option<[Option<String>; 3]> = None;
    let finish = |cur: Option<[Option<String>; 3]>, list: &mut Vec<Allowed>| {
        let Some([krate, dep, reason]) = cur else {
            return Ok(());
        };
        let krate = krate.ok_or("an [[allow]] entry has no `crate`")?;
        let dep = dep.ok_or_else(|| format!("`{krate}`: an entry has no `dep`"))?;
        let reason = reason
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| format!("`{krate}` -> `{dep}` has no `reason`"))?;
        if list.iter().any(|a| a.krate == krate && a.dep == dep) {
            return Err(format!("`{krate}` -> `{dep}` is listed twice"));
        }
        list.push(Allowed { krate, dep, reason });
        Ok::<(), String>(())
    };
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[allow]]" {
            finish(cur.take(), &mut list)?;
            cur = Some([None, None, None]);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `key = \"value\"`", n + 1));
        };
        let value = value
            .trim()
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("line {}: the value must be a quoted string", n + 1))?
            .to_string();
        let Some(entry) = cur.as_mut() else {
            return Err(format!("line {}: a key outside an [[allow]] entry", n + 1));
        };
        let slot = match key.trim() {
            "crate" => 0,
            "dep" => 1,
            "reason" => 2,
            other => return Err(format!("line {}: unknown key `{other}`", n + 1)),
        };
        entry[slot] = Some(value);
    }
    finish(cur, &mut list)?;
    Ok(list)
}

/// The violations the allow-list does not cover, and the entries that no
/// longer match an edge.
pub fn judge(edges: &[Edge], allow: &[Allowed]) -> Vec<String> {
    let mut problems: Vec<String> = edges
        .iter()
        .filter(|e| !allow.iter().any(|a| a.krate == e.krate && a.dep == e.dep))
        .map(|e| format!("`{}` depends on `{}`: {}", e.krate, e.dep, e.why))
        .collect();
    problems.extend(
        allow
            .iter()
            .filter(|a| !edges.iter().any(|e| a.krate == e.krate && a.dep == e.dep))
            .map(|a| {
                format!(
                    "{ALLOW_LIST}: `{}` -> `{}` is within its world now (or gone): remove its entry",
                    a.krate, a.dep
                )
            }),
    );
    problems
}

/// The egui-family crates `Cargo.lock` holds in more than one version.
pub fn lock_duplicates(lock: &str) -> Vec<String> {
    let mut seen: Vec<(String, Vec<String>)> = Vec::new();
    let mut name: Option<String> = None;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
        } else if let Some(v) = quoted(line, "name") {
            name = Some(v);
        } else if let Some(v) = quoted(line, "version")
            && let Some(n) = name.take().filter(|n| is_ui_crate(n))
        {
            match seen.iter_mut().find(|(s, _)| *s == n) {
                Some((_, versions)) if !versions.contains(&v) => versions.push(v),
                Some(_) => {}
                None => seen.push((n, vec![v])),
            }
        }
    }
    seen.into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(n, v)| {
            format!(
                "Cargo.lock holds {} versions of `{n}` ({}): one egui-pop family, one egui",
                v.len(),
                v.join(", ")
            )
        })
        .collect()
}

fn quoted(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
    let v = rest.trim().strip_prefix('"')?.strip_suffix('"')?;
    Some(v.to_string())
}

/// The features that change what a frame draws: the icon font and the
/// font sets, as `(package, feature)`.
pub const RENDER_FEATURES: &[(&str, &str)] = &[
    ("egui-pop-core", "phosphor"),
    ("egui-pop-core", "bundled-fonts"),
    ("egui-pop-core", "system-fonts"),
    ("egui", "default_fonts"),
    ("egui", "monochrome_emoji_fonts"),
    ("epaint", "default_fonts"),
    ("epaint", "monochrome_emoji_fonts"),
];

/// The [`RENDER_FEATURES`] a `cargo tree -e features --prefix none` output
/// turns on, and the packages it holds.
fn render_features(tree: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut on = Vec::new();
    let mut packages = Vec::new();
    for line in tree.lines() {
        let line = line.trim();
        if let Some((pkg, rest)) = line.split_once(" feature \"") {
            let feature = rest.split('"').next().unwrap_or_default();
            if RENDER_FEATURES.contains(&(pkg, feature))
                && !on
                    .iter()
                    .any(|(p, f): &(String, String)| p == pkg && f == feature)
            {
                on.push((pkg.to_string(), feature.to_string()));
            }
        } else if let Some((pkg, version)) = line.split_once(' ')
            && version.starts_with('v')
            && !packages.iter().any(|p| p == pkg)
        {
            packages.push(pkg.to_string());
        }
    }
    (on, packages)
}

/// The drawing features `krate` resolves only in the workspace (or only
/// alone), from the two `cargo tree -e features --prefix none` outputs.
pub fn render_drift(krate: &str, alone: &str, workspace: &str) -> Vec<String> {
    let (alone_on, packages) = render_features(alone);
    let (ws_on, _) = render_features(workspace);
    let mut out = Vec::new();
    for (pkg, feature) in RENDER_FEATURES {
        if !packages.iter().any(|p| p == pkg) {
            continue;
        }
        let has = |on: &[(String, String)]| on.iter().any(|(p, f)| p == pkg && f == feature);
        match (has(&alone_on), has(&ws_on)) {
            (false, true) => out.push(format!(
                "`{krate}` has snapshots but draws without `{pkg}/{feature}` built alone and with it in the workspace: name the feature in its Cargo.toml"
            )),
            (true, false) => out.push(format!(
                "`{krate}` has snapshots and draws with `{pkg}/{feature}` built alone but without it in the workspace"
            )),
            _ => {}
        }
    }
    out
}

/// `cargo tree -e features --prefix none` with `scope` (`-p x` or
/// `--workspace`).
fn feature_tree(root: &Path, scope: &[&str]) -> Result<String, String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["tree", "-e", "features", "--prefix", "none"])
        .args(scope)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo tree: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "cargo tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Whether the crate in `dir` commits snapshot images.
fn has_snapshots(dir: &Path) -> bool {
    std::fs::read_dir(dir.join("tests/snapshots"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "png"))
}

/// The drawing-feature drift of every crate with snapshots.
fn snapshot_drift(root: &Path, krates: &[Krate]) -> Vec<String> {
    let with_snapshots: Vec<&Krate> = krates
        .iter()
        .filter(|k| has_snapshots(&root.join(&k.dir)))
        .collect();
    if with_snapshots.is_empty() {
        return Vec::new();
    }
    let workspace = match feature_tree(root, &["--workspace"]) {
        Ok(t) => t,
        Err(e) => return vec![e],
    };
    let mut out = Vec::new();
    for k in with_snapshots {
        match feature_tree(root, &["-p", &k.name]) {
            Ok(alone) => out.extend(render_drift(&k.name, &alone, &workspace)),
            Err(e) => out.push(e),
        }
    }
    out
}

/// Every problem of the workspace at `root`.
pub fn problems(root: &Path) -> Vec<String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output();
    let json = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => {
            return vec![format!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )];
        }
        Err(e) => return vec![format!("cargo metadata: {e}")],
    };
    let krates = match parse_metadata(&json, root) {
        Ok(k) => k,
        Err(e) => return vec![e],
    };
    let allow = match std::fs::read_to_string(root.join(ALLOW_LIST)) {
        Ok(text) => match parse_allow_list(&text) {
            Ok(a) => a,
            Err(e) => return vec![format!("{ALLOW_LIST}: {e}")],
        },
        Err(e) => return vec![format!("{ALLOW_LIST}: {e}")],
    };
    let mut problems = judge(&violations(&krates), &allow);
    problems.extend(dev_tool_problems(&krates));
    problems.extend(snapshot_drift(root, &krates));
    match std::fs::read_to_string(root.join("Cargo.lock")) {
        Ok(lock) => problems.extend(lock_duplicates(&lock)),
        Err(e) => problems.push(format!("Cargo.lock: {e}")),
    }
    problems
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    if !args.is_empty() {
        return None;
    }
    let problems = problems(root);
    Some(if problems.is_empty() {
        println!(
            "Dependency worlds, the single egui-pop and the snapshot crates' drawing features OK."
        );
        ExitCode::SUCCESS
    } else {
        for p in &problems {
            eprintln!("deps-worlds: {p}");
        }
        eprintln!(
            "xtask: deps-worlds failed (worlds in xtask/src/deps_worlds.rs, debts in {ALLOW_LIST})"
        );
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(name: &str) -> Dep {
        Dep {
            name: name.into(),
            kind: None,
            features: Vec::new(),
        }
    }

    fn dev(name: &str) -> Dep {
        Dep {
            kind: Some("dev".into()),
            ..dep(name)
        }
    }

    fn krate(name: &str, dir: &str, deps: Vec<Dep>) -> Krate {
        Krate {
            name: name.into(),
            dir: dir.into(),
            publishable: false,
            deps,
        }
    }

    fn workspace(extra: Vec<Krate>) -> Vec<Krate> {
        let mut ws = vec![
            krate("helios-core", "crates/helios-core", vec![dep("serde")]),
            krate(
                "helios-ui",
                "crates/helios-ui",
                vec![dep("egui-pop"), dev("egui_kittest")],
            ),
            krate(
                "helios-gui",
                "crates/helios-gui",
                vec![dep("egui-pop"), dep("helios-ui")],
            ),
        ];
        ws.extend(extra);
        ws
    }

    fn pairs(edges: &[Edge]) -> Vec<(&str, &str)> {
        edges
            .iter()
            .map(|e| (e.krate.as_str(), e.dep.as_str()))
            .collect()
    }

    #[test]
    fn deps_worlds_clean_workspace_has_no_violation() {
        assert_eq!(violations(&workspace(vec![])), vec![]);
    }

    #[test]
    fn deps_worlds_module_on_egui_is_named() {
        let ws = workspace(vec![krate(
            "helios-module-x",
            "modules/x",
            vec![dep("helios-core"), dep("egui")],
        )]);
        let v = violations(&ws);
        assert_eq!(pairs(&v), vec![("helios-module-x", "egui")]);
        assert!(v[0].why.contains("visual"), "{}", v[0].why);
    }

    #[test]
    fn deps_worlds_module_on_shell_or_forms_framework_fails() {
        let ws = workspace(vec![krate(
            "helios-module-x",
            "modules/x",
            vec![dep("helios-gui"), dep("helios-ui")],
        )]);
        assert_eq!(
            pairs(&violations(&ws)),
            vec![
                ("helios-module-x", "helios-gui"),
                ("helios-module-x", "helios-ui")
            ]
        );
    }

    #[test]
    fn deps_worlds_hud_render_only_in_the_visual_framework() {
        let render = Dep {
            features: vec!["render".into()],
            ..dep("egui_hud")
        };
        let ws = workspace(vec![
            krate(
                "helios-controls",
                "crates/helios-controls",
                vec![dep("egui_hud")],
            ),
            krate("helios-module-x", "modules/x", vec![render.clone()]),
            krate("helios-rack", "crates/helios-rack", vec![render]),
        ]);
        assert_eq!(
            pairs(&violations(&ws)),
            vec![("helios-module-x", "egui_hud")]
        );
    }

    #[test]
    fn deps_worlds_forms_framework_may_not_use_egui_hud() {
        let ws = vec![krate(
            "helios-ui",
            "crates/helios-ui",
            vec![dep("egui_hud")],
        )];
        assert_eq!(pairs(&violations(&ws)), vec![("helios-ui", "egui_hud")]);
    }

    #[test]
    fn deps_worlds_harness_only_as_dev_dependency() {
        let ws = vec![krate(
            "helios-ui",
            "crates/helios-ui",
            vec![dep("egui_kittest")],
        )];
        assert_eq!(pairs(&violations(&ws)), vec![("helios-ui", "egui_kittest")]);
    }

    #[test]
    fn deps_worlds_new_ui_crate_under_crates_is_engine_until_named() {
        let ws = vec![krate(
            "helios-new",
            "crates/helios-new",
            vec![dep("egui-pop")],
        )];
        let v = violations(&ws);
        assert_eq!(pairs(&v), vec![("helios-new", "egui-pop")]);
        assert!(v[0].why.contains("engine"), "{}", v[0].why);
    }

    #[test]
    fn deps_worlds_dev_tools_are_free() {
        let ws = vec![krate(
            "xtask",
            "xtask",
            vec![dep("egui"), dep("egui_stack")],
        )];
        assert_eq!(violations(&ws), vec![]);
    }

    #[test]
    fn deps_worlds_dev_tool_must_not_be_published() {
        let mut gallery = krate(
            "helios-controls-gallery",
            "crates/helios-controls-gallery",
            vec![],
        );
        assert!(dev_tool_problems(std::slice::from_ref(&gallery)).is_empty());
        gallery.publishable = true;
        let p = dev_tool_problems(&[gallery]);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("`helios-controls-gallery`"), "{p:?}");
        assert!(p[0].contains("publish = false"), "{p:?}");
        // Another world's crate may be published.
        let mut core = krate("helios-core", "crates/helios-core", vec![]);
        core.publishable = true;
        assert!(dev_tool_problems(&[core]).is_empty());
    }

    #[test]
    fn deps_worlds_no_crate_depends_on_a_dev_tool() {
        let extract = || krate("helios-i18n-extract", "crates/helios-i18n-extract", vec![]);
        let ws = workspace(vec![
            extract(),
            krate(
                "helios-module-x",
                "modules/x",
                vec![dep("helios-i18n-extract")],
            ),
            // Tests may use one.
            krate(
                "helios-node",
                "crates/helios-node",
                vec![dev("helios-i18n-extract")],
            ),
            // A dev tool may use another.
            krate("xtask", "xtask", vec![dep("helios-i18n-extract")]),
        ]);
        let p = dev_tool_problems(&ws);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(
            p[0].starts_with("`helios-module-x` depends on `helios-i18n-extract`"),
            "{p:?}"
        );
        assert!(p[0].contains("a visual crate"), "{p:?}");
        // Reported once, by this rule, out of the allow-list's reach.
        assert_eq!(violations(&ws), vec![]);
    }

    #[test]
    fn deps_worlds_parses_publish() {
        let json = r#"{"packages":[
            {"name":"a","manifest_path":"/ws/a/Cargo.toml","publish":[],"dependencies":[]},
            {"name":"b","manifest_path":"/ws/b/Cargo.toml","publish":null,"dependencies":[]},
            {"name":"c","manifest_path":"/ws/c/Cargo.toml","publish":["crates-io"],"dependencies":[]}]}"#;
        let ks = parse_metadata(json, Path::new("/ws")).unwrap();
        let publishable: Vec<bool> = ks.iter().map(|k| k.publishable).collect();
        assert_eq!(publishable, vec![false, true, true]);
    }

    #[test]
    fn deps_worlds_allow_list_covers_and_reports_stale_entries() {
        let ws = workspace(vec![krate(
            "helios-module-x",
            "modules/x",
            vec![dep("egui")],
        )]);
        let allow = parse_allow_list(
            "# debts\n[[allow]]\ncrate = \"helios-module-x\"\ndep = \"egui\"\nreason = \"r\"\n\n\
             [[allow]]\ncrate = \"helios-gui\"\ndep = \"egui\"\nreason = \"gone\"\n",
        )
        .unwrap();
        let problems = judge(&violations(&ws), &allow);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("`helios-gui` -> `egui`"),
            "{problems:?}"
        );
        assert!(problems[0].contains("remove its entry"), "{problems:?}");
    }

    #[test]
    fn deps_worlds_uncovered_violation_names_crate_and_edge() {
        let ws = workspace(vec![krate(
            "helios-module-x",
            "modules/x",
            vec![dep("egui")],
        )]);
        let problems = judge(&violations(&ws), &[]);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with("`helios-module-x` depends on `egui`"),
            "{problems:?}"
        );
    }

    #[test]
    fn deps_worlds_allow_list_needs_a_reason() {
        let err = parse_allow_list("[[allow]]\ncrate = \"a\"\ndep = \"b\"\n").unwrap_err();
        assert!(err.contains("no `reason`"), "{err}");
        let err = parse_allow_list("[[allow]]\ncrate = \"a\"\ndep = \"b\"\nreason = \"r\"\n[[allow]]\ncrate = \"a\"\ndep = \"b\"\nreason = \"r\"\n").unwrap_err();
        assert!(err.contains("twice"), "{err}");
    }

    #[test]
    fn deps_worlds_parses_cargo_metadata() {
        let json = r#"{"packages":[{"name":"helios-module-x",
            "manifest_path":"/ws/modules/x/Cargo.toml",
            "dependencies":[{"name":"egui_hud","kind":null,"features":["render"],
            "uses_default_features":false},{"name":"egui_kittest","kind":"dev",
            "features":[],"uses_default_features":true}]}]}"#;
        let ks = parse_metadata(json, Path::new("/ws")).unwrap();
        assert_eq!(ks.len(), 1);
        assert_eq!(ks[0].dir, "modules/x");
        assert_eq!(ks[0].deps[0].features, vec!["render".to_string()]);
        assert_eq!(ks[0].deps[1].kind.as_deref(), Some("dev"));
        assert_eq!(
            pairs(&violations(&ks)),
            vec![
                ("helios-module-x", "egui_hud"),
                ("helios-module-x", "egui_kittest")
            ]
        );
    }

    #[test]
    fn deps_worlds_lock_with_two_egui_pops_fails() {
        let lock = "[[package]]\nname = \"egui-pop\"\nversion = \"0.10.0\"\n\n\
                    [[package]]\nname = \"egui-pop\"\nversion = \"0.11.0\"\n\n\
                    [[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n\n\
                    [[package]]\nname = \"serde\"\nversion = \"0.9.0\"\n\n\
                    [[package]]\nname = \"egui\"\nversion = \"0.37.0\"\n";
        let d = lock_duplicates(lock);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].contains("`egui-pop` (0.10.0, 0.11.0)"), "{d:?}");
    }

    #[test]
    fn deps_worlds_lock_with_one_of_each_passes() {
        let lock = "[[package]]\nname = \"egui\"\nversion = \"0.37.0\"\n\n\
                    [[package]]\nname = \"egui-pop\"\nversion = \"0.11.0\"\n";
        assert!(lock_duplicates(lock).is_empty());
    }

    const ALONE: &str = "helios-x v0.1.0 (/w/crates/helios-x)\n\
                         egui-pop-core v0.10.0\n\
                         egui-pop-core feature \"bundled-fonts\"\n\
                         egui v0.37.0\n\
                         egui feature \"default_fonts\" (*)\n";

    #[test]
    fn deps_worlds_snapshot_crate_missing_a_workspace_render_feature_fails() {
        let ws =
            format!("{ALONE}egui-pop-core feature \"phosphor\" (*)\nfoo feature \"phosphor\"\n");
        let d = render_drift("helios-x", ALONE, &ws);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].contains("`egui-pop-core/phosphor`"), "{d:?}");
    }

    #[test]
    fn deps_worlds_snapshot_crate_with_the_same_render_features_passes() {
        let ws = format!("{ALONE}epaint feature \"monochrome_emoji_fonts\"\nfoo v1.0.0\n");
        // epaint is not in the crate's own tree: its feature is not its pixels.
        assert!(render_drift("helios-x", ALONE, &ws).is_empty());
    }
}
