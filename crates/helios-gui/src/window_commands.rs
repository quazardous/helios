//! The desktop's window commands, served by its node's command endpoint
//! (`helios_node_core::endpoint::WindowCommands`): the CLI and agents list
//! them beside the node's (`commands.list`, `scope: "window"`), each greyed
//! with the reason its menu entry gives, and run them (`command.run`) in a
//! window, as its menus would, under the origin they name.
//!
//! The endpoint's threads ask; the interface answers on its own thread, at
//! its next frame ([`Shell::serve_window_commands`]), which a request
//! wakes. A [`Bridge`] is made before the window exists (the node is
//! advertised first) and attached once the shell is built: until then it
//! says the window is not open yet.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use egui_pop::api::app::AppHandle;
use egui_pop::api::windows::CONTEXT_APP_WINDOW;
use egui_pop::model::commands::CommandSpec;
use egui_pop::model::ids::CommandId;
use egui_pop::model::when::{When, WhenContext};
use helios_node_core::endpoint::{WindowCommand, WindowCommands};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

use crate::shell::Shell;

/// How long a request waits for the interface's answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// A request for the interface's thread.
enum Ask {
    List {
        window: String,
        reply: Sender<Result<Vec<WindowCommand>, String>>,
    },
    Run {
        id: String,
        params: Value,
        origin: String,
        window: String,
        reply: Sender<Result<Value, String>>,
    },
}

/// The requests the shell answers at each frame.
pub struct Asks(Receiver<Ask>);

/// Where requests go once the shell answers, and what to wake.
type Link = Option<(Sender<Ask>, egui::Context)>;

/// The window commands as the node's endpoint reaches them.
#[derive(Clone, Default)]
pub struct Bridge(Arc<Mutex<Link>>);

impl Bridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// The shell answers from now on; a request wakes `ctx`.
    pub fn attach(&self, shell: &mut Shell, ctx: &egui::Context) {
        let (tx, rx) = channel();
        shell.serve_window_commands(Asks(rx));
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some((tx, ctx.clone()));
    }

    /// Sends a request and waits for its answer.
    fn ask<T>(&self, make: impl FnOnce(Sender<Result<T, String>>) -> Ask) -> Result<T, String> {
        let (reply, answer) = channel();
        {
            let link = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            let Some((tx, ctx)) = link.as_ref() else {
                return Err("the desktop's window is not open yet".into());
            };
            tx.send(make(reply))
                .map_err(|_| "the desktop's window is closed".to_string())?;
            ctx.request_repaint();
        }
        answer.recv_timeout(ANSWER_WITHIN).map_err(|_| {
            format!(
                "the desktop's window did not answer within {} s",
                ANSWER_WITHIN.as_secs()
            )
        })?
    }
}

impl WindowCommands for Bridge {
    fn list(&self, window: &str) -> Result<Vec<WindowCommand>, String> {
        let window = window.to_string();
        self.ask(|reply| Ask::List { window, reply })
    }

    fn run(&self, id: &str, params: Value, origin: &str, window: &str) -> Result<Value, String> {
        let (id, origin, window) = (id.to_string(), origin.to_string(), window.to_string());
        self.ask(|reply| Ask::Run {
            id,
            params,
            origin,
            window,
            reply,
        })
    }
}

/// Answers the requests waiting, on the interface's thread.
pub(crate) fn answer(shell: &mut Shell, asks: &Asks, ctx: &egui::Context) {
    while let Ok(ask) = asks.0.try_recv() {
        match ask {
            Ask::List { window, reply } => {
                let _ = reply.send(list(shell.app().handle(), ctx, &window));
            }
            Ask::Run {
                id,
                params,
                origin,
                window,
                reply,
            } => {
                let _ = reply.send(run(shell, ctx, &id, params, &origin, &window));
            }
        }
    }
}

/// Window `window`'s context: the application's, with its id and
/// whether a modal blocks it, as its menu bar is laid out in.
fn window_context(app: &AppHandle, ctx: &egui::Context, window: &str) -> WhenContext {
    let modal = app
        .windows()
        .into_iter()
        .find(|w| w.id.as_str() == window)
        .is_some_and(|w| egui_pop::core::overlay::modal_open_in(ctx, app.window_group(w.viewport)));
    crate::native_menu::window_context(&app.when_context(), window, modal)
}

