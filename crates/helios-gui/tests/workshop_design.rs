//! A workshop designed from the user's words (experimental, see
//! `docs/design/workshops-on-the-fly.md`, slice 4), end to end and
//! headless: the user describes what they want in the Workshops panel; a
//! scripted agent, speaking to the desktop as the node's MCP server does,
//! reads the context and posts a draft; Helios checks it on the open
//! image, applies it as one undo step, and the panel shows only the
//! sliders the check kept.
//!
//! The owner's example: a backlit photo, "the mountain and the sky too
//! contrasted". Helios does not port darktable's tone equalizer, shadows
//! and highlights or local contrast yet, so the test adds stand-ins of
//! those names (a gain by luminance each) for the draft to use.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, ModuleRef, Needs, ParamSpec, Params,
    PortSpec, Region, Role,
};
use helios_gui::{Experimental, Shell, headless, helios_panels_with, templates};
use helios_shell::commands::Origin;
use helios_template::Source;
use serde_json::{Value as Json, json};

/// A module moving each pixel by a gain of its luminance: a stand-in.
struct StandIn {
    name: &'static str,
    params: fn() -> Vec<ParamSpec>,
    /// The gain, from the parameters (in their order) and the luminance.
    gain: fn(&[f64], f32) -> f32,
}

impl Module for StandIn {
    type Plan = Vec<f64>;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: self.name.into(),
            version: 1,
            description: format!("a test's stand-in for darktable's {}", self.name),
            params: (self.params)(),
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<Vec<f64>, String> {
        Ok((self.params)()
            .iter()
            .map(|p| params.float(&p.name))
            .collect())
    }

