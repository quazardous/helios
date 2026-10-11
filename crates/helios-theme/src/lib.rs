//! Themes as Design Tokens (the W3C community group's format, DTCG):
//! JSON files of named, typed values, read by the GUI by name and never
//! hard-coded in a widget. See `docs/gui.md`, "Themes and profiles".
//!
//! A theme file is a tree of groups and tokens. A token is an object with
//! a `$value`, and a `$type` given on the token or inherited from its
//! group. A token's name is its path, dotted (`fg.weak`). A value can be an
//! alias of another token, `{accent.base}`. At the root, `$extends` names the
//! theme this one builds on: every token not given here comes from it.
//! Unknown data goes under `$extensions`, which is kept and ignored.
//!
//! Types read here: `color` (`#rrggbb`, `#rrggbbaa`, or DTCG's
//! `{"colorSpace": "srgb", "components": [r, g, b], "alpha": a}`),
//! `dimension` (`"4px"` or `{"value": 4, "unit": "px"}`), `number` and
//! `fontFamily` (a name or a list, first preferred).
//!
//! Every theme answers the same [`CONTRACT`]: the tokens the GUI reads.
//! A theme missing one, or giving one the wrong type, falls back to the
//! built-in dark theme for it and says so; the GUI never breaks on a theme.

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json, json};

/// A token's value, resolved.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// sRGB, straight alpha, each from 0 to 1.
    Color([f32; 4]),
    /// In logical pixels.
    Dimension(f32),
    Number(f32),
    /// Families, the first preferred.
    Font(Vec<String>),
}

impl Value {
    fn kind(&self) -> &'static str {
        match self {
            Value::Color(_) => "color",
            Value::Dimension(_) => "dimension",
            Value::Number(_) => "number",
            Value::Font(_) => "fontFamily",
        }
    }

    fn to_json(&self) -> Json {
        match self {
            Value::Color(c) => {
                let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                let hex = format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]));
                if c[3] < 1.0 {
                    json!(format!("{hex}{:02x}", b(c[3])))
                } else {
                    json!(hex)
                }
            }
            Value::Dimension(d) => json!(format!("{d}px")),
            Value::Number(n) => json!(n),
            Value::Font(f) => json!(f),
        }
    }
}

/// The tokens every theme provides, with their type and meaning.
pub const CONTRACT: &[(&str, &str, &str)] = &[
    ("bg.canvas", "color", "behind images and graphs"),
    ("bg.panel", "color", "panels and side bars"),
    ("bg.window", "color", "dialogs, menus, the palette"),
    ("bg.extreme", "color", "text fields, the deepest background"),
    ("bg.widget", "color", "buttons and controls at rest"),
    ("bg.widget_hover", "color", "controls under the pointer"),
    (
        "bg.widget_active",
        "color",
        "controls being pressed or dragged",
    ),
    ("fg.text", "color", "ordinary text"),
    ("fg.weak", "color", "secondary text, hints"),
    ("fg.strong", "color", "titles, emphasis"),
    ("border", "color", "panel and widget outlines"),
    ("accent.base", "color", "focus, selection, primary actions"),
    ("accent.text", "color", "text on the accent"),
    ("selection", "color", "selected text and items"),
    ("danger", "color", "destructive actions, errors"),
    ("warning", "color", "warnings"),
    ("state.done", "color", "a node computed and current"),
    ("state.running", "color", "a node being computed"),
    (
        "state.stale",
        "color",
        "a node whose content is out of date",
    ),
    ("state.failed", "color", "a node whose computation failed"),
    ("state.pending", "color", "a decision waiting for an answer"),
    ("radius.widget", "dimension", "corner radius of controls"),
    (
        "radius.window",
        "dimension",
        "corner radius of windows and menus",
    ),
    ("space.item", "dimension", "space between items"),
    ("font.size", "dimension", "body text size"),
    ("font.ui", "fontFamily", "interface text"),
    ("font.mono", "fontFamily", "numbers and code"),
];

/// A theme as written: its name, what it extends, its raw tokens by path.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    pub extends: Option<String>,
    /// Path to (type if known, raw value).
    pub tokens: BTreeMap<String, (Option<String>, Json)>,
}

const BUILT_IN: &[(&str, &str)] = &[
    (
        "helios-dark",
        include_str!("../themes/helios-dark.tokens.json"),
    ),
    (
        "helios-light",
        include_str!("../themes/helios-light.tokens.json"),
    ),
    (
        "helios-high-contrast",
        include_str!("../themes/helios-high-contrast.tokens.json"),
    ),
];

/// The theme every other falls back to.
pub const BASE: &str = "helios-dark";

