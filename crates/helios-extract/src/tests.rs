use std::collections::BTreeMap;

use serde_json::json;

use super::*;

fn animals() -> ExtractorDef {
    builtins()
        .extractors
        .into_iter()
        .find(|d| d.id == "animals")
        .expect("the animals preset")
}

fn def_with(keys: Vec<KeyDecl>) -> ExtractorDef {
    ExtractorDef {
        id: "test".into(),
        version: 1,
        title: "Test".into(),
        description: None,
        keep: false,
        confidence_floor: 0.0,
        kind: Kind::Llm(LlmSpec {
            capability: Capability::Vision,
            prompt: Some("Describe {{keywords}}".into()),
            prompt_id: None,
            context: vec![ContextVar::Keywords],
            provider: None,
            model: None,
        }),
        keys,
    }
}

fn scene() -> KeyDecl {
    KeyDecl::new("scene", KeyType::Label).finite(&["beach", "city", "forest"])
}

// --- declarations ---

#[test]
fn builtins_are_sound_and_in_a_catalog() {
    let mut cat = Catalog::default();
    for d in builtins().extractors {
        assert!(
            d.problems().is_empty(),
            "{}: {:?}",
            d.pinned(),
            d.problems()
        );
        cat.add(d).unwrap();
    }
    let exif = cat.latest("exif").unwrap();
    assert_eq!(exif.key("iso").unwrap().ty, KeyType::Integer);
    assert!(matches!(exif.kind, Kind::Builtin(ref b) if b.compute == "capture"));
    let u = ExtractorUse::of(&animals());
    assert_eq!(cat.resolve(&u).unwrap().id, "animals");
    let missing = ExtractorUse { version: 9, ..u };
    assert_eq!(
        cat.resolve(&missing),
        Err(DeclError::Unknown("animals@9".into()))
    );
    assert_eq!(
        cat.add(animals()),
        Err(DeclError::Duplicate("animals@1".into()))
    );
}

#[test]
fn bad_declarations_are_refused_with_their_reason() {
    let d = def_with(vec![
        KeyDecl::new("Bad name", KeyType::Text),
        KeyDecl::new("n", KeyType::Number).finite(&["1"]),
        KeyDecl::new("x", KeyType::Label).finite(&["a", "a", "none"]),
        KeyDecl::new("o", KeyType::Label).with_other(),
        KeyDecl::new("c", KeyType::Number).with_confidence(),
        KeyDecl::new("r", KeyType::Number).range(Some(2.0), Some(1.0)),
        KeyDecl::new("r", KeyType::Bool),
    ]);
    let p = d.problems().join("\n");
    for want in [
        "key `Bad name`: a key name",
        "key `n`: only text and label keys list their values",
        "key `x`: `a` is listed twice",
        "key `x`: `none` is reserved",
        "key `o`: `other` is for finite keys",
        "key `c`: a confidence is for label keys",
        "key `r`: min 2 is above max 1",
        "key `r` is declared twice",
    ] {
        assert!(p.contains(want), "missing `{want}` in:\n{p}");
    }

    let mut d = def_with(vec![scene()]);
    if let Kind::Llm(llm) = &mut d.kind {
        llm.prompt = Some("Where? {{folder}}".into());
    }
    assert_eq!(
        d.problems(),
        vec!["the prompt uses `{{folder}}`, not in its context".to_owned()]
    );

    let text = "[[extractors]]\nid = \"x\"\nversion = 1\ntitle = \"X\"\nbuiltin = { compute = \"c\" }\nkeys = []\n";
    assert_eq!(
        Manifest::from_toml(text),
        Err(DeclError::Invalid(
            "x@1".into(),
            vec!["an extractor declares at least one key".into()]
        ))
    );
    let typo = "[[extractors]]\nid = \"x\"\nversion = 1\ntitle = \"X\"\nbuiltin = { compute = \"c\" }\n[[extractors.keys]]\nname = \"k\"\ntype = \"label\"\nmulti_valued = true\n";
    assert!(matches!(
        Manifest::from_toml(typo),
        Err(DeclError::Parse(_))
    ));
}