    fn process(
        &self,
        plan: &Vec<f64>,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0].sample(output);
        let mut data = input.to_vec();
        for p in data.chunks_exact_mut(4) {
            let lum = (p[0] + p[1] + p[2]) / 3.0;
            let g = (self.gain)(plan, lum);
            for v in &mut p[..3] {
                *v *= g;
            }
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

/// darktable's tone equalizer bands, from -8 EV to 0 EV.
const BANDS: [&str; 9] = [
    "noise",
    "ultra_deep_blacks",
    "deep_blacks",
    "blacks",
    "shadows",
    "midtones",
    "highlights",
    "whites",
    "speculars",
];

fn toneequal() -> StandIn {
    StandIn {
        name: "toneequal",
        params: || {
            BANDS
                .iter()
                .map(|b| ParamSpec::float(b, "the band's correction, in EV", -2.0, 2.0, 0.0))
                .collect()
        },
        gain: |bands, lum| {
            let s = f64::from(lum.max(1e-6)).log2().clamp(-8.0, 0.0) + 8.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let i = (s.floor() as usize).min(7);
            let t = s - i as f64;
            let ev = bands[i] * (1.0 - t) + bands[i + 1] * t;
            ev.exp2() as f32
        },
    }
}

fn shadhi() -> StandIn {
    StandIn {
        name: "shadhi",
        params: || {
            vec![
                ParamSpec::float("shadows", "lift the shadows", -100.0, 100.0, 0.0),
                ParamSpec::float("highlights", "hold the highlights", -100.0, 100.0, 0.0),
            ]
        },
        gain: |p, lum| {
            let lum = f64::from(lum);
            let shadows = (1.0 - lum / 0.5).clamp(0.0, 1.0).powi(2);
            let highs = ((lum - 0.5) / 0.5).clamp(0.0, 1.0).powi(2);
            (1.5 * (p[0] / 100.0 * shadows + p[1] / 100.0 * highs)).exp2() as f32
        },
    }
}

fn bilat() -> StandIn {
    StandIn {
        name: "bilat",
        params: || {
            vec![ParamSpec::float(
                "detail",
                "local contrast's detail",
                0.0,
                5.0,
                1.0,
            )]
        },
        gain: |p, lum| {
            let lum = f64::from(lum.max(1e-6));
            let e = 1.0 + 0.25 * (p[0] - 1.0);
            (0.18 * (lum / 0.18).powf(e) / lum) as f32
        },
    }
}

fn stand_ins() {
    for m in [toneequal(), shadhi(), bilat()] {
        templates::add_module(ModuleRef::new(m));
    }
}

const INTENT: &str = "backlit, the mountain and the sky too contrasted";

/// The scripted agent's draft: toneequal and shadhi lift the backlit
/// foreground, an exposure through a sky mask holds the sky, local
/// contrast brings the mountain's detail; three semantic sliders, and a
/// fourth, "Haze", that moves nothing on this photo.
fn backlit_draft() -> Json {
    json!({
        "intent": INTENT,
        "composite": {
            "name": "Backlit mountain",
            "description": "Lift a backlit foreground, hold the sky, bring out the mountain",
            "nodes": [
                { "name": "tone", "module": "toneequal" },
                { "name": "shadows", "module": "shadhi" },
                { "name": "detail", "module": "bilat" },
                { "name": "sky_mask", "module": "mask_parametric",
                  "params": { "luminance": { "list": [0.3, 4.0, 0.05, 0.0] } } },
                { "name": "sky_exp", "module": "exposure" },
                { "name": "blend", "module": "mask_blend" }
            ],
            "connections": [
                { "from": "@", "port": "input", "to": "tone", "input": "input" },
                { "from": "@", "port": "input", "to": "sky_mask", "input": "input" },
                { "from": "tone", "port": "output", "to": "shadows", "input": "input" },
                { "from": "shadows", "port": "output", "to": "detail", "input": "input" },
                { "from": "detail", "port": "output", "to": "sky_exp", "input": "input" },
                { "from": "detail", "port": "output", "to": "blend", "input": "input" },
                { "from": "sky_exp", "port": "output", "to": "blend", "input": "processed" },
                { "from": "sky_mask", "port": "mask", "to": "blend", "input": "mask" }
            ],
            "exits": [ { "from": "blend", "port": "output", "output": "output" } ],
            "exposed": [
                { "name": "backlight_lift", "label": "Backlight lift", "min": 0.0, "max": 1.0,
                  "drives": [
                      { "node": "tone", "param": "blacks", "curve": [[0.0, 0.0], [1.0, 1.5]] },
                      { "node": "tone", "param": "shadows", "curve": [[0.0, 0.0], [1.0, 1.2]] },
                      { "node": "shadows", "param": "shadows", "curve": [[0.0, 0.0], [0.5, 10.0], [1.0, 30.0]] }
                  ] },
                { "name": "sky", "label": "Sky", "min": -1.0, "max": 1.0,
                  "drives": [
                      { "node": "sky_exp", "param": "exposure", "curve": [[-1.0, -1.0], [0.0, 0.0], [1.0, 0.5]] }
                  ] },
                { "name": "mountain_detail", "label": "Mountain detail", "min": 0.0, "max": 1.0,
                  "drives": [
                      { "node": "detail", "param": "detail", "curve": [[0.0, 1.0], [1.0, 2.0]] }
                  ] },
                { "name": "haze", "label": "Haze", "min": 0.0, "max": 1.0,
                  "drives": [
                      { "node": "tone", "param": "speculars", "curve": [[0.0, 0.0], [1.0, 0.0]] }
                  ] }
            ],
            "workshop": {
                "keywords": ["backlit", "mountain", "sky"],
                "zones": [
                    { "name": "sky", "module": "mask_parametric",
                      "params": { "luminance": { "list": [0.3, 4.0, 0.0, 0.0] } } },
                    { "name": "foreground", "module": "mask_parametric",
                      "params": { "luminance": { "list": [0.0, 0.2, 0.0, 0.0] } } }
                ],
                "targets": [
                    { "param": "backlight_lift", "measure": "zones.ratio", "value": -3.0,
                      "zone": "foreground", "against": "sky" }
                ]
            }
        },
        "rationale": {
            "workshop": "The foreground sits about four stops under the sky: lift it, hold the sky",
            "nodes": { "tone": "Lifts the dark bands only", "sky_exp": "Darkens the sky alone" },
            "exposed": {
                "backlight_lift": "One move for the shadows' bands and a little shadhi",
                "sky": "Darker or brighter sky, inside the sky zone",
                "mountain_detail": "Local contrast for the mountain",
                "haze": "Clears haze"
            },
            "zones": { "sky": "Bright and high: the sky" },
            "targets": { "backlight_lift": "Bring the backlight gap to three stops" }
        }
    })
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-design-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The sky's greys run from 0.5 to 0.9 over 12 rows, the foreground's
/// from 0.03 to 0.08 below: four stops apart.
fn backlit() -> Image {
    Image::from_fn(Region::sized(32, 32), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let t = x as f32 / 31.0;
        if y < 12 {
            0.5 + 0.4 * t
        } else {
            0.03 + 0.05 * t
        }
    })
}

/// A window on a blank workspace, workshops on, the backlit photo open.
fn window(root: &Path) -> Harness<'static, Shell> {
    let config = root.join("config");
    let dir = root.join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    let panels = helios_panels_with(Experimental { workshops: true });
    let mut shell = Shell::builder(panels, Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    let built = shell
        .workspace_bundle()
        .unwrap()
        .build_with(&templates::modules(), Source::picture(&backlit()), None)
        .unwrap();
    shell
        .set_view_source(built.graph, built.view.0, &built.view.1)
        .unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    h
}

/// The frames a step takes, the image kept open through a network change.
fn follow(h: &mut Harness<'static, Shell>) {
    for _ in 0..3 {
        h.run();
    }
    if h.state_mut().take_network_changed() {
        assert!(h.state_mut().keep_image_open().unwrap(), "kept in place");
    }
    for _ in 0..2 {
        h.run();
    }
}

/// What the node's MCP server sends the desktop holding the workspace
/// for a `helios_workshop_*` tool, and the desktop's answer.
fn agent(h: &mut Harness<'static, Shell>, root: &Path, mut request: Json) -> Json {
    request["workspace"] = json!(root.join("photos"));
    let out = h.state_mut().workspace_request(&request);
    follow(h);
    out
}

fn answer(out: Json) -> Json {
    assert_eq!(out["ok"], json!(true), "{out}");
    out["answer"].clone()
}

fn steps(h: &Harness<'static, Shell>) -> usize {
    h.state().values().lock().unwrap().undo_steps().len()
}

fn has_value(h: &Harness<'static, Shell>, path: &str) -> bool {
    h.state().values().lock().unwrap().get(path).is_some()
}

#[test]
fn an_agent_designs_the_backlit_workshop_and_the_panel_shows_only_its_sliders() {
    stand_ins();
    let root = temp("backlit");
    let mut h = window(&root);
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    // The user says what they want, and asks.
    h.get_by_label("Describe what you want").focus();
    h.run();
    h.get_by_label("Describe what you want").type_text(INTENT);
    h.run();
    h.get_by_label("Design a workshop").click();
    follow(&mut h);
    h.get_by_label_contains("Waiting for your agent");

    // The agent reads the context: the words, the measures, the catalogue;
    // no pixels, the user has not allowed them.
    let ctx = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.context", "proxy": true }),
    ));
    assert_eq!(ctx["intent"], json!(INTENT));
    assert_eq!(ctx["session"], json!(1));
    let median = ctx["measures"]["image"]["luma.median"].as_f64().unwrap();
    assert!(
        median < -3.0,
        "the foreground is most of the photo: {median}"
    );
    let bright = &ctx["measures"]["candidate_zones"][0];
    assert_eq!(bright["zone"]["name"], json!("bright"));
    assert!(bright["share"].as_f64().unwrap() > 30.0, "{bright}");
    let modules: Vec<&str> = ctx["modules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert!(
        modules.contains(&"toneequal") && modules.contains(&"shadhi"),
        "{modules:?}"
    );
    assert!(ctx["bricks"].as_array().is_some_and(|b| !b.is_empty()));
    assert_eq!(ctx["proxy"], Json::Null);
    assert!(
        ctx["proxy_refused"]
            .as_str()
            .unwrap()
            .contains("workshops.ai.send_proxy"),
        "{ctx}"
    );
    // Only the user switches the proxy on.
    let out = agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.send_proxy", "on": true }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(out["error"].as_str().unwrap().contains("only the user"));

