//! `param.list`, `param.get`, `param.set`, `param.reset`, `param.undo` and
//! `param.redo`: the store as commands, what menus, keys, scripts and
//! agents (through MCP) run alike. Values are plain JSON (a number, a
//! bool, a string, an array), typed by the path's descriptor.

use std::sync::{Arc, Mutex};

use helios_core::Value;
use helios_shell::commands::{CommandSpec, Origin, Registry};
use serde_json::{Value as Json, json};

use crate::{Change, Entry, Set, Store, Type};

/// A JSON value as the type a path holds.
pub fn value_from_json(ty: Type, json: &Json) -> Result<Value, String> {
    let wrong = || format!("{json} is not a {ty:?}");
    Ok(match ty {
        Type::Float => Value::Float(json.as_f64().ok_or_else(wrong)?),
        Type::Int => match json.as_i64() {
            Some(v) => Value::Int(v),
            None => Value::Float(json.as_f64().ok_or_else(wrong)?),
        },
        Type::Bool => Value::Bool(json.as_bool().ok_or_else(wrong)?),
        Type::Choice => Value::Choice(json.as_str().ok_or_else(wrong)?.into()),
        Type::Text => Value::Text(json.as_str().ok_or_else(wrong)?.into()),
        Type::Floats => Value::List(
            json.as_array()
                .ok_or_else(wrong)?
                .iter()
                .map(|v| v.as_f64().ok_or_else(wrong))
                .collect::<Result<_, _>>()?,
        ),
        Type::Ints => Value::Ints(
            json.as_array()
                .ok_or_else(wrong)?
                .iter()
                .map(|v| v.as_i64().ok_or_else(wrong))
                .collect::<Result<_, _>>()?,
        ),
    })
}

pub fn value_to_json(value: &Value) -> Json {
    match value {
        Value::Float(v) => json!(v),
        Value::Int(v) => json!(v),
        Value::Bool(v) => json!(v),
        Value::Choice(v) | Value::Text(v) => json!(v),
        Value::List(v) => json!(v),
        Value::Ints(v) => json!(v),
    }
}

fn describe(path: &str, e: &Entry) -> Json {
    json!({
        "path": path,
        "value": value_to_json(&e.value),
        "default": value_to_json(&e.descriptor.default),
        "changed": e.value != e.descriptor.default,
        "seq": e.seq,
        "origin": e.origin,
        "descriptor": e.descriptor,
    })
}

fn path_of(args: &Json) -> Result<&str, String> {
    args.get("path")
        .and_then(Json::as_str)
        .ok_or_else(|| "`path` is missing".to_string())
}

/// What `param.set` asks for: `value`, `step` (fine steps, negative for
/// down), `coarse`, `reset`, `top` or `bottom`.
fn change_of(store: &Store, path: &str, args: &Json) -> Result<Change, String> {
    let n = |key: &str| args.get(key).and_then(Json::as_i64);
    let flag = |key: &str| args.get(key).and_then(Json::as_bool) == Some(true);
    if let Some(v) = args.get("value") {
        let ty = store
            .get(path)
            .ok_or_else(|| format!("no value at `{path}`"))?
            .descriptor
            .ty;
        Ok(Change::To(value_from_json(ty, v)?))
    } else if let Some(s) = n("step") {
        Ok(Change::Step(s))
    } else if let Some(s) = n("coarse") {
        Ok(Change::Coarse(s))
    } else if flag("reset") {
        Ok(Change::Reset)
    } else if flag("top") {
        Ok(Change::Top)
    } else if flag("bottom") {
        Ok(Change::Bottom)
    } else {
        Err("say what to set: `value`, `step`, `coarse`, `reset`, `top` or `bottom`".into())
    }
}

fn set(store: &Mutex<Store>, args: &Json, origin: &Origin, reset: bool) -> Result<Json, String> {
    let mut store = store.lock().expect("value store lock");
    let path = path_of(args)?;
    let change = if reset {
        Change::Reset
    } else {
        change_of(&store, path, args)?
    };
    let mut s = Set::new(path, change, origin.clone());
    s.force = args.get("force").and_then(Json::as_bool) == Some(true);
    s.if_seq = args.get("if_seq").and_then(Json::as_u64);
    match store.set(s) {
        Ok(a) => Ok(json!({
            "value": value_to_json(&a.value),
            "clamped": a.clamped,
            "seq": a.seq,
            "changed": a.changed,
        })),
        // The refusal as JSON, current value included, so an agent can
        // read it and decide.
        Err(r) => Err(serde_json::to_string(&r).unwrap_or_else(|_| r.to_string())),
    }
}

/// Declares the `param.*` commands on a registry, over a shared store.
pub fn register(registry: &mut Registry, store: Arc<Mutex<Store>>) -> Result<(), String> {
    let s = store.clone();
    registry.register(
        CommandSpec::new(
            "param.list",
            "List the values under a path (`prefix`)",
            "Values",
        ),
        move |args, _| {
            let prefix = args.get("prefix").and_then(Json::as_str).unwrap_or("");
            let store = s.lock().expect("value store lock");
            Ok(Json::Array(
                store
                    .list(prefix)
                    .into_iter()
                    .map(|(p, e)| describe(p, e))
                    .collect(),
            ))
        },
    )?;
    let s = store.clone();
    registry.register(
        CommandSpec::new("param.get", "Read a value (`path`)", "Values"),
        move |args, _| {
            let path = path_of(args)?;
            let store = s.lock().expect("value store lock");
            store
                .get(path)
                .map(|e| describe(path, e))
                .ok_or_else(|| format!("no value at `{path}`"))
        },
    )?;
    let s = store.clone();
    registry.register(
        CommandSpec::new(
            "param.set",
            "Set a value (`path`; `value`, `step`, `coarse`, `reset`, `top` or `bottom`; `force`, `if_seq`)",
            "Values",
        ),
        move |args, origin| set(&s, args, origin, false),
    )?;
    let s = store.clone();
    registry.register(
        CommandSpec::new(
            "param.reset",
            "Reset a value to its default (`path`)",
            "Values",
        ),
        move |args, origin| set(&s, args, origin, true),
    )?;
    let s = store.clone();
    registry.register(
        CommandSpec::new("param.undo", "Undo the last change of values", "Values"),
        move |_, origin| {
            let undone = s
                .lock()
                .expect("value store lock")
                .undo(origin)
                .map_err(|r| r.to_string())?;
            Ok(json!({ "undone": undone }))
        },
    )?;
    let s = store;
    registry.register(
        CommandSpec::new(
            "param.redo",
            "Redo the last undone change of values",
            "Values",
        ),
        move |_, origin| {
            let redone = s
                .lock()
                .expect("value store lock")
                .redo(origin)
                .map_err(|r| r.to_string())?;
            Ok(json!({ "redone": redone }))
        },
    )?;
    Ok(())
}
