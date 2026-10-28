//! Helios's places and modes on egui-pop's context stack (its ADR 0012),
//! declared here, without egui, so the desktop and the MCP server read
//! the same frames (see `docs/gui.md`, "Places and modes").
//!
//! - **Places**: the library ([`LIBRARY`]), the darkroom on an image
//!   ([`DARKROOM`], its payload `{"image": id}` or `{"file": path}`),
//!   entered from the library or opened alone, the workspace's own
//!   network ([`WORKSPACE`], its payload the workspace's key), and a
//!   sub-workspace of it ([`SUBWORKSPACE`], its payload `{"id": id}`),
//!   entered under its parent's place, its crumb its name. The root is
//!   the workspace the window shows.
//! - **Modes**: culling (an orientation, set), mask editing (drawing a
//!   path, pushed and never restored) and before-and-after compare
//!   (pushed).
//!
//! Each is mapped to an arrangement, or, for the short pushed modes, to
//! the panels it shows, so toggling them never moves the user's panels.
//! The modes show as chips in egui-pop's status bar, each in its tone,
//! and a place whose payload says nothing readable names its crumb
//! itself ([`crumb`]).
//! The stack of a workspace is egui-pop's `context` document beside its
//! layout, `<config>/ui/workspaces/<key>/context.json`; [`tools`] reads
//! it and applies transitions to it for agents.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_pop_model::context::{Entry, FrameRegistry, FrameSpec, State, Verdict};
use egui_pop_model::ids::{FrameId, GroupId, PanelId};

/// The library: the vault's images to browse, rate and cull.
pub const LIBRARY: FrameId = FrameId::from_static("helios.library");
/// The darkroom: one image edited (`{"image": id}` or `{"file": path}`).
pub const DARKROOM: FrameId = FrameId::from_static("helios.darkroom");
/// The workspace's network, edited (its payload: the workspace's key).
pub const WORKSPACE: FrameId = FrameId::from_static("helios.workspace");
/// A sub-workspace, edited (`{"id": id}`): its nodes only.
pub const SUBWORKSPACE: FrameId = FrameId::from_static("helios.sub-workspace");

/// Culling: flag, rate and label from anywhere, the image first.
pub const CULLING: FrameId = FrameId::from_static("helios.culling");
/// Drawing a mask's path in the image.
pub const MASK_EDITING: FrameId = FrameId::from_static("helios.mask-editing");
/// The image before and after its edit, side by side.
pub const COMPARE: FrameId = FrameId::from_static("helios.compare");

/// How the user works: culling (more orientations to come).
pub const ORIENTATION: GroupId = GroupId::from_static("orientation");
/// A tool of the image (drawing a mask's path).
pub const TOOL: GroupId = GroupId::from_static("tool");
/// Compare.
pub const COMPARING: GroupId = GroupId::from_static("compare");

/// The arrangement of the library (egui-pop's template of that name).
pub const LIBRARY_ARRANGEMENT: &str = "Library";
/// The arrangement of the workspace's network.
pub const NETWORK_ARRANGEMENT: &str = "Network";
/// The image-first arrangement: culling.
pub const IMAGE_ARRANGEMENT: &str = "Image";
/// The darkroom's arrangement: the workspace's default layout, the image
/// in the centre.
pub const DARKROOM_ARRANGEMENT: &str = "Darkroom";
/// A sub-workspace's arrangement: the network's, its own once the user
/// changes it.
pub const SUBWORKSPACE_ARRANGEMENT: &str = "Sub-workspace";

/// The Masks panel, shown while a mask is edited.
pub const MASKS_PANEL: &str = "helios.masks";
/// The image, shown while comparing.
pub const IMAGE_PANEL: &str = "helios.viewport";

