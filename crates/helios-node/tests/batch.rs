//! `helios batch` end to end: a network over many files, each line of its
//! report, names that collide, a file that fails mid-batch, a sidecar
//! applied, darktable's test raws, and an interrupt that leaves no file.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// A fresh directory; removed when dropped.
struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("helios-batch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn s(&self, name: &str) -> String {
        self.join(name).to_str().unwrap().to_string()
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn picture(path: &Path, width: u32, height: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 7 % 256) as u8, (y * 11 % 256) as u8, 60])
    })
    .save(path)
    .unwrap();
}

/// Runs `helios batch`: exit code, one JSON value per line, stderr.
fn batch(args: &[&str]) -> (i32, Vec<Value>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .arg("batch")
        .args(args)
        .output()
        .unwrap();
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect();
    (
        out.status.code().unwrap_or(-1),
        lines,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The line of an input file.
fn line_of<'a>(lines: &'a [Value], path: &Path) -> &'a Value {
    lines
        .iter()
        .find(|l| l["path"] == json!(path))
        .unwrap_or_else(|| panic!("no line for {}: {lines:?}", path.display()))
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn every_image_is_exported_and_taken_names_get_a_number() {
    let p = Place::new("names");
    std::fs::write(p.join("network.json"), NETWORK).unwrap();
    let (a, b, c) = (p.join("a/x.png"), p.join("b/x.png"), p.join("y.png"));
    picture(&a, 20, 10);
    picture(&b, 30, 15);
    picture(&c, 12, 12);
    // A file already there is never overwritten.
    picture(&p.join("out/y.png"), 1, 1);
    let (code, lines, err) = batch(&[
        "--network",
        &p.s("network.json"),
        "--out",
        &p.s("out"),
        "--format",
        "png",
        "--jobs",
        "2",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        c.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}\n{lines:?}");
    assert_eq!(lines.len(), 3);
    for (input, output, size) in [
        (&a, "x.png", (20, 10)),
        (&b, "x-1.png", (30, 15)),
        (&c, "y-1.png", (12, 12)),
    ] {
        let line = line_of(&lines, input);
        assert!(line["error"].is_null(), "{line}");
        assert_eq!(line["output"], json!(p.join("out").join(output)), "{line}");
        assert!(line["duration_ms"].is_u64());
        let written = image::open(p.join("out").join(output)).unwrap();
        assert_eq!((written.width(), written.height()), size, "{output}");
    }
    assert_eq!(
        files(&p.join("out")),
        ["x-1.png", "x.png", "y-1.png", "y.png"]
    );
    let first = image::open(p.join("out/y.png")).unwrap();
    assert_eq!(first.width(), 1, "the file already there is untouched");
}

#[test]
fn a_file_that_fails_mid_batch_says_why_and_the_others_go_on() {
    let p = Place::new("fail");
    std::fs::write(p.join("network.json"), NETWORK).unwrap();
    let (a, broken, c) = (p.join("a.png"), p.join("broken.png"), p.join("c.png"));
    picture(&a, 8, 4);
    std::fs::write(&broken, b"not a picture").unwrap();
    picture(&c, 4, 8);
    let (code, lines, err) = batch(&[
        "--network",
        &p.s("network.json"),
        "--out",
        &p.s("out"),
        "--name",
        "{stem}-small.{ext}",
        "--quality",
        "80",
        a.to_str().unwrap(),
        broken.to_str().unwrap(),
        c.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(lines.len(), 3, "{lines:?}");
    let failed = line_of(&lines, &broken);
    assert!(failed["output"].is_null());
    assert!(
        failed["error"].as_str().unwrap().contains("broken.png"),
        "{failed}"
    );
    assert!(line_of(&lines, &c)["error"].is_null());
    assert_eq!(files(&p.join("out")), ["a-small.jpg", "c-small.jpg"]);
}

/// A darktable sidecar with one history item: exposure at `ev`.
fn sidecar(ev: f64) -> String {
    let layouts = helios_xmp::Layouts::built_in();
    let version = *layouts.versions("exposure").iter().max().unwrap();
    let layout = layouts.get("exposure", version).unwrap();
    let bytes = helios_xmp::encode(layout, &json!({ "exposure": ev })).unwrap();
    let hex: String = bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/" darktable:history_end="2">
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:num="0" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{hex}" darktable:multi_name="" darktable:multi_priority="0"/>
     <rdf:li darktable:num="1" darktable:operation="frobnicate" darktable:enabled="1" darktable:modversion="1" darktable:params="00" darktable:multi_name="" darktable:multi_priority="0"/>
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
    )
}

fn mean(path: &Path) -> f64 {
    let img = image::open(path).unwrap().to_rgb8();
    let sum: f64 = img.pixels().flat_map(|p| p.0).map(f64::from).sum();
    sum / f64::from(img.width() * img.height() * 3)
}

#[test]
fn a_sidecar_is_applied_on_top_of_the_network_for_its_image_only() {
    let p = Place::new("sidecar");
    std::fs::write(p.join("network.json"), NETWORK).unwrap();
    let (edited, plain) = (p.join("edited.png"), p.join("plain.png"));
    picture(&edited, 16, 16);
    picture(&plain, 16, 16);
    std::fs::write(p.join("edited.png.xmp"), sidecar(1.0)).unwrap();
    let (code, lines, err) = batch(&[
        "--network",
        &p.s("network.json"),
        "--out",
        &p.s("out"),
        "--format",
        "png",
        "--sidecar",
        "--jobs",
        "1",
        edited.to_str().unwrap(),
        plain.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}\n{lines:?}");
    let line = line_of(&lines, &edited);
    assert_eq!(line["sidecar"]["applied"][0]["node"], "exp", "{line}");
    assert_eq!(
        line["sidecar"]["skipped"].as_array().unwrap().len(),
        0,
        "{line}"
    );
    assert!(
        line["sidecar"]["import"]["unknown_modules"]
            .as_array()
            .unwrap()
            .contains(&json!("frobnicate")),
        "{line}"
    );
    assert!(line_of(&lines, &plain)["sidecar"].is_null());
    let (bright, same) = (
        mean(&p.join("out/edited.png")),
        mean(&p.join("out/plain.png")),
    );
    assert!(
        bright > same + 10.0,
        "+1 EV is brighter: {bright} vs {same}"
    );
}

/// [`sidecar`] with darktable's rating (`-1` is a reject).
fn rated_sidecar(ev: f64, rating: i64) -> String {
    sidecar(ev).replace(
        r#"xmlns:darktable="http://darktable.sf.net/""#,
        &format!(
            r#"xmlns:darktable="http://darktable.sf.net/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="{rating}""#
        ),
    )
}

#[test]
fn a_sidecar_kept_in_a_vault_is_a_version_and_its_rating_never_overwrites() {
    use helios_vault::fields::{Command as Field, RATING, REJECTED};
    use helios_vault::{Vault, VersionId};
    use helios_vault_sqlite::SqliteStore;
    let p = Place::new("sidecar-vault");
    std::fs::write(p.join("network.json"), NETWORK).unwrap();
    let (rated, rejected) = (p.join("rated.png"), p.join("rejected.png"));
    picture(&rated, 16, 16);
    picture(&rejected, 16, 12);
    std::fs::write(p.join("rated.png.xmp"), rated_sidecar(1.0, 3)).unwrap();
    std::fs::write(p.join("rejected.png.xmp"), rated_sidecar(0.5, -1)).unwrap();
    let run = || {
        let (code, lines, err) = batch(&[
            "--network",
            &p.s("network.json"),
            "--out",
            &p.s("out"),
            "--format",
            "png",
            "--sidecar",
            "--vault",
            &p.s("vault.db"),
            rated.to_str().unwrap(),
            rejected.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "{err}\n{lines:?}");
        lines
    };
    let lines = run();
    let kept = &line_of(&lines, &rated)["sidecar"]["vault"];
    assert_eq!(kept["added"], true, "{kept}");
    assert_eq!(kept["rating_set"], true, "{kept}");

    let vault = Vault::new(SqliteStore::open(&p.join("vault.db")).unwrap());
    let image = vault.import_file(&rated, 2).unwrap();
    assert_eq!(kept["image"], image.hex());
    let versions = vault.versions(&image).unwrap();
    assert_eq!(versions.len(), 1);
    let version = &versions[0];
    assert_eq!(
        version.id,
        VersionId::from_hex(kept["version"].as_str().unwrap()).unwrap()
    );
    assert_eq!(version.origin, "import:xmp");
    // The network darktable's history describes, with what was left out.
    assert_eq!(version.edit["modules"][0]["module"], "exposure");
    assert_eq!(
        version.edit["modules"][0]["params"]["exposure"]["float"],
        1.0
    );
    assert!(
        version.edit["import"]["unknown_modules"]
            .as_array()
            .unwrap()
            .contains(&json!("frobnicate")),
        "{}",
        version.edit
    );
    let field = |image, name: &str| {
        vault.fields_of(&[image], &[name]).unwrap()[0]
            .get(name)
            .cloned()
    };
    assert_eq!(field(image, RATING), Some(json!(3)));
    let other = vault.import_file(&rejected, 2).unwrap();
    assert_eq!(field(other, REJECTED), Some(json!(true)));
    assert_eq!(field(other, RATING), None);

    // The user rates it here; the same sidecar imported again adds no
    // version and leaves the rating alone.
    vault
        .apply(&[Field::set(image, RATING, json!(5))], "user", None, 3)
        .unwrap();
    let lines = run();
    let again = &line_of(&lines, &rated)["sidecar"]["vault"];
    assert_eq!(again["added"], false, "{again}");
    assert_eq!(again["rating_set"], false, "{again}");
    assert_eq!(vault.versions(&image).unwrap().len(), 1);
    assert_eq!(field(image, RATING), Some(json!(5)));
    let audit = vault.audit(&image).unwrap();
    assert_eq!(
        audit.iter().map(|e| e.who.as_str()).collect::<Vec<_>>(),
        ["import:xmp", "user"]
    );
}

/// A node serving the batch endpoint, started directly; killed when dropped.
struct BatchNode {
    node: std::process::Child,
    inspect: PathBuf,
    batch: PathBuf,
}

impl BatchNode {
    fn start(place: &Place, network: &str) -> Self {
        std::fs::write(place.join("node.json"), network).unwrap();
        let (inspect, batch) = (place.join("node.sock"), place.join("batch.sock"));
        let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
            .arg("--graph")
            .arg(place.join("node.json"))
            .arg("--socket")
            .arg(&inspect)
            .arg("--batch-control")
            .arg(&batch)
            .env_remove("HELIOS_DEBUG_CONTROL")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(node.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("ready "), "{line}");
        Self {
            node,
            inspect,
            batch,
        }
    }

    fn control(&self, args: &[&str]) -> Value {
        let mut all = vec!["control", "--socket", self.batch.to_str().unwrap()];
        all.extend_from_slice(args);
        helios(&all)
    }

    fn inspect(&self, args: &[&str]) -> Value {
        let mut all = vec!["inspect", "--socket", self.inspect.to_str().unwrap()];
        all.extend_from_slice(args);
        helios(&all)
    }
}

impl Drop for BatchNode {
    fn drop(&mut self) {
        let _ = self.node.kill();
        let _ = self.node.wait();
    }
}

fn helios(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(args)
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

/// A sandbox started through the CLI; cleaned when dropped.
#[cfg(debug_assertions)]
struct Sandbox {
    dir: PathBuf,
    inspect: String,
    control: String,
}

#[cfg(debug_assertions)]
impl Sandbox {
    fn start(dir: PathBuf, network: &str) -> Self {
        let network_file = dir.with_extension("json");
        std::fs::write(&network_file, network).unwrap();
        let d = dir.to_str().unwrap();
        helios(&[
            "sandbox",
            "new",
            d,
            "--network",
            network_file.to_str().unwrap(),
        ]);
        let started = helios(&["sandbox", "start", d]);
        Self {
            inspect: started["inspect"].as_str().expect("started").into(),
            control: started["control"].as_str().unwrap().into(),
            dir,
        }
    }

    fn inspect(&self, args: &[&str]) -> Value {
        let mut all = vec!["inspect", "--socket", &self.inspect];
        all.extend_from_slice(args);
        helios(&all)
    }

    fn control(&self, args: &[&str]) -> Value {
        let mut all = vec!["control", "--socket", &self.control];
        all.extend_from_slice(args);
        helios(&all)
    }
}

#[cfg(debug_assertions)]
impl Drop for Sandbox {
    fn drop(&mut self) {
        helios(&["sandbox", "clean", self.dir.to_str().unwrap()]);
    }
}

/// Pausing the scheduler needs the debug control endpoint.
#[cfg(debug_assertions)]
#[test]
fn an_interrupt_cancels_the_exports_and_leaves_no_file() {
    let p = Place::new("interrupt");
    let node = Sandbox::start(p.join("node"), NETWORK);
    let images: Vec<PathBuf> = (0..3).map(|i| p.join(&format!("in{i}.png"))).collect();
    for i in &images {
        picture(i, 8, 8);
    }
    // Paused: the exports wait in the queue until the interrupt.
    node.control(&["pause"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args([
            "batch",
            "--socket",
            &node.inspect,
            "--control",
            &node.control,
        ])
        .args(["--out", &p.s("out"), "--jobs", "2"])
        .args(&images)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let first = node.inspect(&["wait-event", "scheduler.job_queued"]);
    let since = first["event"]["seq"].as_u64().unwrap().to_string();
    let second = node.inspect(&["wait-event", "scheduler.job_queued", "--since", &since]);
    assert_eq!(second["ok"], true, "two exports in flight: {second}");
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let lines: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    let code = child.wait().unwrap().code();
    assert_eq!(code, Some(130));
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines.iter().all(|l| l["error"] == "cancelled"), "{lines:?}");
    let scheduler = node.inspect(&["describe", "scheduler"]);
    assert_eq!(
        scheduler["describe"]["totals"]["cancelled"], 2,
        "{scheduler}"
    );
    node.control(&["resume"]);
    assert_eq!(node.inspect(&["wait-idle"])["ok"], true);
    assert!(
        files(&p.join("out")).is_empty(),
        "{:?}",
        files(&p.join("out"))
    );
}

/// darktable's public test raws (`HELIOS_TEST_IMAGES`, else a darktable
/// checkout next to this repository).
fn test_images() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_TEST_IMAGES").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        },
        PathBuf::from,
    );
    if dir.join("mire1.cr2").is_file() && dir.join("bayer4.crw").is_file() {
        Some(dir)
    } else {
        eprintln!("skipped: no test images in {}", dir.display());
        None
    }
}

/// sRGB 8-bit to CIE L*a*b* (D65).
fn lab(rgb: [u8; 3]) -> [f64; 3] {
    let lin = rgb.map(|v| {
        let c = f64::from(v) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    let x = (0.4124 * lin[0] + 0.3576 * lin[1] + 0.1805 * lin[2]) / 0.95047;
    let y = 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
    let z = (0.0193 * lin[0] + 0.1192 * lin[1] + 0.9505 * lin[2]) / 1.08883;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Mean CIE76 Delta E between two pictures of the same size, over every
/// 16th pixel: a smoke test, and cheap in a debug build.
fn delta_e(a: &Path, b: &Path) -> f64 {
    let (a, b) = (
        image::open(a).unwrap().to_rgb8(),
        image::open(b).unwrap().to_rgb8(),
    );
    assert_eq!(a.dimensions(), b.dimensions());
    let (sum, n) = a
        .pixels()
        .zip(b.pixels())
        .step_by(16)
        .map(|(p, q)| {
            let (l, m) = (lab(p.0), lab(q.0));
            ((l[0] - m[0]).powi(2) + (l[1] - m[1]).powi(2) + (l[2] - m[2]).powi(2)).sqrt()
        })
        .fold((0.0, 0.0), |(sum, n), d| (sum + d, n + 1.0));
    sum / n
}

const RAW_PIPE: &str = r#"
  "modules": [
    { "name": "prep", "module": "rawprepare" },
    { "name": "wb", "module": "whitebalance" },
    { "name": "hl", "module": "highlights" },
    { "name": "demosaic", "module": "demosaic" },
    { "name": "colorin", "module": "colorin" },
    { "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.0 } } },
    { "name": "tone", "module": "sigmoid" }
  ],
  "connections": [
    { "from": "raw", "port": "image", "to": "prep", "input": "input" },
    { "from": "prep", "port": "output", "to": "wb", "input": "input" },
    { "from": "wb", "port": "output", "to": "hl", "input": "input" },
    { "from": "hl", "port": "output", "to": "demosaic", "input": "input" },
    { "from": "demosaic", "port": "output", "to": "colorin", "input": "input" },
    { "from": "colorin", "port": "output", "to": "exp", "input": "input" },
    { "from": "exp", "port": "output", "to": "tone", "input": "input" }
  ]"#;

#[test]
fn the_test_raws_go_through_a_batch_as_through_a_single_export() {
    let Some(dir) = test_images() else { return };
    let p = Place::new("raws");
    // One raw that goes through, and two files that fail: a picture (the
    // raw chain refuses it) and a raw whose sensor is not supported (CYGM).
    // A full-size raw export costs seconds even with the raw chain
    // optimized in debug builds (see Cargo.toml), so one is enough.
    let raw = dir.join("mire1.cr2");
    let cygm = dir.join("bayer4.crw");
    let network = format!(r#"{{ "sources": [{{ "name": "raw", "file": {raw:?} }}], {RAW_PIPE} }}"#);
    std::fs::write(p.join("network.json"), &network).unwrap();
    let pic = p.join("pic.png");
    picture(&pic, 16, 16);
    let (code, lines, err) = batch(&[
        "--network",
        &p.s("network.json"),
        "--out",
        &p.s("out"),
        "--format",
        "png",
        "--jobs",
        "2",
        pic.to_str().unwrap(),
        cygm.to_str().unwrap(),
        raw.to_str().unwrap(),
    ]);
    assert_eq!(lines.len(), 3, "{err}\n{lines:?}");
    let line = line_of(&lines, &raw);
    assert!(line["error"].is_null(), "{line}");
    let written = image::open(line["output"].as_str().unwrap()).unwrap();
    let loaded = helios_load::load(&raw).unwrap().image.region();
    assert_eq!(
        (written.width(), written.height()),
        (loaded.width, loaded.height)
    );
    for failed in [&pic, &cygm] {
        let line = line_of(&lines, failed);
        assert!(line["error"].is_string(), "{line}");
        assert!(line["output"].is_null(), "{line}");
    }
    assert_eq!(code, 1, "two files failed");
    assert_eq!(files(&p.join("out")), ["mire1.png"]);

    // The first raw, exported alone by a node whose network names it.
    let node = BatchNode::start(&p, &network);
    let single = p.join("single.png");
    let asked = node.control(&[
        "export",
        "tone",
        "--format",
        "png",
        "--to",
        single.to_str().unwrap(),
    ]);
    assert_eq!(asked["ok"], true, "{asked}");
    let job = asked["job"].as_u64().unwrap().to_string();
    let since = asked["seq"].as_u64().unwrap().to_string();
    let ended = node.inspect(&[
        "wait-event",
        "scheduler.job_ended",
        "--since",
        &since,
        "--timeout",
        "600000",
    ]);
    assert_eq!(ended["event"]["payload"]["job"].to_string(), job, "{ended}");
    assert_eq!(ended["event"]["payload"]["state"], "done", "{ended}");
    let de = delta_e(&single, &p.join("out/mire1.png"));
    assert!(
        de < 0.5,
        "mean Delta E {de} between a batch and a single export"
    );
}
