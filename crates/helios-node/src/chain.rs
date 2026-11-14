//! `helios run`'s chains: several commands written as words or lines,
//! run in order as one `command.batch` (`docs/cli.md`, "Chains").
//!
//! A step is a command id and its params:
//!
//! - `library.import path=/photos recursive=true`: each `KEY=VALUE` sets
//!   one param, its value read as JSON (`2`, `true`, `[1,2]`), else as text;
//! - `export.run {"preset": "web"}`: a JSON object gives them all (later
//!   `KEY=VALUE` words are set over it).
//!
//! Written as one string (a script's line, or one shell word holding
//! spaces), a step is split into words as a shell would: `'…'` and `"…"`
//! keep spaces, `\` escapes the next character. A script holds one step
//! per line; blank lines and lines starting with `#` are skipped.
//!
//! Keys: a chain sent with key `K` gives step `n` (from 1) the key `K.n`,
//! so the same chain run again with the same key applies each step that
//! already ran once (`already_applied`) and runs only the ones that did
//! not: after an unknown outcome, or after a failed step was fixed.

use serde_json::{Map, Value, json};

/// The longest chain key: its steps' keys (`K.n`) stay within
/// [`helios_inspect::request_ids::LONGEST`].
pub const LONGEST_KEY: usize = helios_inspect::request_ids::LONGEST - 4;

/// One step: its command id and params.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: String,
    pub params: Map<String, Value>,
}

/// `text` split into words as a shell would (quotes and `\`).
///
/// # Errors
/// A quote left open.
pub fn words(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some(end) if end == c => break,
                        Some('\\') if c == '"' => match chars.next() {
                            Some(next) => word.push(next),
                            None => return Err(format!("`{text}`: a quote is left open")),
                        },
                        Some(other) => word.push(other),
                        None => return Err(format!("`{text}`: a quote is left open")),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// A step from its words: the id, then `KEY=VALUE` words or a JSON object.
///
/// # Errors
/// No id, a word that is neither `KEY=VALUE` nor a JSON object.
pub fn step(words: &[String]) -> Result<Step, String> {
    let (id, rest) = words.split_first().ok_or("a step without a command id")?;
    if id.is_empty() || id.contains('=') || id.starts_with('{') {
        return Err(format!("`{id}` is not a command id"));
    }
    let mut params = Map::new();
    for word in rest {
        if word.trim_start().starts_with('{') {
            match serde_json::from_str::<Value>(word) {
                Ok(Value::Object(fields)) => params.extend(fields),
                _ => return Err(format!("{id}: `{word}` is not a JSON object")),
            }
            continue;
        }
        let (key, text) = word
            .split_once('=')
            .filter(|(key, _)| !key.is_empty())
            .ok_or_else(|| format!("{id}: `{word}` is not KEY=VALUE"))?;
        let value = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
        params.insert(key.to_string(), value);
    }
    Ok(Step {
        id: id.clone(),
        params,
    })
}

/// A step written as one string: the id, then a JSON object (the rest of
/// the line) or `KEY=VALUE` words.
///
/// # Errors
/// As [`step`], and a quote left open.
pub fn step_line(line: &str) -> Result<Step, String> {
    let line = line.trim();
    if let Some((id, rest)) = line.split_once(char::is_whitespace)
        && rest.trim_start().starts_with('{')
    {
        return step(&[id.to_string(), rest.trim().to_string()]);
    }
    step(&words(line)?)
}

/// The steps of a script: one per line, blank lines and `#` comments
/// skipped.
///
/// # Errors
/// A line that is not a step, named by its number.
pub fn script(text: &str) -> Result<Vec<Step>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|(n, line)| step_line(line).map_err(|e| format!("line {}: {e}", n + 1)))
        .collect()
}

/// The steps of the command line's words, `--` between steps. A step
/// given as one word holding spaces is split as a script's line.
///
/// # Errors
/// An empty step, or one that is not a step.
pub fn from_words(groups: &[Vec<String>]) -> Result<Vec<Step>, String> {
    groups
        .iter()
        .enumerate()
        .map(|(n, group)| match group.as_slice() {
            [] => Err(format!("step {} is empty", n + 1)),
            [one] => step_line(one),
            many => step(many),
        })
        .collect()
}

