//! `cargo xtask hud-bench [COMPOSITE...]`: the counter of the `egui_hud`
//! agent bench (`docs/design/egui-hud-agent-bench.md`).
//!
//! `egui_hud`'s design rule: a feature must reduce what an agent composing a
//! workshop has to know. The bench counts, in the files that make a
//! workshop, the presentation decisions written by hand and the rules that
//! apply to one control only, by pattern, so the counts are never taken by
//! eye. A workshop is a composite file (`*.helios-composite.json`); its
//! files are the composite itself and the declarations of the modules its
//! nodes name. The host's files (the rack, the catalogue) are shared by
//! every workshop and counted once, apart.
//!
//! The patterns are the doc's counting rules, one function each; their
//! tests are the rules' examples.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const USAGE: &str = "hud-bench [COMPOSITE...]";

/// The composites scanned when none is given.
pub const COMPOSITES: &str = "templates/composites";

/// The host files every workshop shares, relative to the workspace root.
pub const HOST: &[&str] = &[
    "crates/helios-controls/src/catalogue.rs",
    "crates/helios-rack/src/lib.rs",
    "crates/helios-rack/src/plugin.rs",
    "crates/helios-rack/src/tools.rs",
];

/// The face ids of `egui_hud`'s face library (`egui_hud::face::library`).
pub const FACES: &[&str] = &[
    "slider",
    "fader",
    "knob",
    "endless-knob",
    "scrub",
    "meter",
    "readout",
    "readout-lines",
    "checkbox",
    "switch",
    "push-button",
    "lamp",
    "menu",
    "preview-menu",
    "segmented",
    "radio",
    "folded-list",
    "curve",
    "point-list",
    "colour-wheel",
    "text-field",
];

/// The keys of an exposed parameter (or a part) that decide its
/// presentation. The others (`name`, `node`, `param`, `label`, `unit`,
/// `min`, `max`, `soft`) are content: what the workshop shows, not how.
pub const PRESENTATION_KEYS: &[&str] = &[
    "face",
    "faces",
    "slot",
    "size",
    "kit",
    "integration",
    "orientation",
    "representation",
    "surface",
    "widget",
];

/// The counts of one file, or of a sum of files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// A face named or reordered by hand.
    pub face: usize,
    /// A slot size, size policy or orientation set by hand.
    pub slot: usize,
    /// A kit or an integration level overridden.
    pub kit: usize,
    /// A host drawing that draws (a `Drawings::draw` that is not `None`).
    pub host_drawing: usize,
    /// A rule that applies to one control only (a test on its id).
    pub one_control: usize,
}

impl Counts {
    /// The presentation decisions: face, slot, kit and host drawing.
    pub fn decisions(&self) -> usize {
        self.face + self.slot + self.kit + self.host_drawing
    }

    fn add(&mut self, o: Counts) {
        self.face += o.face;
        self.slot += o.slot;
        self.kit += o.kit;
        self.host_drawing += o.host_drawing;
        self.one_control += o.one_control;
    }
}