impl Theme {
    /// Reads a theme file. `name` is used when the file gives none
    /// (`$name` at the root, a Helios addition).
    pub fn parse(name: &str, text: &str) -> Result<Theme, String> {
        let root: Json = serde_json::from_str(text).map_err(|e| format!("theme `{name}`: {e}"))?;
        let obj = root
            .as_object()
            .ok_or_else(|| format!("theme `{name}`: not a JSON object"))?;
        let mut tokens = BTreeMap::new();
        walk(obj, "", None, &mut tokens);
        Ok(Theme {
            name: obj
                .get("$name")
                .and_then(Json::as_str)
                .unwrap_or(name)
                .to_string(),
            extends: obj.get("$extends").and_then(Json::as_str).map(String::from),
            tokens,
        })
    }

    /// The built-in themes, by name.
    pub fn built_in() -> Vec<Theme> {
        BUILT_IN
            .iter()
            .map(|(n, t)| Theme::parse(n, t).expect("built-in themes parse"))
            .collect()
    }
}

fn walk(
    obj: &Map<String, Json>,
    prefix: &str,
    group_type: Option<&str>,
    out: &mut BTreeMap<String, (Option<String>, Json)>,
) {
    let own_type = obj.get("$type").and_then(Json::as_str).or(group_type);
    for (key, v) in obj {
        if key.starts_with('$') {
            continue;
        }
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let Some(child) = v.as_object() else { continue };
        if let Some(value) = child.get("$value") {
            let kind = child
                .get("$type")
                .and_then(Json::as_str)
                .or(own_type)
                .map(String::from);
            out.insert(path, (kind, value.clone()));
        } else {
            walk(child, &path, own_type, out);
        }
    }
}

fn hex(text: &str) -> Option<[f32; 4]> {
    let h = text.strip_prefix('#')?;
    let byte = |i: usize| {
        u8::from_str_radix(h.get(i..i + 2)?, 16)
            .ok()
            .map(|b| f32::from(b) / 255.0)
    };
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 1.0]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// A raw value of a type, aliases already followed.
fn typed(kind: &str, raw: &Json) -> Result<Value, String> {
    let bad = || format!("`{raw}` is not a {kind}");
    match kind {
        "color" => match raw {
            Json::String(s) => hex(s).map(Value::Color).ok_or_else(bad),
            Json::Object(o) => {
                if o.get("colorSpace").and_then(Json::as_str) != Some("srgb") {
                    return Err(format!("`{raw}`: only srgb colors are read"));
                }
                let c = o
                    .get("components")
                    .and_then(Json::as_array)
                    .filter(|c| c.len() == 3)
                    .ok_or_else(bad)?;
                let f = |i: usize| c[i].as_f64().map(|v| v as f32).ok_or_else(bad);
                let alpha = o.get("alpha").and_then(Json::as_f64).unwrap_or(1.0) as f32;
                Ok(Value::Color([f(0)?, f(1)?, f(2)?, alpha]))
            }
            _ => Err(bad()),
        },
        "dimension" => match raw {
            Json::String(s) => s
                .strip_suffix("px")
                .and_then(|n| n.trim().parse().ok())
                .map(Value::Dimension)
                .ok_or_else(bad),
            Json::Object(o) if o.get("unit").and_then(Json::as_str) == Some("px") => o
                .get("value")
                .and_then(Json::as_f64)
                .map(|v| Value::Dimension(v as f32))
                .ok_or_else(bad),
            _ => Err(bad()),
        },
        "number" => raw
            .as_f64()
            .map(|v| Value::Number(v as f32))
            .ok_or_else(bad),
        "fontFamily" => match raw {
            Json::String(s) => Ok(Value::Font(vec![s.clone()])),
            Json::Array(a) => a
                .iter()
                .map(|f| f.as_str().map(String::from).ok_or_else(bad))
                .collect::<Result<_, _>>()
                .map(Value::Font),
            _ => Err(bad()),
        },
        other => Err(format!("type `{other}` is not read by Helios")),
    }
}

/// A theme ready to use: every contract token has a value.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub name: String,
    pub values: BTreeMap<String, Value>,
    /// What was wrong and fell back, in words; empty for a clean theme.
    pub problems: Vec<String>,
}

impl Resolved {
    pub fn color(&self, token: &str) -> [f32; 4] {
        match self.values.get(token) {
            Some(Value::Color(c)) => *c,
            _ => [1.0, 0.0, 1.0, 1.0],
        }
    }

    pub fn size(&self, token: &str) -> f32 {
        match self.values.get(token) {
            Some(Value::Dimension(d) | Value::Number(d)) => *d,
            _ => 0.0,
        }
    }

    pub fn font(&self, token: &str) -> Vec<String> {
        match self.values.get(token) {
            Some(Value::Font(f)) => f.clone(),
            _ => Vec::new(),
        }
    }