/// The `command.batch` params of `steps`: the first failed step stops the
/// rest unless `keep_going`; step `n` keyed `{key}.{n}` when `key` is given.
pub fn batch(steps: &[Step], keep_going: bool, key: Option<&str>) -> Value {
    let steps: Vec<Value> = steps
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let mut step = json!({ "id": s.id, "params": s.params });
            if let Some(key) = key {
                step["request_id"] = json!(format!("{key}.{}", n + 1));
            }
            step
        })
        .collect();
    json!({ "steps": steps, "stop_on_error": !keep_going, "origin": "cli" })
}

/// One step's line in the human output: `ok`, `failed` or `skipped`, its
/// number and id, then the answer (cut) or the error.
pub fn human_line(n: usize, result: &Value) -> String {
    let id = result["id"].as_str().unwrap_or("?");
    if result["skipped"] == true {
        return format!("skipped {n} {id}");
    }
    if result["ok"] == true {
        let again = if result[helios_inspect::request_ids::ALREADY_APPLIED] == true {
            " (already applied)"
        } else {
            ""
        };
        let answer = result["answer"].clone();
        let mut text = serde_json::to_string(&answer).unwrap_or_default();
        if text.chars().count() > 160 {
            text = text.chars().take(159).collect::<String>() + "…";
        }
        return format!("ok      {n} {id}{again} {text}");
    }
    let message = result["error"]["message"].as_str().unwrap_or("failed");
    format!("failed  {n} {id}: {message}")
}

/// One step's line in the JSON output: its result with its number.
pub fn json_line(n: usize, result: &Value) -> String {
    let mut line = json!({ "step": n });
    if let Value::Object(fields) = result {
        line.as_object_mut()
            .expect("an object")
            .extend(fields.clone());
    }
    serde_json::to_string(&line).expect("JSON values serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn a_step_line_reads_its_params_as_json_or_text() {
        let step =
            step_line(r#"library.import path="/my photos" recursive=true depth=2 name=x"#).unwrap();
        assert_eq!(step.id, "library.import");
        assert_eq!(
            Value::Object(step.params),
            json!({ "path": "/my photos", "recursive": true, "depth": 2, "name": "x" })
        );
        let step = step_line(r#"export.run {"preset": "web", "to": "/out dir"}"#).unwrap();
        assert_eq!(
            Value::Object(step.params),
            json!({ "preset": "web", "to": "/out dir" })
        );
        assert!(step_line("x 'open").is_err());
        assert!(step_line("x notapair").is_err());
        assert!(step_line("x {bad").is_err());
    }

    #[test]
    fn a_script_skips_blank_lines_and_comments_and_names_a_bad_line() {
        let steps =
            script("# import then select\n\nlibrary.import path=/p\n  library.select.all\n")
                .unwrap();
        let ids: Vec<&str> = steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["library.import", "library.select.all"]);
        let error = script("a\nb nope\n").unwrap_err();
        assert!(error.starts_with("line 2:"), "{error}");
    }

    #[test]
    fn words_given_apart_are_taken_as_they_are() {
        let steps = from_words(&[
            s(&["library.import", "path=/my photos"]),
            s(&["library.select.all"]),
            s(&["export.run preset=web"]),
        ])
        .unwrap();
        assert_eq!(
            Value::Object(steps[0].params.clone()),
            json!({ "path": "/my photos" })
        );
        assert_eq!(steps[2].id, "export.run");
        assert_eq!(
            Value::Object(steps[2].params.clone()),
            json!({ "preset": "web" })
        );
        assert!(from_words(&[s(&["a"]), vec![]]).is_err());
    }

    #[test]
    fn each_step_of_a_keyed_chain_gets_its_own_key() {
        let steps = from_words(&[s(&["a"]), s(&["b", "x=1"])]).unwrap();
        let batch = batch(&steps, true, Some("k"));
        assert_eq!(batch["stop_on_error"], false);
        assert_eq!(batch["origin"], "cli");
        assert_eq!(batch["steps"][0]["request_id"], "k.1");
        assert_eq!(batch["steps"][1]["request_id"], "k.2");
        assert_eq!(batch["steps"][1]["params"], json!({ "x": 1 }));
    }
}