#[test]
fn manifest_round_trips_through_toml_and_json() {
    let mut m = builtins();
    let mut semi = def_with(vec![
        KeyDecl::new("mood", KeyType::Label)
            .semi_finite(&["calm", "busy"])
            .multi(),
        KeyDecl::new("caption", KeyType::Text),
    ]);
    semi.confidence_floor = 0.25;
    semi.keep = true;
    m.extractors.push(semi);

    let toml = m.to_toml();
    assert_eq!(Manifest::from_toml(&toml).unwrap(), m, "{toml}");
    let json = serde_json::to_string(&m).unwrap();
    assert_eq!(Manifest::from_json(&json).unwrap(), m, "{json}");

    // The documented shape: finite list, semi-finite flag, kind table.
    assert!(toml.contains("semi_finite = true"), "{toml}");
    assert!(toml.contains("[extractors.llm]"), "{toml}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["extractors"][0]["builtin"]["compute"], "capture");

    let u = ExtractorUse {
        def: "animals".into(),
        version: 1,
        params: BTreeMap::from([("language".into(), json!("fr"))]),
    };
    let back: ExtractorUse = toml::from_str(&toml::to_string(&u).unwrap()).unwrap();
    assert_eq!(back, u);
}

// --- schema derivation ---

#[test]
fn finite_single_key_is_an_enum_with_none() {
    let d = def_with(vec![scene()]);
    let s = answer_schema(&d);
    assert_eq!(
        s,
        json!({
            "type": "object",
            "required": ["scene"],
            "additionalProperties": false,
            "properties": {
                "scene": { "type": "string", "enum": ["beach", "city", "forest", "none"] }
            }
        })
    );
}

#[test]
fn finite_multi_key_with_confidence_and_other() {
    let s = answer_schema(&animals());
    assert_eq!(
        s["properties"]["animal"],
        json!({
            "type": "array",
            "maxItems": 30,
            "items": {
                "type": "object",
                "required": ["value", "p"],
                "additionalProperties": false,
                "properties": {
                    "value": { "type": "string", "enum": ["dog", "cat", "bird", "horse", "other"] },
                    "p": { "type": "number", "minimum": 0, "maximum": 1 }
                }
            }
        })
    );
}

#[test]
fn open_and_typed_keys() {
    let d = def_with(vec![
        KeyDecl::new("caption", KeyType::Text),
        KeyDecl::new("tags", KeyType::Label).multi(),
        KeyDecl::new("score", KeyType::Integer).range(Some(1.0), Some(5.0)),
        KeyDecl::new("people", KeyType::Bool),
    ]);
    let p = &answer_schema(&d)["properties"];
    assert_eq!(
        p["caption"],
        json!({ "type": ["string", "null"], "minLength": 1, "maxLength": 300 })
    );
    assert_eq!(
        p["tags"],
        json!({ "type": "array", "maxItems": 30, "items": { "type": "string", "minLength": 1, "maxLength": 100 } })
    );
    assert_eq!(
        p["score"],
        json!({ "type": ["integer", "null"], "minimum": 1.0, "maximum": 5.0 })
    );
    assert_eq!(p["people"], json!({ "type": ["boolean", "null"] }));
}

#[test]
fn derived_schemas_stay_in_helios_ai_subset() {
    let mut defs = builtins().extractors;
    defs.push(def_with(vec![
        scene(),
        KeyDecl::new("mood", KeyType::Label)
            .semi_finite(&["calm"])
            .with_confidence(),
        KeyDecl::new("caption", KeyType::Text),
    ]));
    for d in &defs {
        let s = answer_schema(d);
        assert!(helios_ai::schema::lint(&s).is_empty(), "{}", d.pinned());
    }
    // A fitting answer passes both helios-ai's generic check and ours.
    let d = defs.last().unwrap();
    let good = json!({ "scene": "beach", "mood": { "value": "calm", "p": 0.8 }, "caption": null });
    assert!(helios_ai::schema::validate(&answer_schema(d), &good).is_empty());
    assert!(validate(d, &good).is_ok());
}

#[test]
fn key_instructions_name_every_key() {
    let text = key_instructions(&animals());
    assert!(
        text.contains("`animal`: any of dog, cat, bird, horse, or other; several allowed"),
        "{text}"
    );
    assert!(text.contains("\"p\": confidence between 0 and 1"), "{text}");
}

// --- validation ---

#[test]
fn a_good_answer_gives_typed_values() {
    let d = def_with(vec![
        scene(),
        KeyDecl::new("caption", KeyType::Text),
        KeyDecl::new("score", KeyType::Integer).range(Some(1.0), Some(5.0)),
        KeyDecl::new("people", KeyType::Bool),
        KeyDecl::new("tags", KeyType::Label).multi(),
    ]);
    let a = check_reply(
        &d,
        "Sure!\n```json\n{\"scene\": \"Beach\", \"caption\": \"A dog runs.\", \"score\": 4.0, \"people\": false, \"tags\": [\"dog\", \"sea\", \"dog\"]}\n```",
    )
    .unwrap();
    assert_eq!(
        a.get("scene"),
        [Value::Label {
            value: "beach".into(),
            p: None
        }]
    );
    assert_eq!(a.get("caption")[0].as_str(), Some("A dog runs."));
    assert_eq!(a.get("score"), [Value::Integer { value: 4 }]);
    assert_eq!(a.get("people"), [Value::Bool { value: false }]);
    assert_eq!(a.get("tags").len(), 2, "duplicates merged");
    assert!(a.adjustments.is_empty());

    let none = validate(
        &d,
        &json!({ "scene": "none", "caption": null, "score": null, "people": null, "tags": [] }),
    )
    .unwrap();
    assert!(none.values.values().all(Vec::is_empty));
}

#[test]
fn out_of_list_values_are_dropped_or_mapped_and_counted() {
    let mut d = animals();
    d.confidence_floor = 0.5;
    let a = validate(
        &d,
        &json!({ "animal": [
            { "value": "dog", "p": 0.94 },
            { "value": "goat", "p": 0.8 },
            { "value": "cat", "p": 0.3 }
        ] }),
    )
    .unwrap();
    assert_eq!(
        a.get("animal"),
        [
            Value::Label {
                value: "dog".into(),
                p: Some(0.94)
            },
            Value::Label {
                value: "other".into(),
                p: Some(0.8)
            },
        ]
    );
    assert_eq!(a.count(Adjust::MappedToOther), 1);
    assert_eq!(a.count(Adjust::BelowFloor), 1);

    // Without `other`, the value is dropped.
    let d = def_with(vec![scene().multi()]);
    let a = validate(&d, &json!({ "scene": ["city", "desert", "moon"] })).unwrap();
    assert_eq!(a.get("scene").len(), 1);
    assert_eq!(a.count(Adjust::Dropped), 2);
    assert_eq!(a.adjustments[0].value, "desert");

    // Semi-finite: kept, counted as new.
    let d = def_with(vec![
        KeyDecl::new("mood", KeyType::Label)
            .semi_finite(&["calm"])
            .multi(),
    ]);
    let a = validate(&d, &json!({ "mood": ["calm", "stormy"] })).unwrap();
    assert_eq!(a.get("mood").len(), 2);
    assert_eq!(a.count(Adjust::New), 1);
}

#[test]
fn wrong_types_are_errors_named_by_path() {
    let d = def_with(vec![
        KeyDecl::new("score", KeyType::Integer).range(Some(1.0), Some(5.0)),
        KeyDecl::new("people", KeyType::Bool),
        KeyDecl::new("tags", KeyType::Label).multi(),
    ]);
    let errors = validate(
        &d,
        &json!({ "score": 9, "people": "yes", "tags": "dog", "extra": 1 }),
    )
    .unwrap_err();
    assert_eq!(
        errors,
        vec![
            "$: `extra` is not expected",
            "$.score: 9 is above 5",
            "$.people: expected boolean, got string",
            "$.tags: expected array, got string",
        ]
    );
    let e = validate(
        &animals(),
        &json!({ "animal": [{ "value": "dog", "p": 2 }, "cat"] }),
    )
    .unwrap_err();
    assert_eq!(
        e,
        vec![
            "$.animal[0].p: 2 is outside 0 to 1",
            "$.animal[1]: expected an object {value, p}, got string",
        ]
    );
    assert_eq!(
        validate(&animals(), &json!({})).unwrap_err(),
        vec!["$: `animal` is missing"]
    );
    assert_eq!(
        check_reply(&animals(), "I cannot tell.").unwrap_err(),
        vec!["$: the reply holds no JSON object"]
    );
    let note = retry_note(&e);
    assert!(note.starts_with("Your answer did not fit:\n- $.animal[0].p"));
}

// --- cache key ---

#[test]
fn cache_key_is_stable_and_tracks_what_changes_the_answer() {
    // Declared here, not from the presets, so the golden key below only
    // moves with the key layout.
    let d = def_with(vec![scene()]);
    let u = ExtractorUse {
        def: "test".into(),
        version: 1,
        params: BTreeMap::from([("language".into(), json!("en")), ("n".into(), json!(2))]),
    };
    let inputs = BTreeMap::from([("keywords".to_owned(), "Kenya".to_owned())]);
    let base = CacheInput::new("img-1", &d, &u, "ollama/qwen2.5vl:7b", &inputs);
    let key = base.key();
    assert_eq!(key.len(), 64);
    // Pinned: a change here empties every user's cache, so it bumps
    // `cache::SCHEME` on purpose.
    assert_eq!(key, GOLDEN_KEY);

    // Parameter order does not matter.
    let mut reordered = serde_json::Map::new();
    reordered.insert("n".into(), json!(2));
    reordered.insert("language".into(), json!("en"));
    let u2 = ExtractorUse {
        params: reordered.into_iter().collect(),
        ..u.clone()
    };
    assert_eq!(
        CacheInput::new("img-1", &d, &u2, "ollama/qwen2.5vl:7b", &inputs).key(),
        key
    );

    // A new title is not a new answer; a new prompt, model, version,
    // parameter, input or image source is.
    let mut retitled = d.clone();
    retitled.title = "Animals".into();
    assert_eq!(
        CacheInput {
            def: &retitled,
            ..base.clone()
        }
        .key(),
        key
    );
    let mut reworded = d.clone();
    if let Kind::Llm(l) = &mut reworded.kind {
        l.prompt = Some("Which animals do you see?".into());
    }
    let mut bumped = d.clone();
    bumped.version = 2;
    let fr = ExtractorUse {
        params: BTreeMap::from([("language".into(), json!("fr"))]),
        ..u.clone()
    };
    let other_inputs = BTreeMap::new();
    let changed = [
        CacheInput {
            def: &reworded,
            ..base.clone()
        }
        .key(),
        CacheInput {
            def: &bumped,
            ..base.clone()
        }
        .key(),
        CacheInput {
            model: "openai/gpt-4o",
            ..base.clone()
        }
        .key(),
        CacheInput {
            params: &fr.params,
            ..base.clone()
        }
        .key(),
        CacheInput {
            inputs: &other_inputs,
            ..base.clone()
        }
        .key(),
        CacheInput {
            image_version: Some("v2"),
            ..base.clone()
        }
        .key(),
        CacheInput {
            image: "img-2",
            ..base.clone()
        }
        .key(),
    ];
    for (i, k) in changed.iter().enumerate() {
        assert_ne!(*k, key, "change {i} kept the key");
    }
}

const GOLDEN_KEY: &str = "26d1a0f8f3761837c8d0d48d8dccdf91598d9682e39b47f3c0e3200193aa3b2c";