    // The agent posts its draft: checked, applied as one undo step.
    let before = steps(&h);
    let a = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.design", "draft": backlit_draft() }),
    ));
    let check = &a["check"];
    assert_eq!(check["ok"], json!(true), "{check}");
    let verdict = |name: &str| {
        check["sliders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == json!(name))
            .map(|s| s["verdict"].clone())
            .unwrap()
    };
    assert_eq!(verdict("haze"), json!("dropped"), "{check}");
    assert_eq!(verdict("backlight_lift"), json!("kept"), "{check}");
    // Half a stop up, the sky's top clips; so does it under strong local
    // contrast: both ranges are pulled in.
    assert_eq!(verdict("sky"), json!("narrowed"), "{check}");
    assert_eq!(verdict("mountain_detail"), json!("narrowed"), "{check}");
    assert!(
        check["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().starts_with("Haze: dropped")),
        "{check}"
    );
    assert_eq!(a["applied"], json!(true));
    assert_eq!(a["designed"], json!("user.backlit-mountain@1"), "{a}");
    let aimed = &a["aimed"][0];
    assert_eq!(aimed["param"], json!("backlight_lift"));
    assert_eq!(aimed["reached"], json!(true), "{a}");
    assert_eq!(steps(&h), before + 1, "one undo step");
    let node = a["node"].as_str().unwrap().to_string();

    // The panel shows the workshop's sliders, and only those.
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label_contains("Designed: user.backlit-mountain@1");
    for name in ["backlight_lift", "sky", "mountain_detail"] {
        assert!(has_value(&h, &format!("node/{node}/{name}")), "{name}");
    }
    assert!(!has_value(&h, &format!("node/{node}/haze")));
    // Their rows (a note may name the Sky, narrowed; the dropped Haze
    // has no row).
    for label in ["Backlight lift", "Mountain detail"] {
        assert!(
            h.query_all_by_label_contains(label).next().is_some(),
            "{label} shown"
        );
    }
    assert!(h.query_by_label("Haze").is_none());
    let file = h
        .state()
        .workspace_state()
        .unwrap()
        .bundle
        .composites
        .into_iter()
        .find(|f| f.id == "user.backlit-mountain")
        .unwrap();
    let exposed: Vec<&str> = file.exposed.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(exposed, ["backlight_lift", "sky", "mountain_detail"]);
    assert_eq!(file.workshop.as_ref().unwrap().intent, INTENT);

    // Ctrl+Z takes the whole design back.
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap()
        .expect("a step to undo");
    follow(&mut h);
    let nodes: Vec<String> = h
        .state()
        .workspace_state()
        .unwrap()
        .bundle
        .template
        .network
        .nodes
        .iter()
        .map(|n| n.name.clone())
        .collect();
    assert!(!nodes.contains(&node), "{nodes:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_refused_draft_applies_nothing_and_the_user_alone_lets_the_proxy_out() {
    stand_ins();
    let root = temp("refused");
    let mut h = window(&root);
    // No session and no words: the agent is told how to start.
    let out = agent(&mut h, &root, json!({ "cmd": "workshop.context" }));
    assert!(
        out["error"].as_str().unwrap().contains("no design session"),
        "{out}"
    );
    h.state_mut()
        .run(
            "workshop.describe",
            json!({ "intent": INTENT }),
            Origin::User,
        )
        .unwrap();
    follow(&mut h);
    // A target on a zone the draft lacks: refused, nothing applied.
    let mut draft = backlit_draft();
    draft["composite"]["workshop"]["targets"][0]["zone"] = json!("moon");
    let before = steps(&h);
    let a = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.design", "draft": draft }),
    ));
    assert_eq!(a["applied"], json!(false));
    assert_eq!(a["check"]["ok"], json!(false));
    assert!(
        a["check"]["refused"][0]
            .as_str()
            .unwrap()
            .contains("no zone `moon`"),
        "{a}"
    );
    assert_eq!(steps(&h), before);
    // A check without applying.
    let a = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.design", "draft": backlit_draft(), "apply": false }),
    ));
    assert_eq!(
        (a["check"]["ok"].clone(), a["applied"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(steps(&h), before);
    // The user lets the proxy out: the agent gets a PNG.
    h.state_mut()
        .run("workshop.send_proxy", json!({ "on": true }), Origin::User)
        .unwrap();
    follow(&mut h);
    let ctx = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.context", "proxy": true }),
    ));
    assert_eq!(ctx["privacy"]["workshops.ai.send_proxy"], json!(true));
    assert_eq!(ctx["proxy"]["mime"], json!("image/png"), "{}", ctx["proxy"]);
    assert_eq!(ctx["proxy"]["width"], json!(32));
    assert!(ctx["proxy"]["base64"].as_str().unwrap().len() > 40);
    let _ = std::fs::remove_dir_all(&root);
}