/// The code a file is counted on: comments and its `#[cfg(test)]` part
/// left out (tests are not what an agent composes).
pub fn code_lines(text: &str) -> Vec<&str> {
    let end = text.find("#[cfg(test)]").unwrap_or(text.len());
    text[..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect()
}

/// Whether `line` holds the string literal `"word"`.
fn has_literal(line: &str, word: &str) -> bool {
    line.contains(&format!("\"{word}\""))
}

/// The counts of a Rust file.
///
/// - face: a face id literal outside a control's own `.face("…")` list (a
///   control naming the faces it accepts is the control's declaration,
///   not a presentation decision), or a hand reorder of `faces`
///   (`faces.insert`, `faces.retain`), or a `ParamWidget::` hint;
/// - slot: `Slot::`, `.slot(`, `SizePolicy`, `Orientation::`;
/// - kit: `Kit::`, `.kit(`, `Integration::`;
/// - host drawing: each `fn draw(` of a `Drawings` impl whose body is not
///   `None`;
/// - one control: a test on an id (`.id == "…"`, `id == CONST`,
///   `.get("…")` on a registry is not one) or a match arm on a control
///   id constant of the catalogue (`catalogue::X =>`).
pub fn count_rust(text: &str) -> Counts {
    let lines = code_lines(text);
    let mut c = Counts::default();
    for l in &lines {
        let t = l.trim();
        if !t.starts_with(".face(") && !t.contains(" .face(") {
            c.face += FACES.iter().filter(|f| has_literal(t, f)).count();
        }
        if t.contains("faces.insert(") || t.contains("faces.retain(") {
            c.face += 1;
        }
        c.face += t.matches("ParamWidget::").count();
        c.slot += ["Slot::", ".slot(", "SizePolicy", "Orientation::"]
            .iter()
            .map(|p| t.matches(p).count())
            .sum::<usize>();
        c.kit += ["Kit::", ".kit(", "Integration::"]
            .iter()
            .map(|p| t.matches(p).count())
            .sum::<usize>();
        if is_id_test(t) {
            c.one_control += 1;
        }
    }
    c.host_drawing = drawing_bodies(&lines);
    c
}

/// A test on one control's id.
fn is_id_test(t: &str) -> bool {
    let id_cmp = [
        "id == \"",
        "id != \"",
        "id == catalogue::",
        "id != catalogue::",
    ];
    id_cmp.iter().any(|p| t.contains(p)) || (t.starts_with("catalogue::") && t.contains("=>"))
}

/// The `fn draw(` bodies of `impl Drawings for` blocks that draw something
/// (anything but a lone `None`).
fn drawing_bodies(lines: &[&str]) -> usize {
    let mut n = 0;
    let mut in_impl = false;
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("impl") && t.contains("Drawings for") {
            in_impl = true;
        } else if t.starts_with("impl") {
            in_impl = false;
        }
        if in_impl && t.starts_with("fn draw(") {
            // The body: from the line ending the signature (`{`) to the
            // first line closing it at the fn's indentation.
            let indent = lines[i].len() - lines[i].trim_start().len();
            let mut body = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && !lines[j - 1].trim_end().ends_with('{') {
                j += 1;
            }
            while j < lines.len() {
                let lj = lines[j];
                let ind = lj.len() - lj.trim_start().len();
                if ind == indent && lj.trim() == "}" {
                    break;
                }
                body.push(lj.trim());
                j += 1;
            }
            if body != ["None"] {
                n += 1;
            }
            i = j;
        }
        i += 1;
    }
    n
}

/// The counts of a JSON declaration (a composite, a kit, a rack file):
/// every presentation key anywhere in it, by kind, and every object
/// naming one `control` with a presentation key beside it (a rule for one
/// control).
pub fn count_json(v: &serde_json::Value) -> Counts {
    let mut c = Counts::default();
    walk(v, &mut c);
    c
}

fn walk(v: &serde_json::Value, c: &mut Counts) {
    match v {
        serde_json::Value::Object(m) => {
            let mut presentation = false;
            for (k, x) in m {
                let kind = match k.as_str() {
                    "face" | "faces" | "widget" => Some(&mut c.face),
                    "slot" | "size" | "orientation" | "representation" | "surface" => {
                        Some(&mut c.slot)
                    }
                    "kit" | "integration" => Some(&mut c.kit),
                    _ => None,
                };
                if let Some(n) = kind {
                    *n += 1;
                    presentation = true;
                }
                walk(x, c);
            }
            if presentation && m.contains_key("control") {
                c.one_control += 1;
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, c)),
        _ => {}
    }
}

/// An exposed parameter shown with no intervention: none of
/// [`PRESENTATION_KEYS`] on it.
pub fn unattended(exposed: &serde_json::Value) -> bool {
    exposed
        .as_object()
        .is_some_and(|m| !PRESENTATION_KEYS.iter().any(|k| m.contains_key(*k)))
}

/// One workshop's line of the report.
#[derive(Debug, Default)]
pub struct Workshop {
    pub id: String,
    /// Whether the composite has a `workshop` block (a workshop, not only
    /// a macro).
    pub has_workshop_block: bool,
    pub counts: Counts,
    /// Exposed parameters, and those shown with no intervention.
    pub exposed: usize,
    pub unattended: usize,
    /// The files holding at least one presentation decision.
    pub decision_files: Vec<String>,
    /// The modules whose declaration was not found.
    pub missing: Vec<String>,
}

impl Workshop {
    /// Files touched to change the style of every workshop at once: the
    /// context (one place) plus each file that pins a presentation by
    /// hand, since a pinned face or slot ignores the context.
    pub fn style_files(&self) -> usize {
        1 + self.decision_files.len()
    }
}

