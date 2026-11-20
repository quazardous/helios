//! `helios --reset` on a fixture home only (HOME and the XDG folders
//! pointed at a temporary folder): the plan said before anything moves,
//! no terminal and no `--confirm RESET` refuses, a held library lock
//! (Helios running) refuses, and a confirmed reset puts Helios's folders
//! aside while every photo stays byte for byte.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home {
    root: PathBuf,
}

impl Home {
    fn new(name: &str) -> Home {
        let root =
            std::env::temp_dir().join(format!("helios-cli-reset-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, file, bytes) in [
            (".config/helios", "global.toml", &b"x = 1\n"[..]),
            (".local/share/helios", "notes", b"data"),
            (".local/state/helios", "helios.log", b"log"),
            (".cache/helios", "thumb", b"cache"),
            ("Pictures", "a.cr3", b"raw"),
            ("Pictures", "a.cr3.xmp", b"<xmp/>"),
        ] {
            let d = root.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), bytes).unwrap();
        }
        Home { root }
    }

    fn helios(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg("--reset")
            .args(args)
            .env("HOME", &self.root)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_CACHE_HOME")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn photos(&self) -> PathBuf {
        self.root.join("Pictures")
    }

    fn bytes(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                let b = std::fs::read(&p).unwrap();
                (p, b)
            })
            .collect();
        out.sort();
        out
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[cfg(unix)]
#[test]
fn the_reset_says_its_plan_asks_for_the_word_and_never_touches_a_photo() {
    let home = Home::new("plan");
    let photos = home.photos();
    let photos_arg = photos.to_str().unwrap();
    let before = Home::bytes(&photos);

    // No terminal and no --confirm: the plan said, then refused.
    let out = home.helios(&["everything", "--photos", photos_arg]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("moved to the backup folder"), "{text}");
    assert!(text.contains("--confirm RESET"), "{text}");
    assert!(
        home.root.join(".config/helios/global.toml").exists(),
        "nothing moved"
    );

    // Another word: refused.
    let out = home.helios(&["everything", "--photos", photos_arg, "--confirm", "yes"]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    assert!(home.root.join(".config/helios").exists());

    // A scope that is not one: usage.
    assert_eq!(home.helios(&["all"]).status.code(), Some(2));

    // Helios running: a node holds the library's lock.
    let library = home.root.join(".local/share/helios/library.db");
    let files = helios_inspect::discovery::files(&library);
    let lock = helios_inspect::discovery::Lock::try_take(&files.lock)
        .unwrap()
        .unwrap();
    let out = home.helios(&["everything", "--photos", photos_arg, "--confirm", "RESET"]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    assert!(said(&out).contains("Helios is running"), "{}", said(&out));
    assert!(
        home.root.join(".config/helios").exists(),
        "nothing moved while running"
    );
    drop(lock);

    // The caches only: the settings stay.
    let out = home.helios(&["caches", "--photos", photos_arg, "--confirm", "RESET"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(
        !home.root.join(".cache/helios").exists(),
        "the caches are removed"
    );
    assert!(
        home.root.join(".config/helios/global.toml").exists(),
        "the settings stay"
    );

    // Everything, confirmed: Helios's folders aside, the photos the same.
    let out = home.helios(&["everything", "--photos", photos_arg, "--confirm", "RESET"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(
        !home.root.join(".config/helios").exists(),
        "the settings are put aside"
    );
    assert!(!home.root.join(".local/state/helios").exists());
    let backups: Vec<_> = std::fs::read_dir(home.root.join(".local/share"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("helios-reset-"))
        .collect();
    assert_eq!(backups.len(), 1, "one dated backup folder");
    assert_eq!(Home::bytes(&photos), before, "every photo byte is the same");
}

/// A photo folder inside Helios's own folders refuses the reset, before
/// anything moves.
#[cfg(unix)]
#[test]
fn a_photo_folder_inside_helios_folders_refuses_the_reset() {
    let home = Home::new("inside");
    let inside = home.root.join(".local/share/helios/Photos");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::write(inside.join("b.jpg"), b"jpeg").unwrap();
    let out = home.helios(&[
        "everything",
        "--photos",
        inside.to_str().unwrap(),
        "--confirm",
        "RESET",
    ]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    assert!(said(&out).contains("not possible"), "{}", said(&out));
    assert_eq!(std::fs::read(inside.join("b.jpg")).unwrap(), b"jpeg");
    assert!(home.root.join(".config/helios").exists(), "nothing moved");
}
