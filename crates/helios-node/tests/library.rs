//! `helios import` and `helios stacks` on a vault file.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("helios-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn s(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn picture(path: &Path, shade: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(32, 24, |x, y| {
        image::Rgb([(x * 7) as u8 ^ shade, (y * 9) as u8, shade])
    })
    .save(path)
    .unwrap();
}

/// Runs `helios`: exit code and stdout.
fn helios(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

#[test]
fn a_folder_is_imported_then_stacked_and_dissolved() {
    let p = Place::new("library");
    let roll = p.0.join("roll");
    picture(&roll.join("a.png"), 0);
    picture(&roll.join("b.png"), 80);
    std::fs::copy(roll.join("a.png"), roll.join("z-again.png")).unwrap();
    let vault = p.s("vault.db");
    let (code, out) = helios(&["import", "--vault", &vault, roll.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    let lines: Vec<Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let report = &lines.last().unwrap()["report"];
    assert_eq!(report["added"], 2, "{out}");
    assert_eq!(report["skipped"], 1, "{out}");

    // Renaming needs a copy.
    let (code, _) = helios(&[
        "import",
        "--vault",
        &vault,
        "--rename",
        "{date}",
        roll.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);

    let a = roll.join("a.png");
    let b = roll.join("b.png");
    let (code, out) = helios(&[
        "stacks",
        "--vault",
        &vault,
        "create",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        "--top",
        b.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{out}");
    let stack: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(stack["members"].as_array().unwrap().len(), 2);
    let (_, out) = helios(&["stacks", "--vault", &vault, "list"]);
    let list: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let id = stack["id"].as_str().unwrap();
    let (code, _) = helios(&["stacks", "--vault", &vault, "dissolve", id]);
    assert_eq!(code, 0);
    let (_, out) = helios(&["stacks", "--vault", &vault, "list"]);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        serde_json::json!([])
    );

    // A suggestion is recorded, not applied.
    let (code, out) = helios(&["stacks", "--vault", &vault, "suggest"]);
    assert_eq!(code, 0, "{out}");
    let decision: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(decision["status"], "proposed");
    let (_, out) = helios(&["stacks", "--vault", &vault, "decisions"]);
    assert_eq!(
        serde_json::from_str::<Value>(&out)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
