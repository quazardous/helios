//! `helios.param`: one parameter's value, copied with its type.
//!
//! The item holds the path, the module, the type and the value as the
//! store keeps it (JSON), with the value as shown (`+0.30 EV`) for the
//! text fallback. It pastes onto a parameter of the same kind (a float
//! onto a float; an integer and a float onto each other, rounded), the
//! store clamping it to the target's range; text pastes too, read with
//! the target's type: a value as shown (`+0.30 EV`, `30 %`, `on`), a
//! choice, a list as JSON, or the path of another parameter (its value).
//! So what `param.copyValue` and `param.copyPath` copy pastes back, from
//! the owner's item or from the system clipboard's text.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::AppHandle;
use egui_pop::api::clipboard::{ClipItem, ClipSource, ClipTarget, FormatId, Paste, TEXT};
use helios_core::Value;
use helios_values::{Change, Clamped, Descriptor, GestureId, Set, Store, Type};
use serde_json::{Value as Json, json};

use super::PARAM;

/// The type's version.
pub const VERSION: u32 = 1;

/// The egui memory key of the parameter whose widget has the focus.
const FOCUS: &str = "helios.clipboard.param";

/// What a copy's text is: the value as shown, or the parameter's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    Value,
    Path,
}

/// A type's name as the payload writes it.
pub fn type_name(ty: Type) -> &'static str {
    match ty {
        Type::Float => "float",
        Type::Int => "int",
        Type::Bool => "bool",
        Type::Choice => "choice",
        Type::Floats => "floats",
        Type::Ints => "ints",
        Type::Text => "text",
    }
}

fn type_of(name: &str) -> Option<Type> {
    Some(match name {
        "float" => Type::Float,
        "int" => Type::Int,
        "bool" => Type::Bool,
        "choice" => Type::Choice,
        "floats" => Type::Floats,
        "ints" => Type::Ints,
        "text" => Type::Text,
        _ => return None,
    })
}

/// A value as shown, in English: what the system clipboard receives.
pub fn shown(d: &Descriptor, v: &Value) -> String {
    match v {
        // A list in JSON, which pastes back.
        Value::List(_) | Value::Ints(_) => helios_values::value_to_json(v).to_string(),
        _ => crate::params::shown(d, v),
    }
}

/// The item's payload for a parameter.
pub fn payload(path: &str, d: &Descriptor, v: &Value, text: Text) -> Json {
    json!({
        "path": path,
        "module": d.module,
        "label": d.label,
        "type": type_name(d.ty),
        "value": helios_values::value_to_json(v),
        "text": match text {
            Text::Value => shown(d, v),
            Text::Path => path.to_string(),
        },
    })
}

/// A payload's text fallback.
pub fn text_of(payload: &Json) -> String {
    payload["text"].as_str().unwrap_or_default().to_string()
}

/// The item a copy of a parameter makes.
pub fn item(path: &str, d: &Descriptor, v: &Value, text: Text) -> ClipItem {
    let p = payload(path, d, v, text);
    let t = text_of(&p);
    ClipItem::new(PARAM, VERSION, p, &d.label).fallback(TEXT, Json::String(t))
}

/// A copied value adapted to a target's type, and what it lost.
pub fn adapt(target: &Descriptor, payload: &Json) -> Result<(Value, Option<String>), String> {
    let from = payload["type"]
        .as_str()
        .and_then(type_of)
        .ok_or("the copied parameter has no type")?;
    let value = &payload["value"];
    let label = payload["label"].as_str().unwrap_or("the value");
    let refused = || {
        format!(
            "{label} is {} and {} takes {}",
            a(type_name(from)),
            target.label,
            a(type_name(target.ty))
        )
    };
    match (from, target.ty) {
        (Type::Float, Type::Int) => {
            let x = value.as_f64().ok_or_else(refused)?;
            let lost = (x.fract() != 0.0).then(|| "the fraction".to_string());
            Ok((Value::Int(x.round() as i64), lost))
        }
        (Type::Int, Type::Float) => Ok((Value::Float(value.as_f64().ok_or_else(refused)?), None)),
        (Type::Choice, Type::Choice) => {
            let c = value.as_str().ok_or_else(refused)?;
            choice(target, c).map(|c| (Value::Choice(c), None))
        }
        (f, t) if f == t => Ok((helios_values::value_from_json(t, value)?, None)),
        _ => Err(refused()),
    }
}

