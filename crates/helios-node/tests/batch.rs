//! `helios batch` end to end: a network over many files, each line of its
//! report, names that collide, a file that fails mid-batch, a sidecar
//! applied, darktable's test raws, an interrupt that leaves no file, and
//! the batch as a job on its node (its exports as children, cancelled by
//! `job.cancel`).

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
    // The node's refusal comes by key, for a client to translate.
    assert_eq!(failed["message"]["key"], "node-load-refused", "{failed}");
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
        line["sidecar"]["import"]["not_applied"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["operation"] == "frobnicate" && n["reason"]["kind"] == "unknown_module"),
        "{line}"
    );
    // Said once, in the line and on stderr.
    let warning =
        "1 of 2 history items not applied, kept for later: frobnicate v1 (no Helios module)";
    assert_eq!(line["sidecar"]["warning"], warning, "{line}");
    assert_eq!(err.matches(warning).count(), 1, "{err}");
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
        version.edit["import"]["not_applied"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["operation"] == "frobnicate"),
        "{}",
        version.edit
    );
    // The item not applied is kept as darktable wrote it.
    let kept_item = &version.edit["darktable"]["history"][1];
    assert_eq!(kept_item["status"], "not_applied", "{}", version.edit);
    assert_eq!(kept_item["attributes"]["darktable:params"], "00");
    assert!(
        version.edit["darktable"]["sidecar"]
            .as_str()
            .unwrap()
            .contains("frobnicate")
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

/// The versions of an image in a vault, by label, with their exposure.
fn kept_versions(vault_file: &Path, image: &Path) -> Vec<(String, f64)> {
    use helios_vault::Vault;
    use helios_vault_sqlite::SqliteStore;
    let vault = Vault::new(SqliteStore::open(vault_file).unwrap());
    let id = vault.import_file(image, 9).unwrap();
    let mut out: Vec<(String, f64)> = vault
        .versions(&id)
        .unwrap()
        .iter()
        .map(|v| {
            (
                v.label.clone().unwrap_or_default(),
                v.edit["modules"][0]["params"]["exposure"]["float"]
                    .as_f64()
                    .unwrap(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn darktable_duplicates_are_kept_as_versions_and_the_base_is_exported() {
    let p = Place::new("duplicates");
    std::fs::write(p.join("network.json"), NETWORK).unwrap();
    let photo = p.join("photo.png");
    picture(&photo, 16, 16);
    std::fs::write(p.join("photo.png.xmp"), rated_sidecar(1.0, 2)).unwrap();
    std::fs::write(p.join("photo_01.png.xmp"), rated_sidecar(-1.0, 5)).unwrap();
    // The same edit twice: still two versions, never merged.
    std::fs::write(p.join("photo_02.png.xmp"), sidecar(-1.0)).unwrap();
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
        photo.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}\n{lines:?}");
    let line = line_of(&lines, &photo);
    // The base sidecar is the one applied.
    assert_eq!(line["sidecar"]["path"], json!(p.join("photo.png.xmp")));
    let duplicates = line["sidecar"]["duplicates"].as_array().unwrap();
    let numbers: Vec<_> = duplicates.iter().map(|d| d["version"].clone()).collect();
    assert_eq!(numbers, [json!(1), json!(2)], "{line}");
    assert!(
        duplicates.iter().all(|d| d["vault"]["added"] == true),
        "{line}"
    );
    assert_eq!(
        kept_versions(&p.join("vault.db"), &photo),
        [
            ("import:xmp".to_string(), 1.0),
            ("import:xmp duplicate 01".to_string(), -1.0),
            ("import:xmp duplicate 02".to_string(), -1.0),
        ]
    );
    // Only the base's rating is the image's.
    assert_eq!(line["sidecar"]["vault"]["rating_set"], true);
    assert_eq!(duplicates[0]["vault"]["rating_set"], false);
}

/// Runs `helios import-darktable`: exit code, one JSON value per line, stderr.
fn import_darktable(args: &[&str]) -> (i32, Vec<Value>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .arg("import-darktable")
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

#[test]
fn a_darktable_folder_imports_with_its_duplicates_orphans_and_bare_raws() {
    let p = Place::new("import-darktable");
    let dir = p.join("roll");
    let (edited, bare) = (dir.join("edited.png"), dir.join("bare.png"));
    picture(&edited, 16, 16);
    picture(&bare, 16, 16);
    std::fs::write(dir.join("edited.png.xmp"), sidecar(1.0)).unwrap();
    std::fs::write(dir.join("edited_01.png.xmp"), sidecar(2.0)).unwrap();
    // A copy that came back with another case, next to the original.
    std::fs::write(dir.join("edited.PNG.XMP"), sidecar(3.0)).unwrap();
    // Its image was deleted.
    std::fs::write(dir.join("gone.png.xmp"), sidecar(4.0)).unwrap();
    std::fs::write(
        dir.join("broken_02.png.xmp"),
        r#"<rdf:Description darktable:history_end="many"/>"#,
    )
    .unwrap();
    picture(&dir.join("broken.png"), 4, 4);
    let (code, lines, err) =
        import_darktable(&["--vault", &p.s("vault.db"), dir.to_str().unwrap()]);
    // A sidecar that cannot be read fails the run, after the others.
    assert_eq!(code, 1, "{err}\n{lines:?}");
    let of = |image: &Path| {
        lines
            .iter()
            .find(|l| l["image"] == json!(image))
            .unwrap_or_else(|| panic!("no line for {}: {lines:?}", image.display()))
    };
    let versions: Vec<_> = of(&edited)["sidecars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["version"].as_u64().unwrap(),
                s["exact"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(versions, [(0, true), (0, false), (1, true)]);
    assert_eq!(of(&edited)["conflicts"], json!([0]));
    assert_eq!(of(&bare)["sidecars"], json!([]));
    let broken = &of(&dir.join("broken.png"))["sidecars"][0];
    assert!(broken["error"].is_string(), "{broken}");
    let orphan = lines.iter().find(|l| l["orphan"].is_string()).unwrap();
    assert_eq!(orphan["orphan"], json!(dir.join("gone.png.xmp")));
    assert_eq!(orphan["image"], "gone.png");
    let kept = kept_versions(&p.join("vault.db"), &edited);
    assert_eq!(kept.len(), 3, "{kept:?}");
    assert!(kept.contains(&("import:xmp duplicate 01".to_string(), 2.0)));

    // Again: nothing new, and the bare raw is still known.
    let (_, again, _) = import_darktable(&["--vault", &p.s("vault.db"), edited.to_str().unwrap()]);
    assert_eq!(again.len(), 1, "{again:?}");
    assert!(
        again[0]["sidecars"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["vault"]["added"] == false),
        "{again:?}"
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

/// The `job.*` events of a node, from its journal.
#[cfg(debug_assertions)]
fn job_events(node: &Sandbox) -> Vec<Value> {
    node.inspect(&["events"])["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"].as_str().unwrap().starts_with("job."))
        .cloned()
        .collect()
}

/// A node cancels a job on `job.cancel` with its id: a queued export
/// says `job.cancelled` and writes nothing.
#[cfg(debug_assertions)]
#[test]
fn a_node_cancels_an_export_on_job_cancel() {
    let p = Place::new("job-cancel");
    let node = Sandbox::start(p.join("node"), NETWORK);
    let pic = p.join("in.png");
    picture(&pic, 8, 8);
    assert_eq!(
        node.control(&["load-source", "src", pic.to_str().unwrap()])["ok"],
        true
    );
    // Paused: the export waits in the queue.
    node.control(&["pause"]);
    let out = p.join("out.png");
    let asked = node.control(&[
        "export",
        "exp",
        "--format",
        "png",
        "--to",
        out.to_str().unwrap(),
    ]);
    assert_eq!(asked["ok"], true, "{asked}");
    let started = node.inspect(&["wait-event", "job.started"]);
    let id = started["event"]["payload"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(id.starts_with("export-"), "{started}");
    let cancel = node.control(&["job-cancel", &id]);
    assert_eq!(cancel["ok"], true, "{cancel}");
    node.control(&["resume"]);
    assert_eq!(node.inspect(&["wait-idle"])["ok"], true);
    let ends: Vec<Value> = job_events(&node)
        .into_iter()
        .filter(|e| e["payload"]["id"] == id.as_str())
        .map(|e| e["kind"].clone())
        .collect();
    assert_eq!(ends.first(), Some(&json!("job.started")), "{ends:?}");
    assert!(ends.contains(&json!("job.cancelling")), "{ends:?}");
    assert_eq!(ends.last(), Some(&json!("job.cancelled")), "{ends:?}");
    assert!(!out.exists(), "a cancelled export writes nothing");
}

/// A batch is a job on the node it drives, its exports its children:
/// egui-pop's jobs model shows one batch with an export per image.
#[cfg(debug_assertions)]
#[test]
fn a_batch_shows_as_a_parent_with_its_exports_as_children() {
    use egui_pop_model::jobs::{Incoming, JobState, Jobs as Model};

    let p = Place::new("parent");
    let node = Sandbox::start(p.join("node"), NETWORK);
    let images: Vec<PathBuf> = (0..3).map(|i| p.join(&format!("in{i}.png"))).collect();
    for i in &images {
        picture(i, 8, 8);
    }
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args([
            "batch",
            "--socket",
            &node.inspect,
            "--control",
            &node.control,
        ])
        .args(["--out", &p.s("out"), "--format", "png"])
        .args(&images)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut model = Model::new();
    for e in job_events(&node) {
        model.apply(
            &Incoming::new(
                e["seq"].as_u64().unwrap(),
                e["kind"].as_str().unwrap(),
                &e["payload"],
            ),
            0.0,
        );
    }
    let roots = model.roots();
    assert_eq!(roots.len(), 1, "{:?}", model.all());
    let batch = roots[0];
    assert_eq!(batch.kind, "batch");
    assert_eq!(batch.title, "Batch of 3 images");
    assert_eq!(batch.state, JobState::Done);
    let children = model.children(&batch.id);
    assert_eq!(children.len(), 3);
    assert!(
        children
            .iter()
            .all(|c| c.kind == "export" && c.state == JobState::Done)
    );
    assert_eq!(batch.fraction, Some(1.0));
}

/// A batch cancelled on its node (`job.cancel` with the batch's id, as
/// the jobs centre asks): its exports are cancelled, the next ones never
/// asked, no file written, and `helios batch` says it was interrupted.
#[cfg(debug_assertions)]
#[test]
fn a_batch_cancelled_on_its_node_stops_and_leaves_no_file() {
    let p = Place::new("node-cancel");
    let node = Sandbox::start(p.join("node"), NETWORK);
    let images: Vec<PathBuf> = (0..4).map(|i| p.join(&format!("in{i}.png"))).collect();
    for i in &images {
        picture(i, 8, 8);
    }
    // Paused: the exports wait in the queue until the cancel.
    node.control(&["pause"]);
    let child = Command::new(env!("CARGO_BIN_EXE_helios"))
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
    let batch = job_events(&node)
        .into_iter()
        .find(|e| e["kind"] == "job.started" && e["payload"]["kind"] == "batch")
        .expect("the batch's job");
    let id = batch["payload"]["id"].as_str().unwrap().to_string();
    assert_eq!(node.control(&["job-cancel", &id])["ok"], true);
    // `job.cancel` is a request: the run acts on it at its next tick, and
    // an export already done stays done. The node stays paused until the
    // batch's job has ended cancelled, so no export can finish first.
    let mut since = "0".to_string();
    loop {
        let ended = node.inspect(&[
            "wait-event",
            "job.cancelled",
            "--since",
            &since,
            "--timeout",
            "30000",
        ]);
        assert_eq!(ended["ok"], true, "the batch's job ends cancelled: {ended}");
        if ended["event"]["payload"]["id"] == id.as_str() {
            break;
        }
        since = ended["event"]["seq"].as_u64().unwrap().to_string();
    }
    node.control(&["resume"]);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130));
    let lines: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines.iter().all(|l| l["error"] == "cancelled"), "{lines:?}");
    assert_eq!(node.inspect(&["wait-idle"])["ok"], true);
    assert!(
        files(&p.join("out")).is_empty(),
        "{:?}",
        files(&p.join("out"))
    );
    let ends: Vec<Value> = job_events(&node)
        .into_iter()
        .filter(|e| e["payload"]["id"] == id.as_str())
        .map(|e| e["kind"].clone())
        .collect();
    assert_eq!(ends.last(), Some(&json!("job.cancelled")), "{ends:?}");
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