/// Helios's frames, as declared (the desktop adds its keys).
pub fn frames() -> Vec<FrameSpec> {
    vec![
        FrameSpec::place(LIBRARY, "Library").arrangement(LIBRARY_ARRANGEMENT),
        // The darkroom starts from the workspace's default layout, the
        // image in the centre, wherever the image was opened from (the
        // library's grid included), and remembers what the user picks
        // there.
        FrameSpec::place(DARKROOM, "Darkroom")
            .arrangement(DARKROOM_ARRANGEMENT)
            .label(|c| crumb(c.frame, c.title, c.payload)),
        FrameSpec::place(WORKSPACE, "Network")
            .arrangement(NETWORK_ARRANGEMENT)
            .label(|c| crumb(c.frame, c.title, c.payload)),
        // Its crumb is its name ([`guard_subworkspace`]), else its id.
        FrameSpec::place(SUBWORKSPACE, "Sub-workspace")
            .arrangement(SUBWORKSPACE_ARRANGEMENT)
            .label(|c| crumb(c.frame, c.title, c.payload)),
        // The modes the user turns on show as chips in the status bar.
        FrameSpec::mode(CULLING, ORIENTATION, "Culling")
            .arrangement(IMAGE_ARRANGEMENT)
            .visible()
            .tone("accent"),
        FrameSpec::mode(MASK_EDITING, TOOL, "Mask editing")
            .transient()
            .show(PanelId::from_static(MASKS_PANEL))
            .visible()
            .tone("warning"),
        FrameSpec::mode(COMPARE, COMPARING, "Compare")
            .show(PanelId::from_static(IMAGE_PANEL))
            .visible()
            .tone("info"),
    ]
}

/// A place's crumb in the breadcrumb, from its payload (`None`: egui-pop's
/// default, the title): the darkroom on a file says the file's name (an
/// image of the vault, only its id: the title alone), the network says
/// only its title (its payload is the workspace's key, not a name), a
/// sub-workspace its id (its name once the workspace is known:
/// [`guard_subworkspace`]).
pub fn crumb(frame: &FrameId, title: &str, payload: &serde_json::Value) -> Option<String> {
    if *frame == WORKSPACE {
        return Some(title.to_string());
    }
    if *frame == SUBWORKSPACE {
        return Some(subworkspace_of(payload).unwrap_or(title).to_string());
    }
    if *frame == DARKROOM {
        return Some(match Opened::of(payload) {
            Some(Opened::File(p)) => match p.file_name() {
                Some(name) => format!("{title} · {}", name.to_string_lossy()),
                None => title.to_string(),
            },
            _ => title.to_string(),
        });
    }
    None
}

/// The workspace's network guarded: entered only on the workspace
/// `shown` names (the one the window shows, the one an agent works on).
pub fn guard_network(
    spec: FrameSpec,
    shown: impl Fn() -> Option<String> + Send + Sync + 'static,
) -> FrameSpec {
    if spec.id != WORKSPACE {
        return spec;
    }
    spec.can_enter(move |c| match shown() {
        Some(key) if c.payload.as_str() == Some(key.as_str()) => Verdict::Go,
        Some(key) => Verdict::Refuse(format!("the network to edit is this workspace's (`{key}`)")),
        None => Verdict::Refuse("no workspace is open".into()),
    })
}

/// What a sub-workspace's lookup answers for an id: `None` while no
/// workspace is known, `Some(None)` when it has no sub-workspace of that
/// id (never had one, or deleted), else `Some(Some(name))`.
pub type Lookup = Arc<dyn Fn(&str) -> Option<Option<String>> + Send + Sync>;

/// A sub-workspace guarded and named from the workspace `lookup` reads:
/// entered only when it exists (an id deleted is refused, never reused),
/// its crumb its name.
pub fn guard_subworkspace(spec: FrameSpec, lookup: Lookup) -> FrameSpec {
    if spec.id != SUBWORKSPACE {
        return spec;
    }
    let names = lookup.clone();
    spec.can_enter(move |c| match subworkspace_of(c.payload) {
        None => Verdict::Refuse("a sub-workspace is entered by its id: {\"id\": \"…\"}".into()),
        Some(id) => match lookup(id) {
            None => Verdict::Refuse("no workspace is open".into()),
            Some(None) => Verdict::Refuse(format!("the workspace has no sub-workspace `{id}`")),
            Some(Some(_)) => Verdict::Go,
        },
    })
    .label(move |c| {
        let id = subworkspace_of(c.payload)?;
        Some(names(id).flatten().unwrap_or_else(|| id.to_string()))
    })
}

