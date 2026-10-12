//! What panels, widgets and plugins ask the shell for: confirmations,
//! forms, pickers, menus and notices. They never draw a window, a popup or
//! a menu of their own: they ask the shell (`Shell::confirm`,
//! `Shell::form`, `Shell::pick`, `Shell::notify`, or [`request`] from
//! inside a panel), and a test fails if one does (`tests/normalized.rs`).
//!
//! The shell hands them to egui-pop's components (its dialogs, toasts and
//! menus, `egui_pop::api`), which draw them from the theme's tokens. Every
//! open dialog is also data: `dialog.current` describes it and
//! `dialog.answer` answers it, so an agent sees and answers what a human
//! would, its origin recorded like any write.
//!
//! The rules each kind follows are in `docs/gui.md`, "Windows, dialogs and
//! menus".

use egui::Id;
use egui_pop::model::values as pop;
use helios_core::Value;
use helios_values::{Descriptor, PathKind, Type};
use serde::Serialize;
use serde_json::Value as Json;

/// How much a notice matters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// A notice: shown in the status bar and, unless it is plain information,
/// as a toast. It never takes the focus.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Notice {
    pub level: Level,
    pub text: String,
    /// A command offered with it (`(label, command id)`).
    pub action: Option<(String, String)>,
    /// When it was shown, in the GUI's seconds.
    #[serde(skip)]
    pub at: f64,
}

/// A question before an action: the action is named by its verb, never
/// "OK".
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Confirm {
    pub title: String,
    /// The consequences, in one line.
    pub detail: String,
    /// The primary button: "Delete 3 images".
    pub verb: String,
    /// Destructive: the danger style, and the focus starts on Cancel.
    pub danger: bool,
    /// What the primary action runs.
    pub command: String,
    pub args: Json,
}

/// One field of a form, typed by a value descriptor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Field {
    pub key: String,
    pub descriptor: Descriptor,
    pub value: Value,
}

/// A form: fields from descriptors, the standard footer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Form {
    pub title: String,
    pub fields: Vec<Field>,
    pub verb: String,
    /// Run with `{ key: value, … }`.
    pub command: String,
}

/// One choice of a picker.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Choice {
    /// What the command receives.
    pub value: String,
    pub label: String,
    pub detail: Option<String>,
}

/// A list to choose from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Picker {
    pub title: String,
    pub choices: Vec<Choice>,
    /// Run with `{ arg: value }`.
    pub command: String,
    pub arg: String,
}

/// Information to read, closed by its one button ("About Helios").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Info {
    pub title: String,
    pub lines: Vec<String>,
}

/// A menu entry: a command, never anything else.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MenuItem {
    Command { id: String, args: Json },
    Separator,
    Submenu { label: String, items: Vec<MenuItem> },
}

impl MenuItem {
    pub fn command(id: &str) -> Self {
        MenuItem::Command {
            id: id.into(),
            args: Json::Null,
        }
    }
}

/// What a panel asks the shell for, from inside its content.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Confirm(Confirm),
    Form(Form),
    Picker(Picker),
    Menu {
        at: egui::Pos2,
        items: Vec<MenuItem>,
    },
    Notify(Level, String),
}

fn requests_id() -> Id {
    Id::new("helios.requests")
}

/// Asks the shell for a dialog, a menu or a notice from inside a panel;
/// the shell shows it at the next frame.
pub fn request(ctx: &egui::Context, r: Request) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Request>>(requests_id())
            .push(r);
    });
}

/// Opens a context menu of commands on a widget's secondary click.
pub fn context_menu(response: &egui::Response, items: Vec<MenuItem>) {
    if response.secondary_clicked() {
        let at = response
            .interact_pointer_pos()
            .unwrap_or(response.rect.left_bottom());
        request(&response.ctx, Request::Menu { at, items });
    }
}

pub(crate) fn take_requests(ctx: &egui::Context) -> Vec<Request> {
    ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<Request>>(requests_id())))
}

/// How an agent answers (`dialog.answer`).
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Primary,
    Cancel,
    /// A form's values (by field key).
    Values(Json),
    /// A picker's choice, by its value.
    Choice(String),
}

/// A value of Helios's as egui-pop's.
pub(crate) fn value(v: &Value) -> pop::Value {
    match v {
        Value::Float(x) => pop::Value::Float(*x),
        Value::Int(x) => pop::Value::Int(*x),
        Value::Bool(x) => pop::Value::Bool(*x),
        Value::Choice(x) => pop::Value::Choice(x.clone()),
        Value::List(x) => pop::Value::Floats(x.clone()),
        Value::Ints(x) => pop::Value::Ints(x.clone()),
        Value::Text(x) => pop::Value::Text(x.clone()),
    }
}

/// A descriptor of Helios's as egui-pop's: the same fields (egui-pop's
/// value store was extracted from Helios's).
pub(crate) fn descriptor(d: &Descriptor) -> pop::Descriptor {
    let mut out = pop::Descriptor::text(&d.label, "");
    out.ty = match d.ty {
        Type::Float => pop::Type::Float,
        Type::Int => pop::Type::Int,
        Type::Bool => pop::Type::Bool,
        Type::Choice => pop::Type::Choice,
        Type::Floats => pop::Type::Floats,
        Type::Ints => pop::Type::Ints,
        Type::Text => pop::Type::Text,
    };
    out.description.clone_from(&d.description);
    out.default = value(&d.default);
    out.hard = d.hard;
    out.soft = d.soft;
    out.step = d.step;
    out.coarse = d.coarse;
    out.unit.clone_from(&d.unit);
    out.display_scale = d.display_scale;
    out.precision = d.precision;
    out.choices.clone_from(&d.choices);
    out.count = d.count;
    out.capacity = d.capacity;
    out.path = d.path.as_ref().map(|p| pop::PathHint {
        kind: match p.kind {
            PathKind::Dir => pop::PathKind::Dir,
            PathKind::File => pop::PathKind::File,
        },
        extensions: p.extensions.clone(),
    });
    out
}