/// The user describes, the scripted agent designs: the session starts
/// with the design. Returns the workshop's node and the session's id.
fn designed(h: &mut Harness<'static, Shell>, root: &Path) -> (String, String) {
    h.state_mut()
        .run(
            "workshop.describe",
            json!({ "intent": INTENT }),
            Origin::User,
        )
        .unwrap();
    follow(h);
    let a = answer(agent(
        h,
        root,
        json!({ "cmd": "workshop.design", "draft": backlit_draft() }),
    ));
    assert_eq!(a["applied"], json!(true), "{a}");
    (
        a["node"].as_str().unwrap().to_string(),
        a["session"].as_str().expect("a session").to_string(),
    )
}

/// Sets one of the workshop's values, as `origin`, with words or not.
fn set_value(
    h: &mut Harness<'static, Shell>,
    path: &str,
    v: f64,
    origin: Origin,
    aim: Option<&str>,
) {
    let mut set = helios_values::Set::new(
        path,
        helios_values::Change::To(helios_core::Value::Float(v)),
        origin,
    );
    if let Some(aim) = aim {
        set = set.aim(aim);
    }
    h.state().values().lock().unwrap().set(set).unwrap();
    follow(h);
}

fn record(h: &mut Harness<'static, Shell>, root: &Path) -> Json {
    answer(agent(h, root, json!({ "cmd": "workshop.session" })))
}