/// Why a command is greyed in context `cx`, as its menu entry says: the
/// other open window it runs in ("Only in the Helios window"), else the
/// clause that does not hold; `None` when it applies.
pub fn disabled_reason(
    app: &AppHandle,
    ctx: &egui::Context,
    spec: &CommandSpec,
    cx: &WhenContext,
) -> Option<String> {
    use egui_pop::api::strings as txt;
    use egui_pop::core::strings::tr_args;
    if spec.when.is_empty() {
        return None;
    }
    let Ok(when) = When::parse(&spec.when) else {
        return Some(tr_args(
            ctx,
            &txt::MENU_DOES_NOT_APPLY,
            &[("when", &spec.when)],
        ));
    };
    if when.eval(cx) {
        return None;
    }
    let here = cx.get(CONTEXT_APP_WINDOW);
    let elsewhere = app
        .windows()
        .into_iter()
        .filter(|w| w.id.as_str() != here)
        .find(|w| {
            let mut there = cx.clone();
            there.set(CONTEXT_APP_WINDOW, w.id.as_str());
            when.eval(&there)
        });
    Some(match elsewhere {
        Some(w) => tr_args(ctx, &txt::MENU_IN_WINDOW, &[("window", &w.title)]),
        None => tr_args(ctx, &txt::MENU_DOES_NOT_APPLY, &[("when", &spec.when)]),
    })
}

/// The schema of a command's params: its declared arguments when they are
/// an object schema, else any object (its title names its arguments).
fn params_schema(args: &Value) -> Value {
    if args["type"] == "object" {
        args.clone()
    } else {
        json!({ "type": "object" })
    }
}

/// The commands offered to agents, in window `window`'s context.
fn list(app: &AppHandle, ctx: &egui::Context, window: &str) -> Result<Vec<WindowCommand>, String> {
    check_window(window)?;
    let cx = window_context(app, ctx, window);
    let mut commands: Vec<WindowCommand> = app
        .commands()
        .into_iter()
        .filter(|(spec, _)| spec.agent)
        .map(|(spec, _)| {
            let reason = disabled_reason(app, ctx, &spec, &cx);
            WindowCommand {
                id: spec.id.to_string(),
                label: app.command_menu_title(&spec),
                category: spec.category.clone(),
                description: spec.description.clone(),
                params: params_schema(&spec.args),
                keys: app.keys_for(&spec.id),
                icon: spec.icon.clone(),
                read_only: spec.read_only,
                enabled: reason.is_none(),
                disabled_reason: reason,
            }
        })
        .collect();
    commands.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(commands)
}

/// A window id is a word: `main`, `import`, `settings`.
fn check_window(window: &str) -> Result<(), String> {
    if window.is_empty() || window.contains(char::is_whitespace) {
        return Err(format!("`{window}` is not a window id"));
    }
    Ok(())
}

/// Helios's origin for an endpoint's (`user`, `cli`, `control`,
/// `agent:NAME`, `script:NAME`).
fn origin_of(origin: &str) -> Origin {
    if let Some(name) = origin.strip_prefix("agent:") {
        Origin::Agent(name.into())
    } else if let Some(name) = origin.strip_prefix("script:") {
        Origin::Script(name.into())
    } else if origin == "user" {
        Origin::User
    } else {
        Origin::Remote(origin.into())
    }
}

/// Runs command `id` in open window `window`, refused with its reason
/// where it does not apply there.
fn run(
    shell: &mut Shell,
    ctx: &egui::Context,
    id: &str,
    params: Value,
    origin: &str,
    window: &str,
) -> Result<Value, String> {
    check_window(window)?;
    let app = shell.app().handle().clone();
    if !app.windows().iter().any(|w| w.id.as_str() == window) {
        return Err(format!("no `{window}` window is open"));
    }
    let command = CommandId::new(id).map_err(|e| e.to_string())?;
    let spec = app
        .command(&command)
        .filter(|s| s.agent)
        .ok_or_else(|| format!("no command `{id}` in the window"))?;
    let cx = window_context(&app, ctx, window);
    if let Some(reason) = disabled_reason(&app, ctx, &spec, &cx) {
        return Err(reason);
    }
    shell.run_in(id, params, origin_of(origin), &cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_map_to_the_shells() {
        assert_eq!(origin_of("agent:mcp"), Origin::Agent("mcp".into()));
        assert_eq!(origin_of("script:batch"), Origin::Script("batch".into()));
        assert_eq!(origin_of("user"), Origin::User);
        assert_eq!(origin_of("cli"), Origin::Remote("cli".into()));
        assert!(check_window("import").is_ok());
        assert!(check_window("").is_err());
        assert_eq!(params_schema(&Value::Null), json!({ "type": "object" }));
    }
}