    /// The theme as one DTCG file, overrides merged, nothing extended:
    /// what `theme.export` writes.
    pub fn to_tokens(&self) -> Json {
        let mut root = Map::new();
        root.insert("$name".into(), json!(self.name));
        for (path, value) in &self.values {
            let mut node = &mut root;
            let parts: Vec<&str> = path.split('.').collect();
            for part in &parts[..parts.len() - 1] {
                node = node
                    .entry(part.to_string())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .expect("groups are objects");
            }
            let leaf = parts[parts.len() - 1];
            let entry = node.entry(leaf.to_string()).or_insert_with(|| json!({}));
            entry["$type"] = json!(value.kind());
            entry["$value"] = value.to_json();
        }
        Json::Object(root)
    }
}

/// The themes known: the built-in ones and any added (a plugin's, the
/// user's).
pub struct Themes {
    themes: BTreeMap<String, Theme>,
}

impl Default for Themes {
    fn default() -> Self {
        let mut themes = Themes {
            themes: BTreeMap::new(),
        };
        for t in Theme::built_in() {
            themes.add(t);
        }
        themes
    }
}

impl Themes {
    pub fn add(&mut self, theme: Theme) {
        self.themes.insert(theme.name.clone(), theme);
    }

    pub fn names(&self) -> Vec<&str> {
        self.themes.keys().map(String::as_str).collect()
    }

    /// The raw tokens of a theme with everything it extends, the theme's
    /// own winning; an unknown base falls back to [`BASE`], a cycle stops.
    fn flatten(
        &self,
        name: &str,
        problems: &mut Vec<String>,
    ) -> BTreeMap<String, (Option<String>, Json)> {
        let mut chain = Vec::new();
        let mut at = Some(name.to_string());
        while let Some(n) = at {
            if chain.contains(&n) {
                problems.push(format!(
                    "`{n}` extends itself through {}",
                    chain.join(" → ")
                ));
                break;
            }
            let Some(theme) = self.themes.get(&n) else {
                problems.push(format!("no theme `{n}`: {BASE} is used instead"));
                chain.push(BASE.to_string());
                break;
            };
            chain.push(n.clone());
            at = theme.extends.clone();
        }
        let mut tokens = BTreeMap::new();
        for n in chain.iter().rev() {
            if let Some(theme) = self.themes.get(n) {
                tokens.extend(theme.tokens.clone());
            }
        }
        tokens
    }

    /// A theme ready to use, with `overrides` (the user's tokens, by path)
    /// applied last. Unknown override tokens are reported, not fatal.
    pub fn resolve(&self, name: &str, overrides: &BTreeMap<String, Json>) -> Resolved {
        let mut problems = Vec::new();
        let mut raw = self.flatten(name, &mut problems);
        let contract: BTreeMap<&str, &str> = CONTRACT.iter().map(|(p, t, _)| (*p, *t)).collect();
        for (path, value) in overrides {
            match contract.get(path.as_str()) {
                Some(kind) => {
                    raw.insert(path.clone(), (Some(kind.to_string()), value.clone()));
                }
                None => problems.push(format!("override `{path}`: no such token")),
            }
        }
        let base = (name != BASE).then(|| self.resolve(BASE, &BTreeMap::new()));
        let mut values = BTreeMap::new();
        for (path, kind, _) in CONTRACT {
            let got = lookup(&raw, path, &mut Vec::new()).and_then(|(declared, v)| {
                if declared.as_deref().is_some_and(|d| d != *kind) {
                    return Err(format!(
                        "is a {}, not a {kind}",
                        declared.unwrap_or_default()
                    ));
                }
                typed(kind, &v)
            });
            match got {
                Ok(v) => {
                    values.insert(path.to_string(), v);
                }
                Err(e) => {
                    problems.push(format!("`{path}` {e}"));
                    if let Some(v) = base.as_ref().and_then(|b| b.values.get(*path)) {
                        values.insert(path.to_string(), v.clone());
                    }
                }
            }
        }
        Resolved {
            name: name.to_string(),
            values,
            problems,
        }
    }
}

/// A token's raw value with aliases followed (`{path}`), and its type.
fn lookup(
    raw: &BTreeMap<String, (Option<String>, Json)>,
    path: &str,
    seen: &mut Vec<String>,
) -> Result<(Option<String>, Json), String> {
    if seen.iter().any(|s| s == path) {
        return Err(format!("is an alias cycle: {} → {path}", seen.join(" → ")));
    }
    seen.push(path.to_string());
    let (kind, value) = raw.get(path).ok_or("is missing")?;
    match value
        .as_str()
        .and_then(|s| s.strip_prefix('{')?.strip_suffix('}'))
    {
        Some(target) => {
            let (target_kind, v) = lookup(raw, target, seen)?;
            Ok((kind.clone().or(target_kind), v))
        }
        None => Ok((kind.clone(), value.clone())),
    }
}
