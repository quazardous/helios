//! darktable's params layouts, and decoding raw params with them.
//!
//! `layouts.json` comes from darktable's own introspection (see
//! `tools/dt_param_layouts.sh`): for each module, the version of its params
//! and every field with its type, offset and size. Decoding is a walk of
//! that description over the bytes, little-endian as darktable writes them
//! on the platforms it runs on.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value, json};

/// A field of a params struct, as darktable's introspection describes it.
#[derive(Clone, Debug, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub offset: usize,
    pub size: usize,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub default: Option<Value>,
    /// Enum values: name and number.
    #[serde(default)]
    pub values: Vec<EnumValue>,
    /// Array length and element.
    #[serde(default)]
    pub count: Option<usize>,
    #[serde(default)]
    pub element: Option<Box<Field>>,
    /// Struct or union members.
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EnumValue {
    pub name: String,
    pub value: i64,
}

/// The layout of one module's params at one version.
#[derive(Clone, Debug, Deserialize)]
pub struct ModuleLayout {
    pub module: String,
    pub version: u32,
    pub size: usize,
    pub params: Field,
}

#[derive(Deserialize)]
struct File {
    modules: Vec<ModuleLayout>,
}

/// Every layout known, by module.
pub struct Layouts {
    modules: BTreeMap<String, Vec<ModuleLayout>>,
}

/// The layouts of the darktable build Helios was generated against.
const BUILT_IN: &str = include_str!("../layouts.json");

impl Layouts {
    pub fn built_in() -> Self {
        Self::from_json(BUILT_IN).expect("the built-in layouts parse")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(text).map_err(|e| format!("layouts: {e}"))?;
        let mut modules: BTreeMap<String, Vec<ModuleLayout>> = BTreeMap::new();
        for m in file.modules {
            modules.entry(m.module.clone()).or_default().push(m);
        }
        Ok(Self { modules })
    }

    /// The layout of a module at a version, if known.
    pub fn get(&self, module: &str, version: u32) -> Option<&ModuleLayout> {
        self.modules
            .get(module)?
            .iter()
            .find(|m| m.version == version)
    }

    /// The versions known for a module.
    pub fn versions(&self, module: &str) -> Vec<u32> {
        self.modules
            .get(module)
            .map(|v| v.iter().map(|m| m.version).collect())
            .unwrap_or_default()
    }

    /// Every module and version known.
    pub fn all(&self) -> Vec<(&str, u32)> {
        self.modules
            .iter()
            .flat_map(|(name, v)| v.iter().map(move |m| (name.as_str(), m.version)))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

fn bytes<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], String> {
    data.get(at..at + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| {
            format!(
                "params end at {} bytes, field needs {}..{}",
                data.len(),
                at,
                at + N
            )
        })
}

fn field(f: &Field, data: &[u8], base: usize) -> Result<Value, String> {
    let at = base + f.offset;
    Ok(match (f.kind.as_str(), f.size) {
        ("float", 4) => json!(f32::from_le_bytes(bytes(data, at)?)),
        ("double", 8) => json!(f64::from_le_bytes(bytes(data, at)?)),
        ("int" | "long" | "enum", 4) => {
            let v = i32::from_le_bytes(bytes(data, at)?);
            match f.values.iter().find(|e| e.value == i64::from(v)) {
                Some(e) => json!(e.name),
                None => json!(v),
            }
        }
        ("uint" | "ulong", 4) => json!(u32::from_le_bytes(bytes(data, at)?)),
        ("long", 8) => json!(i64::from_le_bytes(bytes(data, at)?)),
        ("ulong", 8) => json!(u64::from_le_bytes(bytes(data, at)?)),
        ("short", 2) => json!(i16::from_le_bytes(bytes(data, at)?)),
        ("ushort", 2) => json!(u16::from_le_bytes(bytes(data, at)?)),
        ("int8" | "char", 1) => json!(i8::from_le_bytes(bytes(data, at)?)),
        ("uint8", 1) => json!(bytes::<1>(data, at)?[0]),
        ("bool", 4) => json!(u32::from_le_bytes(bytes(data, at)?) != 0),
        ("bool", 1) => json!(bytes::<1>(data, at)?[0] != 0),
        ("array", _) => {
            let element = f
                .element
                .as_deref()
                .ok_or("an array without element type")?;
            let count = f.count.unwrap_or(0);
            if element.kind == "char" {
                // A fixed-size string.
                let raw = data
                    .get(at..at + count)
                    .ok_or_else(|| format!("params too short for `{}`", f.name))?;
                let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
                json!(String::from_utf8_lossy(&raw[..end]))
            } else {
                // Offsets inside an element (its own, its members') are
                // those of the first element: shift the base for the others.
                let items = (0..count)
                    .map(|i| field(element, data, base + i * element.size))
                    .collect::<Result<Vec<_>, _>>()?;
                Value::Array(items)
            }
        }
        ("struct", _) => {
            let mut map = Map::new();
            for member in &f.fields {
                map.insert(member.name.clone(), field(member, data, base)?);
            }
            Value::Object(map)
        }
        (kind, size) => json!({ "undecoded": kind, "size": size }),
    })
}

/// Decodes raw params with a module's layout into named values. The size
/// must match: a mismatch means another version than the one claimed.
pub fn decode(layout: &ModuleLayout, data: &[u8]) -> Result<Value, String> {
    if data.len() != layout.size {
        return Err(format!(
            "`{}` v{}: params are {} bytes, its layout {}",
            layout.module,
            layout.version,
            data.len(),
            layout.size
        ));
    }
    field(&layout.params, data, 0)
}

fn slot<'a>(out: &'a mut [u8], f: &Field, at: usize, n: usize) -> Result<&'a mut [u8], String> {
    let len = out.len();
    out.get_mut(at..at + n)
        .ok_or_else(|| format!("`{}` ends past the params' {len} bytes", f.name))
}

