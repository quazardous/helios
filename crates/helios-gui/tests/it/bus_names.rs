//! The bus-contract naming rule on the desktop's bus (its README, "Naming
//! points"): several libraries share one bus, so each declares its points
//! and registers its listeners under its own prefix. egui-pop's are
//! `pop.*`, `egui_stack`'s `stack.*`, and Helios's own `helios.*`.

use egui::vec2;
use egui_kittest::Harness;
use helios_gui::shell::LIBRARY_WINDOW_COMMAND;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

/// The prefixes of the libraries Helios builds on.
const LIBRARIES: [&str; 2] = ["pop.", "stack."];

/// Helios's prefix.
const HELIOS: &str = "helios.";

/// The desktop with its own windows open once, so that their listeners
/// are on the bus too.
fn desktop() -> Harness<'static, Shell> {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().draw_frame(true);
    h.run();
    for (command, args) in [
        (LIBRARY_WINDOW_COMMAND, Value::Null),
        ("activity.show", Value::Null),
        ("library.importWindow", json!({})),
    ] {
        h.state_mut().run(command, args, Origin::User).unwrap();
        h.run_steps(3);
    }
    h
}

/// Every point on the desktop's bus is a library's or Helios's, and so
/// is every listener: a point or a listener Helios adds without its
/// `helios.` prefix fails here, the node's fact kinds (`helios.vault.*`,
/// `helios.volume.*`…) included.
#[test]
fn every_point_and_listener_of_helios_has_its_prefix() {
    let h = desktop();
    let bus = h.state().app().handle().bus();
    let library = |id: &str| LIBRARIES.iter().any(|p| id.starts_with(p));
    let mut wrong = Vec::new();
    let (mut points, mut listeners) = (0, 0);
    for point in bus.points() {
        if !(library(point.id) || point.id.starts_with(HELIOS)) {
            wrong.push(format!("point `{}`", point.id));
        }
        points += 1;
        for l in bus.listeners(point.id).unwrap_or_default() {
            if !(library(&l.id) || l.id.starts_with(HELIOS)) {
                wrong.push(format!(
                    "listener `{}` (owner {:?}) on `{}`",
                    l.id, l.owner, point.id
                ));
            }
            listeners += 1;
        }
    }
    assert!(
        points > 20 && listeners > 30,
        "{points} points, {listeners} listeners"
    );
    // Every fact kind the desktop declares, a point or not.
    for decl in helios_gui::FACTS.iter().copied().flatten() {
        if !decl.kind.starts_with(HELIOS) {
            wrong.push(format!("fact kind `{}`", decl.kind));
        }
    }
    assert!(
        wrong.is_empty(),
        "without their prefix:\n{}",
        wrong.join("\n")
    );
}
