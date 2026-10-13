//! Prompts as versioned assets.
//!
//! A [`Prompt`] is a file: its id, its version, its capability, the
//! models it was tuned for, its text (a `template` with `{{name}}` slots
//! filled from the input) and the JSON schemas of its input and output.
//!
//! - Shipped prompts ([`SHIPPED`]) are built into Helios from
//!   `crates/helios-ai/prompts/` and never change once released.
//! - User prompts are files in the AI directory's `prompts/`, one per
//!   version (`<id>@<version>.json`, or `.toml` written by hand).
//!   [`Library::set`] always writes a new version: a version, once written,
//!   never changes, so a pinned reference keeps its meaning.
//! - A reference ([`PromptRef`]) is `id` (the latest version) or
//!   `id@version` (pinned).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Capability;
use crate::schema;

/// The prompts shipped with Helios: file name and text.
pub const SHIPPED: &[(&str, &str)] = &[
    (
        "image.describe@1.toml",
        include_str!("../prompts/image.describe@1.toml"),
    ),
    (
        "image.assess@1.toml",
        include_str!("../prompts/image.assess@1.toml"),
    ),
    (
        "intent.adjustments@1.toml",
        include_str!("../prompts/intent.adjustments@1.toml"),
    ),
];

/// Where a prompt version comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Shipped,
    User,
}

fn object_schema() -> Value {
    json!({ "type": "object" })
}

/// One version of a prompt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    /// Dotted words: `image.describe`, `stacks.suggest`.
    pub id: String,
    pub version: u32,
    pub capability: Capability,
    #[serde(default)]
    pub description: String,
    /// The models this version was written and checked against.
    #[serde(default)]
    pub tuned_for: Vec<String>,
    /// How many images it takes (vision prompts).
    #[serde(default)]
    pub images: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system: String,
    /// The text sent, `{{name}}` slots filled from the input.
    pub template: String,
    /// The schema of the input.
    #[serde(default = "object_schema")]
    pub input: Value,
    /// The schema of the answer; none for free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// An input to try it with (the bench's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<Value>,
    #[serde(default)]
    pub temperature: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default)]
    pub origin: Origin,
}

impl Prompt {
    /// `id@version`.
    pub fn reference(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    /// A hash of what is sent and checked: two versions with the same hash
    /// ask the same thing.
    pub fn hash(&self) -> String {
        let text = json!({
            "system": self.system, "template": self.template,
            "input": self.input, "output": self.output,
            "temperature": self.temperature, "seed": self.seed,
        });
        blake3::hash(text.to_string().as_bytes()).to_hex()[..16].to_string()
    }

    /// What is wrong with it, if anything.
    pub fn check(&self) -> Result<(), String> {
        check_id(&self.id)?;
        if self.version == 0 {
            return Err("versions start at 1".into());
        }
        if self.template.trim().is_empty() {
            return Err(format!("{}: the template is empty", self.reference()));
        }
        let mut problems = schema::lint(&self.input);
        if let Some(out) = &self.output {
            problems.extend(schema::lint(out));
        }
        if self.capability == Capability::Vision && self.images == 0 {
            problems.push("a vision prompt takes at least one image (`images`)".into());
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(format!("{}: {}", self.reference(), problems.join("; ")))
        }
    }

    /// The text for `input`, checked against the input schema first.
    pub fn render(&self, input: &Value) -> Result<String, String> {
        let errors = schema::validate(&self.input, input);
        if !errors.is_empty() {
            return Err(format!(
                "{}: the input does not fit: {}",
                self.reference(),
                errors.join("; ")
            ));
        }
        let mut text = self.template.clone();
        if let Some(fields) = input.as_object() {
            for (name, value) in fields {
                let shown = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                text = text.replace(&format!("{{{{{name}}}}}"), &shown);
            }
        }
        Ok(text)
    }

    /// The answer in `raw`, checked against the output schema: the
    /// validated answer, or the errors. A free-text prompt's answer is the
    /// text itself.
    pub fn validate(&self, raw: &str) -> Result<Value, Vec<String>> {
        let Some(out) = &self.output else {
            return Ok(Value::String(raw.trim().to_string()));
        };
        let value = crate::json_in(raw).map_err(|e| vec![e])?;
        let errors = schema::validate(out, &value);
        if errors.is_empty() {
            Ok(value)
        } else {
            Err(errors)
        }
    }
}

fn check_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.split('.').all(|w| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        });
    if ok {
        Ok(())
    } else {
        Err(format!(
            "prompt id `{id}`: dotted lowercase words (`image.describe`)"
        ))
    }
}

/// A prompt by id, the latest version or a pinned one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRef {
    pub id: String,
    pub version: Option<u32>,
}

impl PromptRef {
    /// `id` or `id@version`.
    pub fn parse(text: &str) -> Result<PromptRef, String> {
        let (id, version) = match text.split_once('@') {
            Some((id, v)) => (
                id,
                Some(
                    v.parse::<u32>()
                        .map_err(|_| format!("`{text}`: the version is a number"))?,
                ),
            ),
            None => (text, None),
        };
        check_id(id)?;
        Ok(PromptRef {
            id: id.into(),
            version,
        })
    }
}

impl fmt::Display for PromptRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.version {
            Some(v) => write!(f, "{}@{v}", self.id),
            None => f.write_str(&self.id),
        }
    }
}

/// Every prompt version known: shipped and the user's.
#[derive(Clone, Debug, Default)]
pub struct Library {
    /// By id, then version.
    prompts: BTreeMap<String, BTreeMap<u32, Prompt>>,
    /// Where user versions are written; none for a read-only library.
    dir: Option<PathBuf>,
    /// Files that could not be read, with why.
    pub problems: Vec<String>,
}