fn a(ty: &str) -> String {
    match ty {
        "int" => "an integer".into(),
        "float" => "a number".into(),
        "bool" => "a switch".into(),
        "choice" => "a choice".into(),
        "floats" => "a list of numbers".into(),
        "ints" => "a list of integers".into(),
        _ => "a text".into(),
    }
}

/// One of a choice's options, matched without case.
fn choice(d: &Descriptor, text: &str) -> Result<String, String> {
    let text = text.trim();
    d.choices
        .iter()
        .find(|c| c.eq_ignore_ascii_case(text))
        .cloned()
        .ok_or_else(|| {
            format!(
                "`{text}` is not one of {}'s choices ({})",
                d.label,
                d.choices.join(", ")
            )
        })
}

/// Text read with a target's type: the value as shown (`+0.30 EV`,
/// `30 %`), `on` or `off`, a choice, a list in JSON, or any text.
pub fn from_text(d: &Descriptor, text: &str) -> Result<Value, String> {
    let t = text.trim();
    let not = || format!("`{t}` is not a value of {}", d.label);
    Ok(match d.ty {
        Type::Float => {
            let x = crate::params::parse_typed(t).ok_or_else(not)?;
            let scale = if d.display_scale == 0.0 {
                1.0
            } else {
                d.display_scale
            };
            Value::Float(x / scale)
        }
        Type::Int => {
            let x = crate::params::parse_typed(t).ok_or_else(not)?;
            if x.fract() != 0.0 {
                return Err(format!("`{t}` is not a whole number"));
            }
            Value::Int(x as i64)
        }
        Type::Bool => match t.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Value::Bool(true),
            "off" | "false" | "no" | "0" => Value::Bool(false),
            _ => return Err(not()),
        },
        Type::Choice => Value::Choice(choice(d, t)?),
        Type::Text => Value::Text(text.to_string()),
        Type::Floats | Type::Ints => {
            let json: Json = serde_json::from_str(t).map_err(|_| not())?;
            helios_values::value_from_json(d.ty, &json)?
        }
    })
}

/// What a paste writes onto `path`: from a copied parameter, or from
/// text (another parameter's path gives that parameter's value).
pub fn value_for(
    store: &Store,
    path: &str,
    format: &FormatId,
    data: &Json,
) -> Result<(Value, Option<String>), String> {
    let d = &store
        .get(path)
        .ok_or_else(|| format!("no value at `{path}`"))?
        .descriptor;
    if *format == PARAM {
        return adapt(d, data);
    }
    let text = data.as_str().ok_or("nothing to paste")?;
    if let Some(e) = store.get(text.trim()) {
        let p = payload(text.trim(), &e.descriptor, &e.value, Text::Path);
        return adapt(d, &p);
    }
    Ok((from_text(d, text)?, None))
}

/// The parameter endpoint: the focused widget's path, or the one a
/// command pinned.
pub struct Endpoint {
    values: Arc<Mutex<Store>>,
    pinned: RefCell<Option<(String, Text)>>,
}

impl Endpoint {
    pub fn new(values: Arc<Mutex<Store>>) -> Self {
        Self {
            values,
            pinned: RefCell::new(None),
        }
    }

    /// Copies and pastes go to `path` until [`Endpoint::unpin`].
    pub fn pin(&self, path: &str, text: Text) {
        *self.pinned.borrow_mut() = Some((path.into(), text));
    }