#[test]
fn a_piloted_session_records_who_set_what_why_and_what_the_zones_measured() {
    stand_ins();
    let root = temp("session");
    let mut h = window(&root);
    let (node, session) = designed(&mut h, &root);
    let lift = format!("node/{node}/backlight_lift");
    let detail = format!("node/{node}/mountain_detail");
    // The user lifts the backlight, saying why; moves the detail; the
    // agent, on its own, pushes the detail further; the user undoes it.
    set_value(
        &mut h,
        &lift,
        0.9,
        Origin::User,
        Some("the climber's face is too dark"),
    );
    // (The check narrowed the detail's range to about 0 to 0.26.)
    set_value(&mut h, &detail, 0.1, Origin::User, None);
    set_value(&mut h, &detail, 0.2, Origin::Agent("mcp".into()), None);
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap()
        .expect("a step to undo");
    follow(&mut h);

    let out = record(&mut h, &root);
    let rec = &out["record"];
    assert_eq!(rec["session"], json!(session));
    assert_eq!(rec["node"], json!(node));
    assert_eq!(rec["workshop"], json!("user.backlit-mountain@1"));
    assert_eq!(rec["kept"], json!(false));
    let steps = rec["steps"].as_array().unwrap();
    let kinds: Vec<&str> = steps.iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["design", "set", "set", "set", "undo"], "{rec}");
    let authors: Vec<&str> = steps
        .iter()
        .map(|s| s["author"].as_str().unwrap())
        .collect();
    assert!(authors[0].starts_with("agent:"), "{authors:?}");
    assert_eq!(authors[1..], ["user", "user", "agent:mcp", "user"]);
    assert_eq!(steps[0]["aim"], json!(INTENT));
    assert_eq!(steps[1]["aim"], json!("the climber's face is too dark"));
    assert_eq!(steps[2]["aim"], Json::Null);
    assert_eq!(steps[1]["changes"][0]["param"], json!("backlight_lift"));
    assert_eq!(steps[1]["changes"][0]["new"], json!(0.9));
    // The undo puts the agent's value back.
    assert_eq!(
        steps[4]["changes"][0],
        json!({ "param": "mountain_detail", "old": 0.2, "new": 0.1 })
    );
    let seqs: Vec<u64> = steps.iter().map(|s| s["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    // The zones measured around each step: the lift brightens the
    // foreground, and each step starts where the last one ended.
    let m = |i: usize, when: &str, key: &str| {
        steps[i]["measures"][when][key]
            .as_f64()
            .unwrap_or_else(|| panic!("{i} {when} {key}: {}", steps[i]))
    };
    assert!(m(0, "before", "foreground.luma.median") < m(0, "after", "foreground.luma.median"));
    assert!(
        m(1, "after", "foreground.luma.median") > m(1, "before", "foreground.luma.median"),
        "{}",
        steps[1]
    );
    assert_eq!(
        m(1, "before", "foreground.luma.median"),
        m(0, "after", "foreground.luma.median")
    );
    assert!(steps[1]["measures"]["after"]["sky.luma.high"].is_number());
    assert!(steps[1]["measures"]["after"]["image.clip.high"].is_number());
    // The net outcome: the lift as the user set it, the detail as the
    // undo left it.
    assert_eq!(out["net"]["backlight_lift"]["value"], json!(0.9));
    assert_eq!(out["net"]["mountain_detail"]["value"], json!(0.1));
    // The value store's steps carry the words too.
    let store = h.state().values().lock().unwrap();
    assert!(
        store
            .undo_steps()
            .iter()
            .any(|s| s.aim.as_deref() == Some("the climber's face is too dark"))
    );
    drop(store);
    // The panel says it, with Keep.
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label_contains("Session recorded: 5 steps");
    h.get_by_label("Keep");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_session_not_kept_leaves_nothing_and_a_kept_one_survives_a_reopen() {
    use helios_gui::persist::Persist;
    use helios_vault::Vault;
    use helios_vault_sqlite::SqliteStore;
    stand_ins();
    let root = temp("kept");
    let db = root.join("vault.db");
    let mut h = window(&root);
    let mut persist = Persist::new(
        SqliteStore::open(&db).unwrap(),
        std::sync::Arc::new(helios_core::Bus::new()),
    );
    let sessions = |p: &Persist<SqliteStore>| {
        p.vault()
            .workshop_sessions()
            .unwrap()
            .into_iter()
            .map(|(k, v)| (k, v["kept"].as_bool().unwrap()))
            .collect::<Vec<_>>()
    };
    // Recorded as it goes, then closed unkept: gone.
    let (node, dropped) = designed(&mut h, &root);
    set_value(
        &mut h,
        &format!("node/{node}/backlight_lift"),
        0.8,
        Origin::User,
        None,
    );
    persist.frame_settled(h.state_mut(), 1.0);
    assert_eq!(sessions(&persist), [(dropped.clone(), false)]);
    // An agent cannot keep it.
    let out = agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.session", "action": "keep" }),
    );
    assert!(
        out["error"].as_str().unwrap().contains("only the user"),
        "{out}"
    );
    let closed = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.session", "action": "close" }),
    ));
    assert_eq!(closed["dropped"], json!(true));
    persist.frame_settled(h.state_mut(), 2.0);
    assert!(sessions(&persist).is_empty(), "nothing left behind");

    // A new design: a new session, kept by the user.
    let (_, kept) = designed(&mut h, &root);
    assert_ne!(kept, dropped);
    // Its workshop is bundled next to the first one: the files read
    // back as written, not as a change made outside the desktop (which
    // reloaded the layout under the focused panel).
    assert_eq!(h.state_mut().check_workspace_files(), None);
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label("Keep").click();
    follow(&mut h);
    persist.frame_settled(h.state_mut(), 3.0);
    assert_eq!(sessions(&persist), [(kept.clone(), true)]);
    // One more design: the kept session ends and stays; the new one is
    // recorded, unkept, when the desktop goes.
    let (_, left) = designed(&mut h, &root);
    persist.frame_settled(h.state_mut(), 4.0);
    assert_eq!(sessions(&persist).len(), 2);
    drop(persist);
    drop(h);

    // Reopened: the session left unkept is dropped, the kept one is
    // there with its steps.
    let reopened = Persist::new(
        SqliteStore::open(&db).unwrap(),
        std::sync::Arc::new(helios_core::Bus::new()),
    );
    assert_eq!(reopened.drop_unkept_sessions().unwrap(), 1);
    assert_eq!(sessions(&reopened), [(kept.clone(), true)]);
    let doc = Vault::new(SqliteStore::open(&db).unwrap())
        .workshop_session(&kept)
        .unwrap()
        .unwrap();
    assert_eq!(doc["steps"][0]["kind"], json!("design"));
    assert!(reopened.vault().workshop_session(&left).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_agent_proposes_the_user_accepts_one_ghost_and_rejects_another() {
    stand_ins();
    let root = temp("propose");
    let mut h = window(&root);
    let (node, session) = designed(&mut h, &root);
    let lift = format!("node/{node}/backlight_lift");
    let sky = format!("node/{node}/sky");
    let value = |h: &Harness<'static, Shell>, path: &str| {
        h.state()
            .values()
            .lock()
            .unwrap()
            .get(path)
            .unwrap()
            .value
            .as_f64()
            .unwrap()
    };
    let (lift_before, sky_before) = (value(&h, &lift), value(&h, &sky));
    // Out of range, or a slider the workshop lacks: refused.
    for values in [json!({ "backlight_lift": 3.0 }), json!({ "haze": 0.5 })] {
        let out = agent(
            &mut h,
            &root,
            json!({ "cmd": "workshop.propose", "values": values }),
        );
        assert_eq!(out["ok"], json!(false), "{out}");
    }
    // The agent proposes two values: nothing is set, both are ghosts.
    let before = steps(&h);
    let p = answer(agent(
        &mut h,
        &root,
        json!({
            "cmd": "workshop.propose",
            "session": session,
            "aim": "the climber's face is too dark",
            "values": { "backlight_lift": 0.8, "sky": -0.3 }
        }),
    ));
    let of = p["proposal"].as_u64().unwrap();
    assert_eq!(steps(&h), before, "a proposal sets nothing");
    assert_eq!(value(&h, &lift), lift_before);
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label_contains("backlight_lift: 0.80 proposed by agent:");
    h.get_by_label_contains("sky: -0.30 proposed by agent:");
    // Only the user answers.
    let out = agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.answer", "of": of, "param": "sky", "action": "accept" }),
    );
    assert!(
        out["error"].as_str().unwrap().contains("only the user"),
        "{out}"
    );
    // The user accepts the lift (one undo step) and rejects the sky.
    h.state_mut()
        .run(
            "workshop.answer",
            json!({ "of": of, "param": "backlight_lift", "action": "accept" }),
            Origin::User,
        )
        .unwrap();
    follow(&mut h);
    assert_eq!(steps(&h), before + 1, "accepting is one undo step");
    assert_eq!(value(&h, &lift), 0.8);
    h.state_mut()
        .run(
            "workshop.answer",
            json!({ "of": of, "param": "sky", "action": "reject" }),
            Origin::User,
        )
        .unwrap();
    follow(&mut h);
    assert_eq!(steps(&h), before + 1, "rejecting sets nothing");
    assert_eq!(value(&h, &sky), sky_before);
    assert!(
        h.query_by_label_contains("proposed by").is_none(),
        "no ghost left"
    );

    // The record holds the proposal and both answers, with their authors.
    let out = record(&mut h, &root);
    let steps_ = out["record"]["steps"].as_array().unwrap();
    let kinds: Vec<&str> = steps_.iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["design", "propose", "accept", "reject"], "{out}");
    let propose = &steps_[1];
    assert!(propose["author"].as_str().unwrap().starts_with("agent:"));
    assert_eq!(propose["aim"], json!("the climber's face is too dark"));
    assert_eq!(
        propose["proposal"],
        json!({ "backlight_lift": 0.8, "sky": -0.3 })
    );
    let accept = &steps_[2];
    assert_eq!(accept["author"], json!("user"));
    assert_eq!(accept["of"], json!(of));
    assert_eq!(accept["aim"], json!("the climber's face is too dark"));
    assert_eq!(accept["changes"][0]["param"], json!("backlight_lift"));
    assert_eq!(accept["changes"][0]["new"], json!(0.8));
    assert!(accept["measures"]["after"]["foreground.luma.median"].is_number());
    let reject = &steps_[3];
    assert_eq!(
        (
            reject["author"].clone(),
            reject["of"].clone(),
            reject["proposal"].clone()
        ),
        (json!("user"), json!(of), json!({ "sky": -0.3 }))
    );
    assert!(reject.get("changes").is_none());
    assert_eq!(out["net"]["backlight_lift"]["value"], json!(0.8));
    assert!(out["net"].get("sky").is_none());
    // The store's step carries the proposal's words; undo takes it back.
    {
        let mut store = h.state().values().lock().unwrap();
        assert_eq!(
            store.undo_steps().last().unwrap().aim.as_deref(),
            Some("the climber's face is too dark")
        );
        store.undo(&Origin::User).unwrap();
    }
    follow(&mut h);
    assert_eq!(value(&h, &lift), lift_before);

    // A user's own move of a proposed value overrides it.
    let p = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.propose", "values": { "backlight_lift": 0.7 } }),
    ));
    set_value(&mut h, &lift, 0.6, Origin::User, None);
    let out = record(&mut h, &root);
    let last = out["record"]["steps"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["kind"], json!("override"), "{last}");
    assert_eq!(last["of"], p["proposal"]);
    assert_eq!(last["changes"][0]["new"], json!(0.6));
    let _ = std::fs::remove_dir_all(&root);
}