fn put(f: &Field, given: Option<&Value>, out: &mut [u8], base: usize) -> Result<(), String> {
    let at = base + f.offset;
    let value = given.or(f.default.as_ref());
    let number = |v: Option<&Value>| -> Result<f64, String> {
        match v {
            None => Ok(0.0),
            Some(Value::Bool(b)) => Ok(f64::from(u8::from(*b))),
            Some(Value::String(name)) => f
                .values
                .iter()
                .find(|e| e.name == *name)
                .map(|e| e.value as f64)
                .ok_or_else(|| format!("`{}`: no value named `{name}`", f.name)),
            Some(v) => v
                .as_f64()
                .ok_or_else(|| format!("`{}`: `{v}` is not a number", f.name)),
        }
    };
    match (f.kind.as_str(), f.size) {
        ("float", 4) => {
            slot(out, f, at, 4)?.copy_from_slice(&(number(value)? as f32).to_le_bytes());
        }
        ("double", 8) => slot(out, f, at, 8)?.copy_from_slice(&number(value)?.to_le_bytes()),
        ("int" | "long" | "enum", 4) => {
            slot(out, f, at, 4)?.copy_from_slice(&(number(value)? as i32).to_le_bytes());
        }
        ("uint" | "ulong" | "bool", 4) => {
            slot(out, f, at, 4)?.copy_from_slice(&(number(value)? as u32).to_le_bytes());
        }
        ("long", 8) => slot(out, f, at, 8)?.copy_from_slice(&(number(value)? as i64).to_le_bytes()),
        ("ulong", 8) => {
            slot(out, f, at, 8)?.copy_from_slice(&(number(value)? as u64).to_le_bytes());
        }
        ("short", 2) => {
            slot(out, f, at, 2)?.copy_from_slice(&(number(value)? as i16).to_le_bytes());
        }
        ("ushort", 2) => {
            slot(out, f, at, 2)?.copy_from_slice(&(number(value)? as u16).to_le_bytes());
        }
        ("int8" | "char", 1) => slot(out, f, at, 1)?[0] = (number(value)? as i8) as u8,
        ("uint8" | "bool", 1) => slot(out, f, at, 1)?[0] = number(value)? as u8,
        ("array", _) => {
            let element = f
                .element
                .as_deref()
                .ok_or("an array without element type")?;
            let count = f.count.unwrap_or(0);
            if element.kind == "char" {
                let text = value.and_then(Value::as_str).unwrap_or("");
                if text.len() >= count {
                    return Err(format!("`{}`: text longer than {count} bytes", f.name));
                }
                slot(out, f, at, count)?[..text.len()].copy_from_slice(text.as_bytes());
            } else {
                let items = value.and_then(Value::as_array);
                for i in 0..count {
                    let item = items.and_then(|a| a.get(i));
                    put(element, item, out, base + i * element.size)?;
                }
            }
        }
        ("struct", _) => {
            for member in &f.fields {
                put(member, value.and_then(|v| v.get(&member.name)), out, base)?;
            }
        }
        (kind, size) => {
            return Err(format!(
                "`{}`: cannot encode {kind} of {size} bytes",
                f.name
            ));
        }
    }
    Ok(())
}

/// Encodes named values into raw params with a module's layout: the
/// inverse of [`decode`]. Fields left out take their default (zero when
/// darktable declares none); enums take their name or their number.
pub fn encode(layout: &ModuleLayout, values: &Value) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; layout.size];
    put(&layout.params, Some(values), &mut out, 0)?;
    Ok(out)
}
