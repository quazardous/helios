//! The gphoto2 plugin through the broker, against a fake `gphoto2` put
//! first on the plugin's `PATH`: no camera needed.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helios_plugin::{Broker, Notice, Policy, Scope};

const CAMERA: &str = "io.github.quazardous.helios.gphoto2-camera";
const PORT: &str = "usb:001,004";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gphoto2-camera-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The plugin as it would be packaged (its manifest and binary), a fake
/// gphoto2, and a broker whose plugins see the fake first on `PATH`.
fn setup(name: &str) -> (Broker, PathBuf, PathBuf) {
    let root = scratch(name);
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plugin = root.join("plugins").join("gphoto2-camera");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::copy(
        here.join("helios-plugin.toml"),
        plugin.join("helios-plugin.toml"),
    )
    .unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_gphoto2-camera"),
        plugin.join("gphoto2-camera"),
    )
    .unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("gphoto2");
    std::fs::copy(here.join("tests/fake-gphoto2.sh"), &fake).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let camera = root.join("camera");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut broker = Broker::new(Policy {
        env: vec![
            ("PATH".into(), path),
            ("FAKE_GPHOTO2_DIR".into(), camera.display().to_string()),
        ],
        ..Policy::default()
    });
    broker.discover(&[(Scope::User, root.join("plugins"))]);
    assert!(broker.ids().any(|i| i == CAMERA), "{:?}", broker.rejected());
    (broker, root, camera)
}

#[test]
fn capture_and_download_with_a_fake_gphoto2() {
    let (b, root, camera) = setup("capture");
    let notices = b.subscribe();

    let e = b.device_list().unwrap();
    assert!(
        e["errors"][0]
            .as_str()
            .unwrap()
            .contains("`device.usb` (Talks to the cameras plugged in over USB), not granted"),
        "{e}"
    );
    b.grant(CAMERA, "device.usb").unwrap();
    b.grant(CAMERA, "process.spawn").unwrap();

    let list = b.device_list().unwrap();
    assert_eq!(list["devices"][0]["model"], "Helios Fake Camera");
    assert_eq!(list["devices"][0]["id"], PORT);
    assert_eq!(list["devices"][0]["plugin"], CAMERA);
    let info = b.device_info(None, Some(PORT)).unwrap();
    assert_eq!(info["summary"]["Serial Number"], "0042");

    // Without somewhere to write, the shutter does not even fire.
    let dest = root.join("photos");
    let e = b.device_capture(None, Some(PORT), &dest).unwrap_err();
    assert!(e.contains("`folder.write`, not granted"), "{e}");
    assert!(!camera.join("IMG_0001.JPG").exists());

    b.grant(CAMERA, "folder.write").unwrap();
    let shot = b.device_capture(None, Some(PORT), &dest).unwrap();
    assert_eq!(
        shot["captured"][0],
        "/store_00010001/DCIM/100HELIO/IMG_0001.JPG"
    );
    let file = dest.canonicalize().unwrap().join("IMG_0001.JPG");
    assert_eq!(shot["files"][0]["path"], file.display().to_string());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fake picture 1\n");
    assert_eq!(shot["refused"].as_array().unwrap().len(), 0);
    // Only the picture is left: the staging folder is gone.
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1);

    let progress = std::iter::from_fn(|| notices.recv_timeout(Duration::from_secs(10)).ok())
        .find_map(|n| match n {
            Notice::Progress { plugin, params } => Some((plugin, params)),
            _ => None,
        })
        .expect("a progress notice");
    assert_eq!(progress.0, CAMERA);
    assert_eq!(progress.1["done"], 1);
    assert_eq!(progress.1["total"], 1);
    assert_eq!(progress.1["token"], shot["progress"]);

    // A second shot, then both downloaded again: names do not clash.
    b.device_capture(None, Some(PORT), &dest).unwrap();
    let again = b
        .device_download(
            None,
            Some(PORT),
            &[
                "/store_00010001/DCIM/100HELIO/IMG_0001.JPG".into(),
                "/store_00010001/DCIM/100HELIO/IMG_0002.JPG".into(),
            ],
            &dest,
        )
        .unwrap();
    let names: Vec<String> = again["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            Path::new(f["path"].as_str().unwrap())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into()
        })
        .collect();
    assert_eq!(names, ["IMG_0001-1.JPG", "IMG_0002-1.JPG"]);

    let e = b.device_info(None, Some("usb:009,009")).unwrap_err();
    assert!(e.contains("no camera on usb:009,009"), "{e}");
}

/// Its capture controls (`ui/controls.json`): each committed change Helios
/// tells it (`ui/changed`) is set on the camera, and said in the log; a
/// stopped plugin is not started for it, and gets the values when it
/// starts.
#[test]
fn capture_controls_are_set_on_the_camera() {
    let (b, root, camera) = setup("controls");
    let notices = b.subscribe();
    b.grant(CAMERA, "device.usb").unwrap();
    b.grant(CAMERA, "process.spawn").unwrap();
    let values = |v: serde_json::Value| v.as_object().unwrap().clone();

    // Not running: kept for its start.
    let told = b
        .ui_changed(CAMERA, values(serde_json::json!({"iso": "400"})), "edit")
        .unwrap();
    assert_eq!(told, helios_plugin::Told::Nothing);
    b.device_list().unwrap();
    let told = b
        .ui_changed(
            CAMERA,
            values(serde_json::json!({"iso": "800", "shutter": 0.004})),
            "edit",
        )
        .unwrap();
    assert_eq!(told, helios_plugin::Told::Notified);
    let said: Vec<String> =
        std::iter::from_fn(|| notices.recv_timeout(Duration::from_secs(10)).ok())
            .filter_map(|n| match n {
                Notice::Log { message, .. } if message.starts_with("ui/changed") => Some(message),
                _ => None,
            })
            .take(2)
            .collect();
    assert_eq!(
        said,
        ["ui/changed: iso=800", "ui/changed: shutterspeed=1/250"]
    );
    let config = std::fs::read_to_string(format!("{}.config", camera.display())).unwrap();
    assert_eq!(config, "iso=800\nshutterspeed=1/250\n");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_focus_tool_s_drag_sets_the_zoom_position_once() {
    let (b, root, camera) = setup("focus");
    let notices = b.subscribe();
    b.grant(CAMERA, "device.usb").unwrap();
    b.grant(CAMERA, "process.spawn").unwrap();
    b.device_list().unwrap();
    // A drag of the focus point on the image: both values, one change.
    let values = serde_json::json!({"focus-x": 1200.4, "focus-y": 800.0});
    let told = b
        .ui_changed(CAMERA, values.as_object().unwrap().clone(), "edit")
        .unwrap();
    assert_eq!(told, helios_plugin::Told::Notified);
    let said =
        std::iter::from_fn(|| notices.recv_timeout(Duration::from_secs(10)).ok()).find_map(|n| {
            match n {
                Notice::Log { message, .. } if message.starts_with("ui/changed") => Some(message),
                _ => None,
            }
        });
    assert_eq!(
        said.as_deref(),
        Some("ui/changed: eoszoomposition=1200,800")
    );
    let config = std::fs::read_to_string(format!("{}.config", camera.display())).unwrap();
    assert_eq!(config, "eoszoomposition=1200,800\n");
    let _ = std::fs::remove_dir_all(&root);
}