/// The source files of a module named by a composite node, relative to
/// the root: `modules/<name>/src/*.rs` and `*.json` when that crate
/// exists. A crate holding several modules is named by their common
/// prefix (`mask_blur` in `modules/masks`, `vkdt_usm` in `modules/vkdt`);
/// there the module's files are those whose code holds its name as a
/// string literal (its declaration), not its siblings', and a prefix
/// never picks a crate that does not declare the module.
pub fn module_files(root: &Path, module: &str) -> Vec<PathBuf> {
    let modules = root.join("modules");
    let own = src_files(&modules.join(module));
    if !own.is_empty() {
        return own;
    }
    let Some((prefix, _)) = module.split_once('_') else {
        return Vec::new();
    };
    let literal = format!("\"{module}\"");
    let declares = |f: &PathBuf| {
        std::fs::read_to_string(f)
            .is_ok_and(|t| code_lines(&t).iter().any(|l| l.contains(&literal)))
    };
    [prefix.to_string(), format!("{prefix}s")]
        .iter()
        .map(|dir| {
            src_files(&modules.join(dir))
                .into_iter()
                .filter(declares)
                .collect::<Vec<_>>()
        })
        .find(|files| !files.is_empty())
        .unwrap_or_default()
}

/// A crate's `src/*.rs` and `src/*.json`, sorted; empty when it has none.
fn src_files(crate_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(crate_dir.join("src"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs" || x == "json"))
        .collect();
    files.sort();
    files
}

fn count_file(path: &Path) -> Result<Counts, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.extension().is_some_and(|x| x == "json") {
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(count_json(&v))
    } else {
        Ok(count_rust(&text))
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

/// Counts one composite and the modules it names.
pub fn workshop(root: &Path, composite: &Path) -> Result<Workshop, String> {
    let text =
        std::fs::read_to_string(composite).map_err(|e| format!("{}: {e}", composite.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", composite.display()))?;
    let mut w = Workshop {
        id: v["id"].as_str().unwrap_or("?").to_string(),
        has_workshop_block: v.get("workshop").is_some(),
        ..Workshop::default()
    };
    let own = count_json(&v);
    if own.decisions() > 0 {
        w.decision_files.push(rel(root, composite));
    }
    w.counts.add(own);
    let exposed = v["exposed"].as_array().cloned().unwrap_or_default();
    w.exposed = exposed.len();
    w.unattended = exposed.iter().filter(|e| unattended(e)).count();
    let mut modules: Vec<String> = v["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n["module"].as_str().map(str::to_string))
        .collect();
    modules.sort();
    modules.dedup();
    let mut seen: Vec<PathBuf> = Vec::new();
    for m in modules {
        let files = module_files(root, &m);
        if files.is_empty() {
            w.missing.push(m);
            continue;
        }
        for f in files {
            if seen.contains(&f) {
                continue;
            }
            let c = count_file(&f)?;
            if c.decisions() > 0 {
                w.decision_files.push(rel(root, &f));
            }
            w.counts.add(c);
            seen.push(f);
        }
    }
    Ok(w)
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    if args.iter().any(|a| a.starts_with('-')) {
        return None;
    }
    let composites: Vec<PathBuf> = if args.is_empty() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(root.join(COMPOSITES))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".helios-composite.json"))
            .collect();
        v.sort();
        v
    } else {
        args.iter().map(|a| root.join(a)).collect()
    };
    println!(
        "| composite | workshop | face | slot | kit | host drawing | decisions | one-control rules | exposed unattended | style files |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for c in &composites {
        match workshop(root, c) {
            Ok(w) => {
                let k = w.counts;
                println!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {}/{} | {} |",
                    w.id,
                    if w.has_workshop_block { "yes" } else { "no" },
                    k.face,
                    k.slot,
                    k.kit,
                    k.host_drawing,
                    k.decisions(),
                    k.one_control,
                    w.unattended,
                    w.exposed,
                    w.style_files(),
                );
                for f in &w.decision_files {
                    println!("  decisions in {f}");
                }
                for m in &w.missing {
                    println!("  module {m}: declaration not found");
                }
            }
            Err(e) => {
                eprintln!("hud-bench: {e}");
                return Some(ExitCode::from(1));
            }
        }
    }
    println!();
    println!("Host files, shared by every workshop:");
    println!("| file | face | slot | kit | host drawing | one-control rules |");
    println!("|---|---|---|---|---|---|");
    for h in HOST {
        match count_file(&root.join(h)) {
            Ok(k) => println!(
                "| {h} | {} | {} | {} | {} | {} |",
                k.face, k.slot, k.kit, k.host_drawing, k.one_control
            ),
            Err(e) => {
                eprintln!("hud-bench: {e}");
                return Some(ExitCode::from(1));
            }
        }
    }
    Some(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_control_naming_its_faces_is_not_a_decision() {
        let c = count_rust(
            "Control::new(\"g\", \"Gain\", spec)\n    .face(\"fader\")\n    .face(\"knob\")\n",
        );
        assert_eq!(c.face, 0);
    }

    #[test]
    fn a_face_picked_by_hand_is_one_decision_and_a_reorder_another() {
        let c = count_rust("let f = Some(\"preview-menu\");\nc.faces.insert(0, v);\n");
        assert_eq!(c.face, 2);
    }

    #[test]
    fn comments_and_tests_are_not_counted() {
        let c = count_rust("// a \"knob\" here\n#[cfg(test)]\nlet f = \"slider\";\n");
        assert_eq!(c, Counts::default());
    }

    #[test]
    fn slot_and_kit_patterns_count() {
        let c = count_rust(
            "let s = Slot::new(2, 1);\nlet k = Kit::compact();\nx.integration = Integration::Managed;\n",
        );
        assert_eq!((c.slot, c.kit), (1, 2));
    }

    #[test]
    fn a_drawing_returning_none_draws_nothing() {
        let src = "impl Drawings for Host<'_> {\n    fn draw(\n        &mut self,\n    ) -> Option<Vec<Edit>> {\n        None\n    }\n}\n";
        assert_eq!(count_rust(src).host_drawing, 0);
        let src = "impl Drawings for Host<'_> {\n    fn draw(&mut self) -> Option<Vec<Edit>> {\n        paint(ui);\n        Some(vec![])\n    }\n}\n";
        assert_eq!(count_rust(src).host_drawing, 1);
    }

    #[test]
    fn a_test_on_one_id_is_a_one_control_rule() {
        let c =
            count_rust("if c.id == \"exposure\" {\nmatch id {\n    catalogue::EXPOSURE => 1,\n}\n");
        assert_eq!(c.one_control, 2);
    }

    #[test]
    fn json_presentation_keys_count_by_kind() {
        let v = json!({
            "exposed": [
                { "name": "a", "node": "n", "param": "p", "label": "A", "min": 0 },
                { "name": "b", "node": "n", "param": "q", "face": "knob", "slot": "1x1" }
            ],
            "kit": { "integration": "managed" },
            "rules": [ { "control": "exposure", "face": "scrub" } ]
        });
        let c = count_json(&v);
        assert_eq!((c.face, c.slot, c.kit, c.one_control), (2, 1, 2, 1));
        let exposed = v["exposed"].as_array().unwrap();
        assert!(unattended(&exposed[0]));
        assert!(!unattended(&exposed[1]));
    }

    /// A scratch workspace holding `modules/<dir>/src/lib.rs` files.
    fn scratch(name: &str, crates: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hud-bench-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, lib) in crates {
            let src = root.join("modules").join(dir).join("src");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("lib.rs"), lib).unwrap();
        }
        root
    }

    #[test]
    fn a_module_is_found_in_the_crate_that_declares_it() {
        let root = scratch(
            "resolve",
            &[
                ("exposure", "name: \"exposure\".into(),\n"),
                ("masks", "name: \"mask_blend\".into(),\n"),
                ("vkdt", "module(\n    \"vkdt_usm\",\n)\n"),
            ],
        );
        // A sibling module's file in the same crate is not the module's.
        std::fs::write(
            root.join("modules/vkdt/src/llap.rs"),
            "module(\"vkdt_llap\");\nlet face = \"curve\";\n",
        )
        .unwrap();
        let files = |m: &str| -> Vec<String> {
            module_files(&root, m)
                .iter()
                .map(|p| rel(&root, p))
                .collect()
        };
        assert_eq!(files("exposure"), ["modules/exposure/src/lib.rs"]);
        assert_eq!(files("mask_blend"), ["modules/masks/src/lib.rs"]);
        assert_eq!(files("vkdt_usm"), ["modules/vkdt/src/lib.rs"]);
        assert_eq!(files("vkdt_llap"), ["modules/vkdt/src/llap.rs"]);
        // A prefix alone is not enough: the crate must declare the module.
        assert!(files("vkdt_nothing").is_empty());
        assert!(files("unknown").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn style_files_are_the_context_plus_each_pinning_file() {
        let w = Workshop {
            decision_files: vec!["a.json".into(), "b.rs".into()],
            ..Workshop::default()
        };
        assert_eq!(w.style_files(), 3);
        assert_eq!(Workshop::default().style_files(), 1);
    }
}