fn parse(name: &str, text: &str) -> Result<Prompt, String> {
    let prompt: Prompt = if Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?
    } else {
        toml::from_str(text).map_err(|e| format!("{name}: {e}"))?
    };
    prompt.check()?;
    Ok(prompt)
}

impl Library {
    /// The shipped prompts, and the user's from `dir` when given.
    pub fn load(dir: Option<&Path>) -> Library {
        let mut library = Library {
            dir: dir.map(Path::to_path_buf),
            ..Library::default()
        };
        for (name, text) in SHIPPED {
            match parse(name, text) {
                Ok(mut p) => {
                    p.origin = Origin::Shipped;
                    library.insert(p);
                }
                Err(e) => library.problems.push(e),
            }
        }
        let Some(dir) = dir else {
            return library;
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| {
                    e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("toml")
                })
            })
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let read = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"));
            match read.and_then(|text| parse(&name, &text)) {
                Ok(mut p) => {
                    let taken = library
                        .prompts
                        .get(&p.id)
                        .and_then(|v| v.get(&p.version))
                        .is_some_and(|q| q.origin == Origin::Shipped);
                    if taken {
                        library.problems.push(format!(
                            "{name}: {} is a shipped version, which a user file cannot replace",
                            p.reference()
                        ));
                        continue;
                    }
                    p.origin = Origin::User;
                    library.insert(p);
                }
                Err(e) => library.problems.push(e),
            }
        }
        library
    }

    fn insert(&mut self, prompt: Prompt) {
        self.prompts
            .entry(prompt.id.clone())
            .or_default()
            .insert(prompt.version, prompt);
    }

    /// The prompt a reference names.
    pub fn get(&self, reference: &PromptRef) -> Result<&Prompt, String> {
        let versions = self
            .prompts
            .get(&reference.id)
            .ok_or_else(|| format!("no prompt `{}`", reference.id))?;
        match reference.version {
            Some(v) => versions
                .get(&v)
                .ok_or_else(|| format!("no version {v} of `{}`", reference.id)),
            None => Ok(versions
                .values()
                .next_back()
                .expect("an id has at least one version")),
        }
    }

    /// Every prompt, latest version of each.
    pub fn latest(&self) -> impl Iterator<Item = &Prompt> {
        self.prompts.values().filter_map(|v| v.values().next_back())
    }

    /// Every id with its versions, for listing.
    pub fn list(&self) -> Value {
        Value::Array(
            self.prompts
                .values()
                .filter_map(|versions| {
                    let latest = versions.values().next_back()?;
                    Some(json!({
                        "id": latest.id,
                        "capability": latest.capability,
                        "description": latest.description,
                        "latest": latest.version,
                        "versions": versions.values().map(|p| json!({
                            "version": p.version,
                            "origin": p.origin,
                            "tuned_for": p.tuned_for,
                            "hash": p.hash(),
                        })).collect::<Vec<_>>(),
                    }))
                })
                .collect(),
        )
    }

    /// Writes `prompt` as the next version of its id (its own `version` is
    /// ignored) and returns it as written.
    pub fn set(&mut self, mut prompt: Prompt) -> Result<Prompt, String> {
        let dir = self
            .dir
            .clone()
            .ok_or("this library has no directory to write to")?;
        prompt.version = self
            .prompts
            .get(&prompt.id)
            .and_then(|v| v.keys().next_back())
            .map_or(1, |v| v + 1);
        prompt.origin = Origin::User;
        prompt.check()?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{}.json", prompt.reference()));
        let text = serde_json::to_string_pretty(&prompt).expect("prompts serialize");
        let partial = path.with_extension("json.partial");
        std::fs::write(&partial, text)
            .and_then(|()| std::fs::rename(&partial, &path))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.insert(prompt.clone());
        Ok(prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_prompts_load_and_check() {
        let library = Library::load(None);
        assert!(library.problems.is_empty(), "{:?}", library.problems);
        assert_eq!(library.latest().count(), SHIPPED.len());
        for p in library.latest() {
            let example = p.example.clone().unwrap_or_else(|| json!({}));
            p.render(&example)
                .unwrap_or_else(|e| panic!("{}: its example does not fit: {e}", p.reference()));
        }
    }

    #[test]
    fn references_parse() {
        assert_eq!(
            PromptRef::parse("image.describe@2").unwrap(),
            PromptRef {
                id: "image.describe".into(),
                version: Some(2)
            }
        );
        assert_eq!(PromptRef::parse("image.describe").unwrap().version, None);
        assert!(PromptRef::parse("image.describe@x").is_err());
        assert!(PromptRef::parse("Image Describe").is_err());
    }

    #[test]
    fn slots_are_filled_from_the_input() {
        let p = Prompt {
            id: "t.x".into(),
            version: 1,
            capability: Capability::Chat,
            description: String::new(),
            tuned_for: Vec::new(),
            images: 0,
            system: String::new(),
            template: "Say {{word}} {{n}} times.".into(),
            input: json!({ "type": "object", "required": ["word"] }),
            output: None,
            example: None,
            temperature: 0.0,
            seed: None,
            origin: Origin::User,
        };
        assert_eq!(
            p.render(&json!({ "word": "hi", "n": 3 })).unwrap(),
            "Say hi 3 times."
        );
        assert!(
            p.render(&json!({}))
                .unwrap_err()
                .contains("`word` is missing")
        );
        assert_eq!(p.validate(" free text \n").unwrap(), json!("free text"));
    }
}
