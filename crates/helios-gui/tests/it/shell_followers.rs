//! The shell's per-pass followers: `Shell::ui` draws, it does not
//! remember. What follows a change listens to the event that says it (a
//! named listener in its point's order: `workspace.applied`,
//! `library.changed`, `image.opened`, `settings.changed`, ...). The
//! followers still called each pass are listed here, each with why it
//! stays: a per-frame input or a thread's mailbox, or the event it waits
//! for. A new one fails this test: make it a listener, or list it here
//! with its reason.

/// The per-pass followers `Shell::ui` may call, as the scan names them,
/// with why each stays.
const ALLOWED: &[(&str, &str)] = &[
    // Stays: a frame's input, a thread's mailbox, pixels.
    (
        "follow_scale",
        "stays: egui's per-frame input (`scale::changed`), already change-gated",
    ),
    (
        "follow_workspace",
        "stays: a mailbox of other threads (the file watcher, agents' endpoint requests)",
    ),
    (
        "follow_opening",
        "stays: pixels (the large thumbnail's texture arriving for the stand-in)",
    ),
    (
        "focus.preview.follow",
        "stays: pixels (the shown graph's render asked once it changes)",
    ),
    (
        "follow_photo_panels",
        "stays: the preview's render status (`panel.state`), read per frame",
    ),
    // Waits for an event that does not exist yet.
    (
        "contexts::follow_place",
        "waits: egui-pop's `context.changed` as a local bus point",
    ),
    (
        "focus.follow.frame",
        "waits: egui-pop's `context.changed` as a local bus point",
    ),
    (
        "follow_comparison",
        "waits: egui-pop's `context.changed` as a local bus point",
    ),
    (
        "follow_surfaces",
        "waits: egui-pop's focused-panel and layout changes as local bus points (the open surfaces and the double-click target)",
    ),
    (
        "follow_workspace_keys",
        "waits: an egui-pop keymap-layer-changed event",
    ),
    (
        "follow_theme",
        "waits: a `theme.installed` event (or a listener on the `theme.load` step)",
    ),
    (
        "follow_language",
        "waits: egui-pop's i18n plugin saying the locale changed on the bus (it applies the language in its own per-pass block)",
    ),
];

/// Followers that do not carry `follow` in their name.
const UNNAMED: &[&str] = &["sync_workspace"];

/// The body of `Shell::ui` in the shell's source.
fn ui_body() -> String {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shell.rs"))
        .expect("the shell's source");
    let start = source
        .find("    pub fn ui(&mut self, ui: &mut Ui) {")
        .expect("`Shell::ui` in shell.rs");
    let body = &source[start..];
    let end = body.find("\n    }\n").expect("the end of `Shell::ui`");
    body[..end].to_string()
}

/// The calls in `code` (comments left out), each as its path without
/// `self.` or `crate::`, in order.
fn calls(code: &str) -> Vec<String> {
    let code: String = code
        .lines()
        .map(|l| l.split("//").next().unwrap_or_default())
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let mut found = Vec::new();
    let mut path = String::new();
    for c in code.chars() {
        if c.is_alphanumeric() || matches!(c, '_' | ':' | '.') {
            path.push(c);
            continue;
        }
        if c == '(' && !path.is_empty() {
            let p = path.trim_start_matches('.');
            let p = p.strip_prefix("self.").unwrap_or(p);
            let p = p.strip_prefix("crate::").unwrap_or(p);
            found.push(p.to_string());
        }
        path.clear();
    }
    found
}

/// The per-pass followers `Shell::ui` calls.
fn followers() -> Vec<String> {
    calls(&ui_body())
        .into_iter()
        .filter(|c| c.contains("follow") || UNNAMED.contains(&c.as_str()))
        .collect()
}

#[test]
fn the_scan_finds_calls_named_or_listed() {
    let code = "self.follow_a(&x); // self.follow_b()\n\
                crate::contexts::follow_place(\n a,\n);\n\
                self.focus\n  .follow\n  .frame(h, &mut v);\n\
                self.sync_workspace();\n self.draw(ui);";
    assert_eq!(
        calls(code),
        [
            "follow_a",
            "contexts::follow_place",
            "focus.follow.frame",
            "sync_workspace",
            "draw"
        ]
    );
}

/// Every per-pass follower left in `Shell::ui` is listed with its
/// reason; a new one fails here, and one moved to an event leaves the
/// list (19 per-pass followers before the first moves; 16 once the
/// snapshots' toasts were said with the asks answered after the panels;
/// 13 once the workspace's state, the workshop session and the edit's
/// marks followed the values on `values.changed`; 11 now: the Decisions
/// panel's cards and the Queue panel follow them there too, the Queue
/// panel the jobs' events as well).
#[test]
fn every_per_pass_follower_in_the_shell_is_listed_with_its_reason() {
    let found = followers();
    let mut unique = found.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        found.len(),
        "a follower called twice: {found:?}"
    );
    let new: Vec<&String> = found
        .iter()
        .filter(|f| !ALLOWED.iter().any(|(a, _)| a == f))
        .collect();
    assert!(
        new.is_empty(),
        "per-pass followers not listed in tests/it/shell_followers.rs: {new:?}. \
         Make each a named listener of the event that changes its input, \
         or list it with its reason"
    );
    let gone: Vec<&str> = ALLOWED
        .iter()
        .map(|(a, _)| *a)
        .filter(|a| !found.iter().any(|f| f == a))
        .collect();
    assert!(
        gone.is_empty(),
        "listed but no longer called in `Shell::ui`: {gone:?}; take them off the list"
    );
    assert!(ALLOWED.iter().all(|(_, why)| !why.is_empty()));
    assert_eq!(found.len(), 11, "{found:?}");
}

/// The followers moved to events are not called each pass.
#[test]
fn the_followers_moved_to_events_are_not_called_each_pass() {
    let found = followers();
    for moved in [
        "follow_subworkspaces",
        "follow_import_sessions",
        "follow_snapshots",
        "sync_workspace",
        "follow_session",
        "follow_edit_marks",
        "follow_decisions",
        "follow_queue",
    ] {
        assert!(
            !found.iter().any(|f| f == moved),
            "{moved} is called in `ui`"
        );
    }
}
