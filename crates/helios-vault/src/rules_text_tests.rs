use serde_json::{Value, json};

use super::*;
use crate::rules::{MAX_CONDITIONS, RULE_FIELDS, RuleField};

fn all(rules: Vec<Rule>) -> Rule {
    Rule::group(Match::All, rules)
}

fn any(rules: Vec<Rule>) -> Rule {
    Rule::group(Match::Any, rules)
}

fn none(rules: Vec<Rule>) -> Rule {
    Rule::group(Match::None, rules)
}

fn c(field: &str, op: &str, value: Value) -> Rule {
    Rule::condition(field, op, value).unwrap()
}

fn parsed(text: &str) -> Rule {
    parse_text(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn day(t: &str) -> i64 {
    crate::capture::day_start(t).unwrap()
}

/// Today's search texts (egui-pop's keys and Helios's own) read as the
/// terms the Library always ran.
#[test]
fn today_s_terms_read_as_they_always_did() {
    let table: Vec<(&str, Rule)> = vec![
        ("", all(vec![])),
        ("rating>=3", all(vec![c("rating", ">=", json!(3))])),
        ("rating:3", all(vec![c("rating", ">=", json!(3))])),
        ("stars:4", all(vec![c("rating", ">=", json!(4))])),
        ("rating:9", all(vec![c("rating", ">=", json!(5))])),
        ("label:RED", all(vec![c("label", "has", json!("red"))])),
        (
            "label:red label:blue",
            all(vec![any(vec![
                c("label", "has", json!("red")),
                c("label", "has", json!("blue")),
            ])]),
        ),
        ("flag:pick", all(vec![c("flag", "is", json!("pick"))])),
        (
            "tag:sea #sun",
            all(vec![
                c("tag", "has", json!("sea")),
                c("tag", "has", json!("sun")),
            ]),
        ),
        (
            "date:2024-06-01..2024-06-30",
            all(vec![c(
                "captured",
                "between",
                json!([day("2024-06-01"), day("2024-07-01") - 1]),
            )]),
        ),
        (
            "date:2024-06-01..",
            all(vec![c("captured", ">=", json!(day("2024-06-01")))]),
        ),
        (
            "date:..2024-06-30",
            all(vec![c("captured", "<", json!(day("2024-07-01")))]),
        ),
        (
            "from:2024-06-01 to:2024-06-30",
            all(vec![
                c("captured", ">=", json!(day("2024-06-01"))),
                c("captured", "<", json!(day("2024-07-01"))),
            ]),
        ),
        ("camera:\"Z 8\"", all(vec![c("camera", "is", json!("Z 8"))])),
        (
            "camera:Z8 camera:R5",
            all(vec![any(vec![
                c("camera", "is", json!("Z8")),
                c("camera", "is", json!("R5")),
            ])]),
        ),
        ("lens:L1", all(vec![c("lens", "is", json!("L1"))])),
        ("type:CR3", all(vec![c("file_type", "is", json!("cr3"))])),
        (
            "folder:\"/photos/2024 trip\"",
            all(vec![c("folder", "in", json!("/photos/2024 trip"))]),
        ),
        ("collection:abc", all(vec![c("album", "in", json!("abc"))])),
        ("import:s1", all(vec![c("import", "in", json!("s1"))])),
        (
            "keyword:Animals|Dog",
            all(vec![c("keyword", "has", json!("Animals|Dog"))]),
        ),
        (
            "presence:offline",
            all(vec![c("presence", "is", json!("offline"))]),
        ),
        ("cull:sharp", all(vec![c("cull", "is", json!("sharp"))])),
        ("backup:ok", all(vec![c("backup", "is", json!("met"))])),
        (
            "backup:short backup:none",
            all(vec![
                c("backup", "is", json!("short")),
                c("backup", "is", json!("none")),
            ]),
        ),
        (
            "beach IMG",
            all(vec![
                c("text", "contains", json!("beach")),
                c("text", "contains", json!("IMG")),
            ]),
        ),
        // An unknown key or a bad value stays a word, as it was typed.
        (
            "colour:red",
            all(vec![c("text", "contains", json!("colour:red"))]),
        ),
        (
            "label:pink",
            all(vec![c("text", "contains", json!("label:pink"))]),
        ),
        (
            "presence:gone",
            all(vec![c("text", "contains", json!("presence:gone"))]),
        ),
    ];
    for (text, want) in table {
        assert_eq!(parsed(text), want, "{text}");
    }
}

#[test]
fn negation_or_and_ranges_read_into_the_tree() {
    assert_eq!(
        parsed("-flag:reject -#draft -rating:>=3 -beach"),
        all(vec![
            c("flag", "is_not", json!("reject")),
            c("tag", "lacks", json!("draft")),
            c("rating", "<", json!(3)),
            none(vec![c("text", "contains", json!("beach"))]),
        ])
    );
    assert_eq!(
        parsed("(label:red OR #beach) -(flag:reject OR cull:soft)"),
        all(vec![
            any(vec![
                c("label", "has", json!("red")),
                c("tag", "has", json!("beach"))
            ]),
            none(vec![
                c("flag", "is", json!("reject")),
                c("cull", "is", json!("soft")),
            ]),
        ])
    );
    assert_eq!(
        parsed("label:red OR label:blue rating:>=4"),
        all(vec![any(vec![
            c("label", "has", json!("red")),
            all(vec![
                c("label", "has", json!("blue")),
                c("rating", ">=", json!(4))
            ]),
        ])]),
        "OR at the top: any of the sides"
    );
    assert_eq!(
        parsed("+label:red +label:blue"),
        all(vec![
            c("label", "has", json!("red")),
            c("label", "has", json!("blue"))
        ]),
        "+ keeps both"
    );
    assert_eq!(
        parsed("iso:<=800 f:1.4..2.8 shutter:<1/250 focal:50"),
        all(vec![
            c("iso", "<=", json!(800)),
            c("aperture", "between", json!([1.4, 2.8])),
            c("shutter", "<", json!(0.004)),
            c("focal_length", "=", json!(50)),
        ])
    );
    assert_eq!(
        parsed("captured:2024-06 captured:>2023 imported:last:30d captured:this-year"),
        all(vec![
            c(
                "captured",
                "between",
                json!([day("2024-06-01"), day("2024-07-01") - 1])
            ),
            c("captured", ">=", json!(day("2024-01-01"))),
            c("imported", "within", json!({"last": 30, "unit": "days"})),
            c("captured", "within", json!("this_year")),
        ])
    );
    assert_eq!(
        parsed("camera:~Z title:sunset title:=Dawn -camera:R5"),
        all(vec![
            c("camera", "contains", json!("Z")),
            c("title", "contains", json!("sunset")),
            c("title", "is", json!("Dawn")),
            c("camera", "is_not", json!("R5")),
        ])
    );
}

#[test]
fn faults_are_errors_with_where_they_are() {
    let e = parse_text("rating:3 (label:red").unwrap_err();
    assert_eq!(e.at, 9, "{e}");
    let e = parse_text("label:red)").unwrap_err();
    assert_eq!(e.at, 9, "{e}");
    let deep = "(".repeat(MAX_DEPTH + 1) + &")".repeat(MAX_DEPTH + 1);
    assert!(
        parse_text(&deep)
            .unwrap_err()
            .message
            .contains("nest at most")
    );
    let many = vec!["#a"; MAX_CONDITIONS + 1].join(" ");
    assert!(
        parse_text(&many)
            .unwrap_err()
            .message
            .contains("conditions")
    );
    // An unclosed quote runs to the end, as typing it does.
    assert_eq!(
        parsed("camera:\"Z 8"),
        all(vec![c("camera", "is", json!("Z 8"))])
    );
}

#[test]
fn trees_are_written_as_people_type_them() {
    let tree = all(vec![
        c("rating", ">=", json!(3)),
        any(vec![
            c("label", "has", json!("red")),
            c("label", "has", json!("blue")),
        ]),
        c("camera", "is", json!("Z 8")),
        c("flag", "is_not", json!("reject")),
        c("tag", "has", json!("beach")),
        c(
            "captured",
            "between",
            json!([day("2024-06-01"), day("2024-07-01") - 1]),
        ),
        c("text", "contains", json!("sunset")),
    ]);
    let text = to_text(&tree);
    assert_eq!(
        text,
        "rating:>=3 label:red label:blue camera:\"Z 8\" -flag:reject #beach \
         captured:2024-06-01..2024-06-30 sunset"
    );
    assert_eq!(parsed(&text), tree);
    // Both labels, not either.
    let both = all(vec![
        c("label", "has", json!("red")),
        c("label", "has", json!("blue")),
    ]);
    assert_eq!(to_text(&both), "+label:red +label:blue");
    assert_eq!(parsed(&to_text(&both)), both);
    // A top that is not `all` reads back inside one.
    let top = any(vec![
        c("rating", ">=", json!(4)),
        c("flag", "is", json!("pick")),
    ]);
    assert_eq!(to_text(&top), "(rating:>=4 OR flag:pick)");
    assert_eq!(parsed(&to_text(&top)), all(vec![top]));
}

// --- The round trip on generated trees -------------------------------------

/// A small deterministic generator (xorshift), so a failing tree is the
/// same at every run.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    fn text(&mut self) -> String {
        const PIECES: &[&str] = &[
            "a", "Z", "8", "é", " ", "\"", "(", ")", ":", "~", "-", "#", "OR", ">=", "\\", "..",
            "|", "/", "@", "+", "x y",
        ];
        loop {
            let n = 1 + self.below(4);
            let t: String = (0..n).map(|_| *self.pick(PIECES)).collect();
            let t = t.trim().to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }

    fn time(&mut self) -> i64 {
        let day = 10_000 + self.below(20_000) as i64;
        match self.below(3) {
            0 => day * 86_400,
            1 => (day + 1) * 86_400 - 1,
            _ => day * 86_400 + self.below(86_400) as i64,
        }
    }

    fn number(&mut self, spec: &RuleField) -> Value {
        if spec.name == "rating" {
            return if self.below(4) == 0 {
                json!(self.below(11) as f64 / 2.0)
            } else {
                json!(self.below(6))
            };
        }
        match self.below(3) {
            0 => json!(self.below(5000) as f64 / 8.0),
            1 => json!(-(self.below(100) as i64)),
            _ => json!(self.below(5000)),
        }
    }

    fn condition(&mut self) -> Rule {
        loop {
            let spec = self.pick(RULE_FIELDS);
            let op = *self.pick(spec.kind.ops());
            let value = match (spec.kind, op) {
                (RuleKind::Number, "between") => {
                    let (a, b) = (self.number(spec), self.number(spec));
                    json!([a, b])
                }
                (RuleKind::Number, _) => self.number(spec),
                (RuleKind::Date | RuleKind::Imported, "within") => match self.below(3) {
                    0 => json!("today"),
                    1 => json!("this_year"),
                    _ => json!({
                        "last": 1 + self.below(400),
                        "unit": self.pick(&["days", "weeks", "months"]),
                    }),
                },
                (RuleKind::Date | RuleKind::Imported, "between") => {
                    let a = self.time();
                    json!([a, a + self.below(400 * 86_400) as i64])
                }
                (RuleKind::Date | RuleKind::Imported, _) => json!(self.time()),
                (RuleKind::Set, _) if spec.name == "label" => {
                    json!(self.pick(&["red", "yellow", "green", "blue", "purple"]))
                }
                (RuleKind::Folder, _) => json!(format!("/{}", self.text())),
                // The text writes file types in lower case, as typed.
                (RuleKind::Text, _) if spec.name == "file_type" => {
                    json!(self.text().to_lowercase())
                }
                _ => match spec.kind.values() {
                    Some(values) => json!(self.pick(values)),
                    None => json!(self.text()),
                },
            };
            // A value the field refuses (a range upside down) is drawn again.
            if let Ok(rule) = Rule::condition(spec.name, op, value) {
                return rule;
            }
        }
    }

    fn rule(&mut self, depth: usize) -> Rule {
        if depth == 0 || self.below(3) > 0 {
            return self.condition();
        }
        let mode = *self.pick(&Match::ALL);
        let n = self.below(4);
        let rules = (0..n).map(|_| self.rule(depth - 1)).collect();
        Rule::group(mode, rules)
    }

    /// A tree, an `all` group at its top, where same-key terms are made
    /// likely (they are what the grammar's merging is about).
    fn tree(&mut self) -> Rule {
        let n = self.below(6);
        let mut rules: Vec<Rule> = (0..n).map(|_| self.rule(3)).collect();
        if self.below(2) == 0 {
            let label = |l: &str| c("label", "has", json!(l));
            match self.below(3) {
                0 => rules.push(any(vec![label("red"), label("blue")])),
                1 => rules.extend([label("red"), label("green")]),
                _ => rules.extend([label("red"), any(vec![label("blue"), label("green")])]),
            }
        }
        all(rules)
    }
}

/// Every tree, every field and operator: written then read, it is the
/// same tree.
#[test]
fn generated_trees_read_back_as_written() {
    let mut g = Gen(0x5078_6a3c_91d2_4e11);
    let mut seen_ops = std::collections::BTreeSet::<(&str, &str)>::default();
    for _ in 0..4000 {
        let tree = g.tree();
        let text = to_text(&tree);
        let back = parse_text(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(back, tree, "{text}");
        assert_eq!(to_text(&back), text);
        let mut walk = vec![&tree];
        while let Some(r) = walk.pop() {
            match r {
                Rule::Condition(c) => {
                    seen_ops.insert((c.field.name, c.op));
                }
                Rule::Group { rules, .. } => walk.extend(rules),
            }
        }
    }
    for f in RULE_FIELDS {
        for op in f.kind.ops() {
            assert!(
                seen_ops.contains(&(f.name, *op)),
                "{} {op} never drawn",
                f.name
            );
        }
    }
}
