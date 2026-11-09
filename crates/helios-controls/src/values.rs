//! `helios_values::Store` seen through `egui_hudget`'s store traits:
//! [`StoreRef`] reads, [`StoreMut`] also writes inside gestures. Helios's
//! values and descriptors map to the framework's ([`value`],
//! [`descriptor`]), `ParamWidget` included.

use egui_hudget::value::{self as hv, Param, ParamDescriptor, ParamType, ValueSource, ValueStore};
use helios_core::{ParamWidget, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, GestureId, Set, Store, Type};

/// A Helios value as the framework's.
pub fn value(v: &Value) -> hv::Value {
    match v {
        Value::Float(v) => hv::Value::Float(*v),
        Value::Int(v) => hv::Value::Int(*v),
        Value::Bool(v) => hv::Value::Bool(*v),
        Value::Choice(v) => hv::Value::Choice(v.clone()),
        Value::List(v) => hv::Value::Floats(v.clone()),
        Value::Ints(v) => hv::Value::Ints(v.clone()),
        Value::Text(v) => hv::Value::Text(v.clone()),
    }
}

/// The framework's value as a Helios one.
pub fn helios_value(v: hv::Value) -> Value {
    match v {
        hv::Value::Float(v) => Value::Float(v),
        hv::Value::Int(v) => Value::Int(v),
        hv::Value::Bool(v) => Value::Bool(v),
        hv::Value::Choice(v) => Value::Choice(v),
        hv::Value::Floats(v) => Value::List(v),
        hv::Value::Ints(v) => Value::Ints(v),
        hv::Value::Text(v) => Value::Text(v),
    }
}

/// A parameter widget as the framework's.
pub fn widget(w: ParamWidget) -> hv::Widget {
    match w {
        ParamWidget::Curve => hv::Widget::Curve,
        ParamWidget::Wheel => hv::Widget::Wheel,
    }
}

/// A Helios descriptor as the framework's parameter descriptor.
pub fn descriptor(d: &Descriptor) -> ParamDescriptor {
    ParamDescriptor {
        ty: match d.ty {
            Type::Float => ParamType::Float,
            Type::Int => ParamType::Int,
            Type::Bool => ParamType::Bool,
            Type::Choice => ParamType::Choice,
            Type::Floats => ParamType::Floats,
            Type::Ints => ParamType::Ints,
            Type::Text => ParamType::Text,
        },
        label: d.label.clone(),
        description: d.description.clone(),
        default: value(&d.default),
        hard: d.hard,
        soft: d.soft,
        step: d.step,
        coarse: d.coarse,
        unit: d.unit.clone(),
        display_scale: d.display_scale,
        precision: d.precision,
        choices: d.choices.clone(),
        logarithmic: d.logarithmic,
        widget: d.widget.map(widget),
        module: d.module.clone(),
    }
}

fn get(s: &Store, path: &str) -> Option<hv::Value> {
    s.get(path).map(|e| value(&e.value))
}

fn default_value(s: &Store, path: &str) -> Option<hv::Value> {
    s.get(path).map(|e| value(&e.descriptor.default))
}

fn params(s: &Store, prefix: &str) -> Vec<Param> {
    s.list(prefix)
        .into_iter()
        .map(|(path, e)| Param {
            path: path.to_owned(),
            value: value(&e.value),
            descriptor: descriptor(&e.descriptor),
        })
        .collect()
}

/// A store read through the framework.
#[derive(Clone, Copy)]
pub struct StoreRef<'a>(pub &'a Store);

impl ValueSource for StoreRef<'_> {
    fn get(&self, path: &str) -> Option<hv::Value> {
        get(self.0, path)
    }

    fn default_value(&self, path: &str) -> Option<hv::Value> {
        default_value(self.0, path)
    }

    fn params(&self, prefix: &str) -> Vec<Param> {
        params(self.0, prefix)
    }
}

/// A store read and written through the framework: its gestures are the
/// store's, held against other origins, one undo step each.
pub struct StoreMut<'a>(pub &'a mut Store);

impl ValueSource for StoreMut<'_> {
    fn get(&self, path: &str) -> Option<hv::Value> {
        get(self.0, path)
    }

    fn default_value(&self, path: &str) -> Option<hv::Value> {
        default_value(self.0, path)
    }

    fn params(&self, prefix: &str) -> Vec<Param> {
        params(self.0, prefix)
    }
}

impl ValueStore for StoreMut<'_> {
    type Origin = Origin;
    type Gesture = GestureId;

    fn begin(&mut self, origin: &Origin, label: &str) -> GestureId {
        self.0.begin_gesture(origin.clone(), label)
    }

    fn preview(
        &mut self,
        gesture: GestureId,
        path: &str,
        v: hv::Value,
        origin: &Origin,
        force: bool,
    ) -> Result<hv::Value, String> {
        let mut set = Set::new(path, Change::To(helios_value(v)), origin.clone()).gesture(gesture);
        set.force = force;
        let applied = self.0.set(set).map_err(|r| r.to_string())?;
        Ok(value(&applied.value))
    }

    fn commit(&mut self, gesture: GestureId) {
        self.0.end(gesture);
    }

    fn cancel(&mut self, gesture: GestureId) -> usize {
        self.0.cancel(gesture)
    }

    fn device(origin: &Origin) -> &'static str {
        match origin {
            Origin::User => "pointer",
            Origin::Agent(_) => "agent",
            Origin::Script(_) => "script",
            Origin::Midi => "midi",
            Origin::Remote(_) => "remote",
        }
    }
}