    pub fn unpin(&self) {
        *self.pinned.borrow_mut() = None;
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The parameter copied from or pasted onto, and what its text is.
    fn current(&self, app: &AppHandle) -> Option<(String, Text)> {
        if let Some(p) = self.pinned.borrow().clone() {
            return Some(p);
        }
        let path = app
            .egui()
            .data(|d| d.get_temp::<String>(egui::Id::new(FOCUS)))?;
        Some((path, Text::Value))
    }
}

/// Says a parameter's widget has the focus: the Edit commands copy it
/// and paste onto it. Called each pass while it has the focus.
pub fn focus(app: &AppHandle, path: &str, widget: egui::Id) {
    app.egui()
        .data_mut(|d| d.insert_temp(egui::Id::new(FOCUS), path.to_string()));
    egui_pop::api::clipboard::bind(app, super::PARAM_ENDPOINT, widget);
}

impl ClipSource for Endpoint {
    fn copy(&self, app: &AppHandle) -> Option<ClipItem> {
        let (path, text) = self.current(app)?;
        let store = self.store();
        let e = store.get(&path)?;
        Some(item(&path, &e.descriptor, &e.value, text))
    }

    fn can_copy(&self, app: &AppHandle) -> bool {
        self.current(app)
            .is_some_and(|(p, _)| self.store().get(&p).is_some())
    }
}

impl ClipTarget for Endpoint {
    fn accepts(&self, app: &AppHandle) -> Vec<FormatId> {
        match self.current(app) {
            Some((p, _)) if self.store().get(&p).is_some() => vec![PARAM, TEXT],
            _ => Vec::new(),
        }
    }