/// "Make it automatic" (slice 7): the session's values on its test photo
/// become rows with their evidence; an agent reads and amends them, the
/// user alone confirms, and the confirmed rows write the workshop's next
/// version as one undo step: the lift a target, the detail a fixed
/// default, the session kept.
#[test]
fn make_it_automatic_writes_the_next_version_once_the_user_confirms() {
    stand_ins();
    let root = temp("generalise");
    let mut h = window(&root);
    let (node, session) = designed(&mut h, &root);
    let lift = format!("node/{node}/backlight_lift");
    let detail = format!("node/{node}/mountain_detail");
    set_value(
        &mut h,
        &lift,
        0.9,
        Origin::User,
        Some("the climber's face is too dark"),
    );
    set_value(&mut h, &detail, 0.1, Origin::User, None);

    // The agent reads the rows: one per slider the session set, a fixed
    // value by default on one test photo, the lift's target offered.
    let a = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.generalise" }),
    ));
    assert_eq!(a["session"], json!(session));
    assert_eq!(a["photos"], json!(1));
    let rows = a["rows"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r["param"].as_str().unwrap()).collect();
    assert_eq!(names, ["backlight_lift", "mountain_detail"], "{a}");
    for r in rows {
        assert_eq!(r["default"], json!("fixed"), "{r}");
        assert!(
            r["candidates"].as_array().is_some_and(|c| !c.is_empty()),
            "{r}"
        );
    }
    assert!(rows[0]["target"].is_object(), "{}", rows[0]);
    assert_eq!(rows[0]["fixed"], json!(0.9));
    assert!(
        rows[0]["note"]
            .as_str()
            .unwrap()
            .contains("one test photo cannot tell"),
        "{}",
        rows[0]
    );
    assert_eq!(a["zones"], json!([]));
    assert_eq!((a["J"].clone(), a["A"].clone()), (json!(0), json!(0.0)));
    // An agent amends the choices, a wrong one refused; it never confirms.
    let a = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.generalise", "rows": { "mountain_detail": "ai" } }),
    ));
    assert_eq!(a["rows"][1]["default"], json!("ai"));
    assert_eq!((a["J"].clone(), a["A"].clone()), (json!(1), json!(1.0)));
    let out = agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.generalise", "rows": { "nope": "fixed" } }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    let out = agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.generalise", "confirm": true }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(out.to_string().contains("only the user confirms"), "{out}");

    // The panel shows the rows the agent amended, with J; Make it
    // automatic shows them again at their defaults, with Confirm.
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label_contains("AI per photo: 1 junctions, 1 calls");
    h.get_by_label("Make it automatic").click();
    follow(&mut h);
    for _ in 0..3 {
        h.run();
    }
    h.get_by_label_contains("AI per photo: none");
    h.get_by_label("Confirm");
    assert!(h.get_all_by_label("Keep as AI").count() >= 2);

    // The rows the user will confirm, as the agent reads them.
    let shown = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.generalise", "rows": { "backlight_lift": "target" } }),
    ));
    let version = |h: &Harness<'static, Shell>, v: u32| {
        h.state()
            .workspace_bundle()
            .unwrap()
            .composites
            .into_iter()
            .find(|f| f.id == "user.backlit-mountain" && f.version == v)
    };
    let before = steps(&h);

    // The user confirms: the lift a target, the detail its value, one
    // undo step.
    h.state_mut()
        .run(
            "workshop.generalise",
            json!({ "rows": { "backlight_lift": "target" }, "confirm": true }),
            Origin::User,
        )
        .unwrap();
    follow(&mut h);
    assert_eq!(steps(&h), before + 1, "one undo step");
    let file = version(&h, 2).expect("the next version");
    let block = file.workshop.as_ref().unwrap();
    assert_eq!(block.targets.len(), 1, "{block:?}");
    assert_eq!(
        serde_json::to_value(&block.targets[0]).unwrap(),
        shown["rows"][0]["target"]["target"]
    );
    let default = |name: &str| {
        file.exposed
            .iter()
            .find(|x| x.name == name)
            .and_then(|x| x.default.clone())
    };
    assert_eq!(
        default("mountain_detail"),
        Some(helios_core::Value::Float(0.1))
    );
    assert_eq!(default("backlight_lift"), None);
    // The image keeps its values; the session is kept, going on with the
    // version written.
    let v = h
        .state()
        .values()
        .lock()
        .unwrap()
        .get(&lift)
        .map(|e| e.value.clone());
    assert_eq!(v, Some(helios_core::Value::Float(0.9)));
    let rec = record(&mut h, &root)["record"].clone();
    assert_eq!(rec["kept"], json!(true));
    assert_eq!(rec["workshop"], json!("user.backlit-mountain@2"));
    let _ = std::fs::remove_dir_all(&root);
}