/// The frames of the workspace `key`: entering its network checks the
/// payload is that workspace; entering a sub-workspace, that it has it
/// (`subworkspaces`: its sub-workspaces, as its template keeps them).
pub fn registry(key: &str, subworkspaces: &[crate::format::SubWorkspace]) -> FrameRegistry {
    let names: Vec<(String, String)> = subworkspaces
        .iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    let lookup: Lookup = Arc::new(move |id| {
        Some(
            names
                .iter()
                .find(|(i, _)| i == id)
                .map(|(_, name)| name.clone()),
        )
    });
    let mut r = FrameRegistry::new();
    for f in frames() {
        let key = key.to_string();
        let f = guard_network(f, move || Some(key.clone()));
        r.insert(guard_subworkspace(f, lookup.clone()))
            .expect("Helios's frames are declared once");
    }
    r
}

/// A sub-workspace's payload.
pub fn subworkspace_payload(id: &str) -> serde_json::Value {
    serde_json::json!({ "id": id })
}

/// The sub-workspace a payload names.
pub fn subworkspace_of(payload: &serde_json::Value) -> Option<&str> {
    payload["id"].as_str().filter(|id| !id.is_empty())
}

/// The place path that shows a sub-workspace (`path`: it and its
/// parents, top down, as `Template::subworkspace_path` gives them) from
/// the state shown: the places above the first sub-workspace are kept
/// (the network, the darkroom, the root), the sub-workspaces are its
/// parents then it.
pub fn subworkspace_places(state: &State, path: &[&str]) -> Vec<Entry> {
    let mut places: Vec<Entry> = state
        .places
        .iter()
        .take_while(|e| e.frame != SUBWORKSPACE)
        .cloned()
        .collect();
    places.extend(path.iter().map(|id| Entry {
        frame: SUBWORKSPACE,
        payload: subworkspace_payload(id),
    }));
    places
}

/// The sub-workspace shown, if the top place is one.
pub fn subworkspace_shown(state: &State) -> Option<&str> {
    state
        .places
        .last()
        .filter(|e| e.frame == SUBWORKSPACE)
        .and_then(|e| subworkspace_of(&e.payload))
}

/// The darkroom's payload for an image of the vault.
pub fn image_payload(id: &str) -> serde_json::Value {
    serde_json::json!({ "image": id })
}

/// The darkroom's payload for a file opened on its own.
pub fn file_payload(path: &Path) -> serde_json::Value {
    serde_json::json!({ "file": path })
}

/// What a darkroom payload opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// An image of the vault, by id.
    Image(String),
    /// A file on its own.
    File(PathBuf),
}

impl Opened {
    pub fn of(payload: &serde_json::Value) -> Option<Opened> {
        if let Some(id) = payload["image"].as_str() {
            return Some(Opened::Image(id.to_string()));
        }
        payload["file"]
            .as_str()
            .map(|p| Opened::File(PathBuf::from(p)))
    }

    pub fn payload(&self) -> serde_json::Value {
        match self {
            Opened::Image(id) => image_payload(id),
            Opened::File(p) => file_payload(p),
        }
    }
}

/// A workspace's stack, egui-pop's `context` document:
/// `<config>/ui/workspaces/<key>/context.json`.
pub fn document(config: &Path, workspace: &Path) -> PathBuf {
    config
        .join("ui")
        .join("workspaces")
        .join(crate::workspace::key(workspace))
        .join("context.json")
}

/// The tools agents reach the stack with through MCP, on a workspace's
/// `context` document: what a desktop opening the workspace comes back
/// to. Each transition goes through egui-pop's pure machine against
/// Helios's frames, with `if_rev` and `dry_run`.
pub mod tools {
    use std::path::{Path, PathBuf};

    use egui_pop_model::context::{self, ContextDoc, Envelope, Request};
    use serde_json::{Value as Json, json};

