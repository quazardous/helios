//! Where Ctrl+Z goes in Helios (#5258, egui-pop's focus routing,
//! `docs/design/undo.md`): one test per case. The focused view's history;
//! with no view focused, the history of the view focused last in the
//! window; with none focused yet, nowhere, and a message says why; never
//! the newest step of another history. Through the real keys and the
//! real focus (`<panel>.focus`, what a click in a panel does).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::api::history;
use helios_gui::Shell;
use helios_values::HistoryScope;

use crate::view_undo::{
    EV, add_node, ctrl_z, focus, follow, in_network, set, steps, temp, value, window,
};

/// egui-pop's message when Ctrl+Z finds no view focused yet
/// (`egui-pop-history-nothing-focused`).
const NOTHING_FOCUSED: &str = "Nothing to undo here: click a view first";

fn edit_scope() -> history::ScopeId {
    helios_gui::history::scope_id(HistoryScope::Image)
}

fn workspace_scope() -> history::ScopeId {
    helios_gui::history::scope_id(HistoryScope::Workspace)
}

/// The focus leaves the view for a place naming no history (the focused
/// widget lets go of it, as after a click on an empty spot).
fn lose_focus(h: &mut Harness<'static, Shell>) {
    let focused = h
        .ctx
        .memory(egui::Memory::focused)
        .expect("a widget focused");
    h.ctx.memory_mut(|m| m.surrender_focus(focused));
    follow(h);
    assert_eq!(
        history::focused_scope(h.state().app().handle()),
        None,
        "no view focused any more"
    );
}

#[test]
fn a_focused_view_s_ctrl_z_undoes_its_own_history() {
    let root = temp("routing-focused");
    let (mut h, _persist) = window(&root);
    let ev0 = value(&h, EV);
    set(&h, EV, 1.0, None);
    follow(&mut h);
    focus(&mut h, "helios.viewport");
    assert_eq!(
        history::focused_scope(h.state().app().handle()),
        Some(edit_scope())
    );
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), ev0, "the image's edit undone");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn with_no_view_focused_ctrl_z_goes_to_the_view_focused_last() {
    let root = temp("routing-last");
    let (mut h, _persist) = window(&root);
    let ev0 = value(&h, EV);
    focus(&mut h, "helios.viewport");
    set(&h, EV, 1.0, None);
    follow(&mut h);
    // The workspace's step is the newer one.
    add_node(&mut h);
    lose_focus(&mut h);
    let app = h.state().app().handle();
    assert_eq!(history::last_focused(app), Some(edit_scope()));
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), ev0, "the last focused view's edit undone");
    assert!(in_network(&h, "sigmoid"), "the newer graph edit left alone");
    assert!(h.query_by_label(NOTHING_FOCUSED).is_none());
    // The focus on a panel naming no history (the History panel, as the
    // journey's view-undo step does): still the image's.
    focus(&mut h, "helios.viewport");
    set(&h, EV, 2.0, None);
    follow(&mut h);
    focus(&mut h, "egui_pop.history");
    assert_eq!(history::focused_scope(h.state().app().handle()), None);
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), ev0, "the image's edit undone again");
    assert!(in_network(&h, "sigmoid"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn with_no_view_focused_yet_ctrl_z_undoes_nothing_and_says_why() {
    let root = temp("routing-none");
    let (mut h, _persist) = window(&root);
    {
        let app = h.state().app().handle();
        assert_eq!(history::focused_scope(app), None, "no view focused yet");
        assert_eq!(history::last_focused(app), None);
    }
    set(&h, EV, 1.0, None);
    follow(&mut h);
    add_node(&mut h);
    let edits = steps(&h, HistoryScope::Image);
    let changes = steps(&h, HistoryScope::Workspace);
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), Some(1.0), "the edit stays");
    assert!(in_network(&h, "sigmoid"), "the workspace change stays");
    assert_eq!(steps(&h, HistoryScope::Image), edits);
    assert_eq!(steps(&h, HistoryScope::Workspace), changes);
    assert!(
        h.query_by_label(NOTHING_FOCUSED).is_some(),
        "the message says why"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ctrl_z_never_takes_the_newest_step_of_another_history() {
    let root = temp("routing-other");
    let (mut h, _persist) = window(&root);
    add_node(&mut h);
    // The image's step is the newer one.
    set(&h, EV, 1.0, None);
    follow(&mut h);
    let edits = steps(&h, HistoryScope::Image);
    assert!(edits > 0 && steps(&h, HistoryScope::Workspace) > 0);
    focus(&mut h, "helios.graph");
    assert_eq!(
        history::focused_scope(h.state().app().handle()),
        Some(workspace_scope())
    );
    ctrl_z(&mut h);
    assert!(
        !in_network(&h, "sigmoid"),
        "the focused history's step undone"
    );
    assert_eq!(
        value(&h, EV),
        Some(1.0),
        "the other history's newer step stays"
    );
    assert_eq!(steps(&h, HistoryScope::Image), edits);
    let _ = std::fs::remove_dir_all(&root);
}
