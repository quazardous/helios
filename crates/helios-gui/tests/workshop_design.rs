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
