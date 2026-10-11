//! `metadata.read` through the example plugin `exif-exiftool`, compared
//! with what exiftool itself reads. Skipped when exiftool or Python is
//! missing.

use std::path::{Path, PathBuf};
use std::process::Command;

use helios_plugin::{Broker, Policy, Scope};
use serde_json::{Value as Json, json};

const EXIF: &str = "io.github.quazardous.helios.exif-exiftool";

/// A 1x1 RGB PNG.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0,
    0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 223, 192, 0, 0, 4, 1,
    1, 128, 197, 42, 24, 93, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn installed(program: &str, arg: &str) -> bool {
    let ok = Command::new(program)
        .arg(arg)
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("{program} is not installed: skipped");
    }
    ok
}

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples")
}

/// A tagged test image, written by exiftool.
fn image() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-plugin-exiftool-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tagged.png");
    std::fs::write(&path, PNG).unwrap();
    let out = Command::new("exiftool")
        .args([
            "-q",
            "-overwrite_original",
            "-EXIF:Make=HeliosTest",
            "-EXIF:Model=Bench One",
            "-EXIF:FNumber=2.8",
            "-XMP:Subject=sky",
            "-XMP:Subject=sea",
            "-GPSLatitude=45.5",
            "-GPSLatitudeRef=N",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    path
}

fn field<'a>(answer: &'a Json, name: &str) -> &'a Json {
    answer["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap_or_else(|| panic!("no field `{name}` in {answer}"))
}

#[test]
fn metadata_read_matches_exiftool() {
    if !installed("python3", "--version") || !installed("exiftool", "-ver") {
        return;
    }
    let image = image();
    let mut b = Broker::new(Policy::default());
    b.discover(&[(Scope::Builtin, examples())]);
    assert!(b.ids().any(|id| id == EXIF), "{:?}", b.rejected());

    // Not granted yet: nothing runs.
    let e = b.metadata_read(&image).unwrap_err();
    assert!(e.contains("images.read"), "{e}");
    b.grant(EXIF, "images.read").unwrap();
    let e = b.metadata_read(&image).unwrap_err();
    assert!(e.contains("process.spawn"), "{e}");
    b.grant(EXIF, "process.spawn").unwrap();

    let answer = b.metadata_read(&image).unwrap();
    let out = Command::new("exiftool")
        .args(["-json", "-G", "-n"])
        .arg(&image)
        .output()
        .unwrap();
    let truth: Json = serde_json::from_slice(&out.stdout).unwrap();
    let truth = &truth[0];
    for (name, tag) in [
        ("camera_make", "EXIF:Make"),
        ("camera_model", "EXIF:Model"),
        ("aperture", "EXIF:FNumber"),
        ("keywords", "XMP:Subject"),
        ("gps_latitude", "Composite:GPSLatitude"),
    ] {
        let f = field(&answer, name);
        assert_eq!(f["value"], truth[tag], "{name}");
        assert_eq!(f["source"], tag, "{name}");
        assert_eq!(f["plugin"], EXIF);
    }
    assert_eq!(field(&answer, "camera_model")["value"], "Bench One");
    assert_eq!(field(&answer, "keywords")["value"], json!(["sky", "sea"]));

    // The command it contributes runs in it, started on its own event.
    let version = b.execute(EXIF, "exiftool.version", json!({})).unwrap();
    let ver = Command::new("exiftool").arg("-ver").output().unwrap();
    assert_eq!(
        version["version"],
        String::from_utf8_lossy(&ver.stdout).trim()
    );
}