    fn workspace(args: &Json) -> Result<PathBuf, String> {
        let dir = args["workspace"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .ok_or("`workspace` is missing")?;
        if dir.is_absolute() {
            Ok(dir)
        } else {
            Err("`workspace` is an absolute path".into())
        }
    }

    fn load(path: &Path) -> Result<ContextDoc, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => context::schema()
                .load::<ContextDoc>(&text)
                .map(|l| l.value)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ContextDoc::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    fn save(path: &Path, doc: &ContextDoc) -> Result<(), String> {
        let text = context::schema().save(doc).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Written whole, then moved over the old one.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// The workspace's sub-workspaces, as its files hold it (none when
    /// they cannot be read: entering one is then refused).
    fn subworkspaces(config: &Path, dir: &Path) -> Vec<crate::format::SubWorkspace> {
        crate::workspace::Files::new(config, dir)
            .load_on_disk()
            .map(|s| s.bundle.template.subworkspaces)
            .unwrap_or_default()
    }

    /// The sub-workspaces as agents enter them: each `id` (the
    /// `helios.sub-workspace` payload's), `name` and `parent`.
    pub fn listed(subworkspaces: &[crate::format::SubWorkspace]) -> Json {
        subworkspaces
            .iter()
            .map(|s| json!({ "id": s.id, "name": s.name, "parent": s.parent }))
            .collect()
    }

    /// `{workspace}`: the stack (places, modes, revision), the frames it
    /// may hold, and the sub-workspaces a `helios.sub-workspace` place
    /// may name.
    pub fn read(config: &Path, args: &Json) -> Result<Json, String> {
        let dir = workspace(args)?;
        let path = super::document(config, &dir);
        let doc = load(&path)?;
        let frames: Vec<_> = super::frames();
        Ok(json!({
            "state": doc.state,
            "rev": doc.state.rev,
            "place": doc.state.place(),
            "frames": frames,
            "subworkspaces": listed(&subworkspaces(config, &dir)),
            "document": path,
        }))
    }

    /// `{workspace, request, if_rev?, dry_run}`: a transition (`request`
    /// as egui-pop's `context.request` takes it: `op` enter, leave, jump,
    /// go, `set_mode`, `push_mode`, `pop_mode`, `clear_mode` or back), checked
    /// against `if_rev`, written unless `dry_run`. A refusal is an error
    /// saying why.
    pub fn request(config: &Path, args: &Json) -> Result<Json, String> {
        apply(config, "context.request", args)
    }

    /// `{workspace, places, dry_run}`: goes to a place path (`places`:
    /// each `frame` with its `payload`).
    pub fn go(config: &Path, args: &Json) -> Result<Json, String> {
        apply(config, "context.go", args)
    }

    /// `{workspace, mode?, push?, group?, dry_run}`: sets a mode, pushes
    /// it (`push`), or turns a group off (`group`, no `mode`).
    pub fn set_mode(config: &Path, args: &Json) -> Result<Json, String> {
        apply(config, "context.set_mode", args)
    }

    /// The transition a tool asks for (`cmd`: `context.request`,
    /// `context.go` or `context.set_mode`), with its `if_rev` and
    /// `dry_run`: the same on a workspace's files and on the desktop
    /// holding it.
    pub fn envelope_of(cmd: &str, args: &Json) -> Result<Envelope, String> {
        let request = match cmd {
            "context.request" => serde_json::from_value(args["request"].clone())
                .map_err(|e| format!("`request`: {e}"))?,
            "context.go" => Request::Go {
                places: serde_json::from_value(args["places"].clone())
                    .map_err(|e| format!("`places`: {e}"))?,
            },
            "context.set_mode" => match args["mode"].as_str().filter(|m| !m.is_empty()) {
                Some(m) => {
                    let mode = egui_pop_model::ids::FrameId::new(m).map_err(|e| e.to_string())?;
                    if args["push"].as_bool() == Some(true) {
                        Request::push(mode)
                    } else {
                        Request::set(mode)
                    }
                }
                None => Request::ClearMode {
                    group: serde_json::from_value(args["group"].clone())
                        .map_err(|e| format!("`group` (or `mode`): {e}"))?,
                },
            },
            other => return Err(format!("no context tool `{other}`")),
        };
        Ok(Envelope {
            request,
            if_rev: args["if_rev"].as_u64(),
            dry_run: args["dry_run"].as_bool().unwrap_or(false),
        })
    }

    /// What a transition answers: the state it leads to (for a dry run,
    /// the one it would), the frames entered and left.
    pub fn answer(t: &context::Transition, dry_run: bool) -> Json {
        json!({
            "dry_run": dry_run,
            "rev": t.state.rev,
            "state": t.state,
            "entered": t.entered,
            "left": t.left,
            "redirected": t.redirected,
        })
    }

    fn apply(config: &Path, cmd: &str, args: &Json) -> Result<Json, String> {
        let dir = workspace(args)?;
        let env = envelope_of(cmd, args)?;
        let path = super::document(config, &dir);
        let mut doc = load(&path)?;
        let registry = super::registry(&crate::workspace::key(&dir), &subworkspaces(config, &dir));
        let t = context::apply(&doc.state, &registry, &[], &env)
            .map_err(|r| format!("refused ({:?}, by {}): {}", r.kind, r.by, r.reason))?;
        if !env.dry_run {
            doc.state = t.state.clone();
            save(&path, &doc)?;
        }
        Ok(answer(&t, env.dry_run))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_frames_are_declared_once_with_their_groups() {
        let r = registry("k", &[]);
        assert_eq!(r.len(), 7);
        let groups = r.groups();
        assert_eq!(groups.len(), 3);
        assert!(r.get(&MASK_EDITING).unwrap().transient);
        // The modes show as chips, each in its tone; places do not.
        for (mode, tone) in [
            (CULLING, "accent"),
            (MASK_EDITING, "warning"),
            (COMPARE, "info"),
        ] {
            let spec = r.get(&mode).unwrap();
            assert!(spec.visible, "{mode}");
            assert_eq!(spec.tone.as_deref(), Some(tone), "{mode}");
        }
        assert!(!r.get(&LIBRARY).unwrap().visible);
    }

    #[test]
    fn crumbs_say_a_file_s_name_and_never_a_workspace_key() {
        let r = registry("k", &[]);
        let darkroom = r.get(&DARKROOM).unwrap();
        let file = file_payload(Path::new("/photos/shoot/img_0042.cr3"));
        assert_eq!(
            darkroom.label_of("Darkroom", &file).as_deref(),
            Some("Darkroom · img_0042.cr3")
        );
        assert_eq!(
            darkroom
                .label_of("Chambre noire", &image_payload("ab12"))
                .as_deref(),
            Some("Chambre noire")
        );
        let network = r.get(&WORKSPACE).unwrap();
        assert_eq!(
            network.label_of("Network", &json!("0f3a9c")).as_deref(),
            Some("Network")
        );
        assert_eq!(
            r.get(&LIBRARY).unwrap().label_of("Library", &json!(null)),
            None
        );
    }

    #[test]
    fn an_agent_moves_a_workspace_stack_with_dry_runs() {
        let root = std::env::temp_dir().join(format!("helios-contexts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("config");
        let ws = root.join("shoot");
        let key = crate::workspace::key(&ws);
        let args = |extra: serde_json::Value| {
            let mut a = json!({ "workspace": ws });
            for (k, v) in extra.as_object().unwrap() {
                a[k] = v.clone();
            }
            a
        };
        let read = tools::read(&config, &args(json!({}))).unwrap();
        assert_eq!(read["place"], "root");
        assert_eq!(read["frames"].as_array().unwrap().len(), 7);
        // A blank workspace has no sub-workspace to enter.
        assert_eq!(read["subworkspaces"], json!([]));
        let sub = tools::request(
            &config,
            &args(
                json!({ "request": { "op": "enter", "frame": "helios.sub-workspace", "payload": { "id": "skin" } } }),
            ),
        );
        assert!(sub.unwrap_err().contains("`skin`"));
        // A dry run answers, writes nothing.
        let places = json!([{ "frame": "helios.library" }, { "frame": "helios.darkroom", "payload": { "image": "ab12" } }]);
        let dry = tools::go(&config, &args(json!({ "places": places, "dry_run": true }))).unwrap();
        assert_eq!(dry["dry_run"], true);
        assert_eq!(dry["state"]["places"][1]["payload"]["image"], "ab12");
        assert!(!document(&config, &ws).exists());
        let done = tools::go(&config, &args(json!({ "places": places }))).unwrap();
        assert_eq!(done["rev"], 1);
        // A mode, then a stale revision refused.
        let set = tools::set_mode(
            &config,
            &args(json!({ "mode": "helios.culling", "if_rev": 1 })),
        )
        .unwrap();
        assert_eq!(set["rev"], 2);
        let stale = tools::set_mode(
            &config,
            &args(json!({ "group": "orientation", "if_rev": 1 })),
        );
        assert!(stale.unwrap_err().contains("Stale"));
        // Another workspace's network is refused by its guard.
        let other = tools::request(
            &config,
            &args(
                json!({ "request": { "op": "enter", "frame": "helios.workspace", "payload": "elsewhere" } }),
            ),
        );
        assert!(other.unwrap_err().contains(&key));
        let read = tools::read(&config, &args(json!({}))).unwrap();
        assert_eq!(read["place"], "helios.darkroom");
        assert_eq!(read["state"]["modes"][0]["frame"], "helios.culling");
        // Written as the desktop reads it.
        let text = std::fs::read_to_string(document(&config, &ws)).unwrap();
        assert!(text.contains("\"kind\": \"context\"") || text.contains("\"kind\":\"context\""));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_sub_workspace_is_entered_by_its_id_and_named_by_its_name() {
        use egui_pop_model::context::{self as machine, Envelope, Request};
        let sub = |id: &str, name: &str, parent: Option<&str>| crate::format::SubWorkspace {
            id: id.into(),
            name: name.into(),
            nodes: vec!["retouch".into()],
            parent: parent.map(String::from),
        };
        let subs = [
            sub("skin", "Skin & eyes", None),
            sub("eyes", "Eyes", Some("skin")),
        ];
        let r = registry("k", &subs);
        let spec = r.get(&SUBWORKSPACE).unwrap();
        assert_eq!(spec.arrangement.as_deref(), Some(SUBWORKSPACE_ARRANGEMENT));
        assert_eq!(
            spec.label_of("Sub-workspace", &subworkspace_payload("skin"))
                .as_deref(),
            Some("Skin & eyes")
        );
        // Without the workspace's names, its id.
        assert_eq!(
            registry("k", &[])
                .get(&SUBWORKSPACE)
                .unwrap()
                .label_of("Sub-workspace", &subworkspace_payload("skin"))
                .as_deref(),
            Some("skin")
        );
        let enter = |id: &str| {
            Envelope::new(Request::Enter {
                frame: SUBWORKSPACE,
                payload: subworkspace_payload(id),
            })
        };
        let t = machine::apply(&State::default(), &r, &[], &enter("skin")).unwrap();
        assert_eq!(subworkspace_shown(&t.state), Some("skin"));
        // A deleted (or unknown) id, or no id, is refused with a reason.
        let gone = machine::apply(&State::default(), &r, &[], &enter("hair")).unwrap_err();
        assert!(gone.reason.contains("`hair`"), "{}", gone.reason);
        let none = Envelope::new(Request::enter(SUBWORKSPACE));
        assert!(machine::apply(&State::default(), &r, &[], &none).is_err());
        // The path to a nested one keeps the places above the first
        // sub-workspace and replaces the sub-workspaces.
        let mut state = t.state.clone();
        state.places.insert(
            0,
            Entry {
                frame: WORKSPACE,
                payload: serde_json::json!("k"),
            },
        );
        let places = subworkspace_places(&state, &["skin", "eyes"]);
        let frames: Vec<&str> = places.iter().map(|e| e.frame.as_str()).collect();
        assert_eq!(
            frames,
            [
                "helios.workspace",
                "helios.sub-workspace",
                "helios.sub-workspace"
            ]
        );
        assert_eq!(subworkspace_of(&places[2].payload), Some("eyes"));
    }

    #[test]
    fn a_payload_says_what_the_darkroom_opens() {
        let p = image_payload("ab12");
        assert_eq!(Opened::of(&p), Some(Opened::Image("ab12".into())));
        let f = Opened::File("/tmp/x.png".into());
        assert_eq!(Opened::of(&f.payload()), Some(f));
        assert_eq!(Opened::of(&serde_json::Value::Null), None);
    }
}