/// Opens `image` in place of the photo open, as the vault image `id`:
/// the workspace's network, the workshop's node included, on it.
fn open_photo(h: &mut Harness<'static, Shell>, image: &Image, id: &str) {
    let built = h
        .state()
        .workspace_bundle()
        .unwrap()
        .build_with(&templates::modules(), Source::picture(image), None)
        .unwrap();
    h.state_mut()
        .set_view_source(built.graph, built.view.0, &built.view.1)
        .unwrap();
    h.state_mut().set_edited_image(Some(id.into()));
    follow(h);
}

/// The backlit photo with its sky a stop darker.
fn darker_sky() -> Image {
    let a = backlit();
    Image::from_fn(a.region(), 4, |x, y, c| {
        let v = a.data()[((y * 32 + x) * 4 + i64::from(c)) as usize];
        if c == 3 || y >= 12 { v } else { v / 2.0 }
    })
}

/// A second test photo joins the session with the first move made on it;
/// generalising reads both, and a row kept as AI is written into the
/// workshop as a junction.
#[test]
fn a_second_test_photo_joins_the_session_and_a_junction_is_written() {
    stand_ins();
    let root = temp("second-photo");
    let mut h = window(&root);
    h.state_mut().set_edited_image(Some("photo-a".into()));
    let (node, session) = designed(&mut h, &root);
    let lift = format!("node/{node}/backlight_lift");
    let detail = format!("node/{node}/mountain_detail");
    set_value(&mut h, &lift, 0.9, Origin::User, None);

    // Another photo open: nothing recorded until the workshop moves on it.
    open_photo(&mut h, &darker_sky(), "photo-b");
    let rec = record(&mut h, &root)["record"].clone();
    assert_eq!(rec["images"], json!(["photo-a"]), "{rec}");
    set_value(&mut h, &lift, 0.5, Origin::User, None);
    let rec = record(&mut h, &root)["record"].clone();
    assert_eq!(rec["session"], json!(session));
    assert_eq!(rec["images"], json!(["photo-a", "photo-b"]), "{rec}");
    let last = rec["steps"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["image"], json!(1), "{last}");
    assert_eq!(last["changes"][0]["new"], json!(0.5), "{last}");

    // Back on the first photo, the session goes on there.
    open_photo(&mut h, &backlit(), "photo-a");
    set_value(&mut h, &detail, 0.1, Origin::User, None);
    let rec = record(&mut h, &root)["record"].clone();
    let last = rec["steps"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["image"], json!(0), "{last}");
    assert_eq!(rec["images"].as_array().unwrap().len(), 2);

    // Generalising reads both photos: each row holds a value per photo.
    let a = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.generalise" }),
    ));
    assert_eq!(a["photos"], json!(2), "{a}");
    let rows = a["rows"].as_array().unwrap();
    let row = |name: &str| {
        rows.iter()
            .find(|r| r["param"] == json!(name))
            .unwrap_or_else(|| panic!("no row {name}: {a}"))
            .clone()
    };
    let values = row("backlight_lift")["values"].clone();
    assert_eq!(values.as_array().map(Vec::len), Some(2), "{values}");
    assert!(values.as_array().unwrap().contains(&json!(0.5)), "{values}");

    // The detail kept as AI: written as a junction, J = 1.
    let a = answer(agent(
        &mut h,
        &root,
        json!({ "cmd": "workshop.generalise", "rows": { "mountain_detail": "ai" } }),
    ));
    assert_eq!(a["J"], json!(1), "{a}");
    // The limits a run stays within, prefilled from the two test photos.
    assert_eq!(
        a["limits"]["junctions"]["mountain_detail"]["min_confidence"],
        json!(0.7),
        "{a}"
    );
    assert_eq!(a["limits"]["input_span"].as_array().map(Vec::len), Some(2));
    let fixed = row("mountain_detail")["fixed"].as_f64().unwrap();
    let before = steps(&h);
    h.state_mut()
        .run(
            "workshop.generalise",
            json!({ "rows": { "mountain_detail": "ai" }, "confirm": true }),
            Origin::User,
        )
        .unwrap();
    follow(&mut h);
    assert_eq!(steps(&h), before + 1, "one undo step");
    let file = h
        .state()
        .workspace_bundle()
        .unwrap()
        .composites
        .into_iter()
        .find(|f| f.id == "user.backlit-mountain" && f.version == 2)
        .expect("the next version");
    let block = file.workshop.as_ref().unwrap();
    assert_eq!(block.junctions.len(), 1, "{block:?}");
    let j = &block.junctions[0];
    assert_eq!(j.kind, helios_template::format::JunctionKind::Value);
    assert_eq!(j.name, "mountain_detail");
    assert_eq!(j.fallback, Some(fixed));
    assert!(block.targets.iter().all(|t| t.param != "mountain_detail"));
    // Its limits written with it: the detail's bounds around its test
    // value, the junction's confidence.
    let limits = block.limits.as_ref().expect("the limits");
    let detail = &limits.sliders["mountain_detail"];
    assert_eq!(detail.value, fixed);
    assert!(detail.min <= fixed && fixed <= detail.max, "{detail:?}");
    assert!(limits.sliders.contains_key("backlight_lift"), "{limits:?}");
    assert!(limits.junctions.contains_key("mountain_detail"));
    let _ = std::fs::remove_dir_all(&root);
}