    fn paste(&self, app: &AppHandle, p: &Paste<'_>) -> Result<Json, String> {
        let (path, _) = self.current(app).ok_or("no parameter has the focus")?;
        let mut store = self.store();
        let (value, mut lost) = value_for(&store, &path, &p.repr.format, &p.repr.data)?;
        let e = store
            .get(&path)
            .ok_or_else(|| format!("no value at `{path}`"))?;
        let label = e.descriptor.label.clone();
        let shown_value = shown(&e.descriptor, &value);
        if p.dry_run {
            return Ok(json!(format!("sets {label} to {shown_value}")));
        }
        let mut set = Set::new(&path, Change::To(value), crate::host::to_helios(p.origin)).force();
        if let Some(g) = p.group {
            set = set.gesture(GestureId(g.0));
        }
        let applied = store.set(set).map_err(|e| e.to_string())?;
        if applied.clamped != Clamped::None {
            lost.get_or_insert_with(|| "the part outside its range".into());
        }
        let now = store
            .get(&path)
            .map(|e| shown(&e.descriptor, &applied.value))
            .unwrap_or_default();
        Ok(json!({
            "path": path,
            "value": helios_values::value_to_json(&applied.value),
            "text": now,
            "clamped": applied.clamped,
            "lost": lost,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev() -> Descriptor {
        let mut d = Descriptor::float("Exposure", -3.0, 4.0, 0.0);
        d.unit = Some("EV".into());
        d.precision = 2;
        d
    }

    fn percent() -> Descriptor {
        let mut d = Descriptor::float("Opacity", 0.0, 1.0, 1.0);
        d.unit = Some("%".into());
        d.display_scale = 100.0;
        d.precision = 0;
        d
    }

    #[test]
    fn a_value_reads_as_shown_and_pastes_back() {
        let d = ev();
        let text = shown(&d, &Value::Float(0.3));
        assert_eq!(text, "+0.30 EV");
        assert_eq!(from_text(&d, &text).unwrap(), Value::Float(0.3));
        // A display scale: shown as a percentage, kept as a fraction.
        let p = percent();
        let text = shown(&p, &Value::Float(0.3));
        assert_eq!(from_text(&p, &text).unwrap(), Value::Float(0.3));
        // A comma as the decimal separator.
        assert_eq!(from_text(&d, "-1,5 EV").unwrap(), Value::Float(-1.5));
    }

    #[test]
    fn text_is_read_with_the_target_s_type() {
        let b = Descriptor::bool("Invert", false);
        assert_eq!(from_text(&b, "on").unwrap(), Value::Bool(true));
        assert_eq!(from_text(&b, " Off ").unwrap(), Value::Bool(false));
        assert!(from_text(&b, "maybe").is_err());
        let c = Descriptor::choice("Profile", &["linear", "sigmoid"], "linear");
        assert_eq!(
            from_text(&c, "Sigmoid").unwrap(),
            Value::Choice("sigmoid".into())
        );
        assert!(
            from_text(&c, "cubic")
                .unwrap_err()
                .contains("linear, sigmoid")
        );
        let i = Descriptor::int("Radius", 0, 100, 4);
        assert_eq!(from_text(&i, "12").unwrap(), Value::Int(12));
        assert!(from_text(&i, "1.5").is_err());
        assert!(from_text(&ev(), "a lot").is_err());
    }

    #[test]
    fn lists_go_out_as_json_and_come_back() {
        let mut d = Descriptor::float("Curve", 0.0, 1.0, 0.0);
        d.ty = Type::Floats;
        let v = Value::List(vec![0.0, 0.25, 1.0]);
        let text = shown(&d, &v);
        assert_eq!(text, "[0.0,0.25,1.0]");
        assert_eq!(from_text(&d, &text).unwrap(), v);
    }

    #[test]
    fn a_copied_parameter_pastes_onto_one_of_the_same_kind() {
        let p = payload(
            "node/exposure/exposure",
            &ev(),
            &Value::Float(0.3),
            Text::Value,
        );
        assert_eq!(p["text"], "+0.30 EV");
        assert_eq!(adapt(&ev(), &p).unwrap(), (Value::Float(0.3), None));
        // Onto an integer: rounded, the fraction said.
        let i = Descriptor::int("Radius", 0, 100, 4);
        assert_eq!(
            adapt(&i, &p).unwrap(),
            (Value::Int(0), Some("the fraction".into()))
        );
        // Onto a switch: refused, saying why.
        let b = Descriptor::bool("Invert", false);
        let e = adapt(&b, &p).unwrap_err();
        assert!(e.contains("Exposure is a number"), "{e}");
        // A choice the target does not list.
        let c1 = Descriptor::choice("Profile", &["linear", "sigmoid"], "linear");
        let c2 = Descriptor::choice("Mode", &["fast", "slow"], "fast");
        let p = payload("a", &c1, &Value::Choice("sigmoid".into()), Text::Value);
        assert!(adapt(&c2, &p).is_err());
        assert_eq!(adapt(&c1, &p).unwrap().0, Value::Choice("sigmoid".into()));
    }

    #[test]
    fn copy_path_s_text_pastes_the_value_it_names() {
        let mut store = Store::new();
        store.declare_with("node/a/ev", ev(), Value::Float(1.25));
        store.declare("node/b/ev", ev());
        let it = item("node/a/ev", &ev(), &Value::Float(1.25), Text::Path);
        assert_eq!(it.text(), Some("node/a/ev"));
        // The owner's item: its value.
        let (v, _) = value_for(&store, "node/b/ev", &PARAM, &it.payload).unwrap();
        assert_eq!(v, Value::Float(1.25));
        // Only its text, from the system clipboard: the path's value.
        let (v, _) = value_for(&store, "node/b/ev", &TEXT, &json!("node/a/ev")).unwrap();
        assert_eq!(v, Value::Float(1.25));
        // A value as shown.
        let (v, _) = value_for(&store, "node/b/ev", &TEXT, &json!("+0.30 EV")).unwrap();
        assert_eq!(v, Value::Float(0.3));
    }
}
