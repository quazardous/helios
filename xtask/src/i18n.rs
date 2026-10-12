//! `cargo xtask i18n …`: the catalogues of `i18n/`, through egui-pop's
//! translation library (`egui-pop-i18n`: its extractor and its checks).
//!
//! - `extract`: writes `i18n/en/*.ftl` from Helios's declarations. They
//!   are gathered by `helios-declarations` (a headless desktop and the
//!   module registry), run as a separate binary so xtask does not build
//!   the window's stack for its other tasks. Then, per locale, the keys
//!   missing from it and the keys it has that English no longer has.
//! - `check`: per locale, coverage and missing, orphan, changed and
//!   unstamped keys; fails on faults only (a file that does not parse, a
//!   key defined twice, no English).
//! - `stamp LOCALE [--all]`: records the English hash of newly translated
//!   keys (of every translated key with `--all`, once the changed ones were
//!   reviewed) in `i18n/LOCALE/english.hashes`.

use std::path::Path;
use std::process::{Command, ExitCode};

use egui_pop_i18n::check::{self, Catalogs};
use egui_pop_i18n::extract;
use helios_i18n::Declarations;

use crate::fail;

pub const USAGE: &str = "i18n extract | i18n check | i18n stamp LOCALE [--all]";

pub fn run(root: &Path, words: &[&str]) -> Option<ExitCode> {
    let dir = root.join("i18n");
    Some(match words {
        ["extract"] => match extract_english(root, &dir) {
            Ok(n) => {
                println!("Wrote {n} English catalogue files in i18n/en.");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&[e]),
        },
        ["check"] => match Catalogs::read(&dir) {
            Ok(c) => {
                let report = check::check(&c);
                print!("{report}");
                if report.faults.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                }
            }
            Err(e) => fail(&[format!("i18n/: {e}")]),
        },
        ["stamp", locale] | ["stamp", locale, "--all"] => {
            let all = words.len() == 3;
            let result = Catalogs::read(&dir)
                .map_err(|e| format!("i18n/: {e}"))
                .and_then(|c| check::stamp(&c, locale, all))
                .and_then(|hashes| {
                    std::fs::write(
                        dir.join(locale).join(check::HASHES),
                        check::write_hashes(&hashes),
                    )
                    .map(|()| hashes.len())
                    .map_err(|e| e.to_string())
                });
            match result {
                Ok(n) => {
                    println!("{n} keys stamped in i18n/{locale}/{}.", check::HASHES);
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&[e]),
            }
        }
        _ => return None,
    })
}

fn extract_english(root: &Path, dir: &Path) -> Result<usize, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["run", "--quiet", "--package", "helios-declarations"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "helios-declarations failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let declarations: Declarations = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("helios-declarations' output: {e}"))?;
    let files = declarations.english_files()?;
    let en = dir.join("en");
    std::fs::create_dir_all(&en).map_err(|e| e.to_string())?;
    for (name, text) in &files {
        std::fs::write(en.join(name), text).map_err(|e| format!("i18n/en/{name}: {e}"))?;
    }
    // What each translation lacks, and keeps that English dropped.
    let english: Vec<extract::Found> = declarations
        .english()
        .into_iter()
        .map(|(id, text)| extract::Found {
            key: id.key(),
            english: extract::English::Plain(text),
            origin: String::new(),
        })
        .chain(
            declarations
                .interface
                .iter()
                .map(|(key, text)| extract::Found {
                    key: key.clone(),
                    english: extract::English::Plain(text.clone()),
                    origin: String::new(),
                }),
        )
        .filter(|f| !matches!(&f.english, extract::English::Plain(t) if t.trim().is_empty()))
        .collect();
    let messages = extract::messages(&english)?;
    let catalogs = Catalogs::read(dir).map_err(|e| format!("i18n/: {e}"))?;
    for l in extract::report(&catalogs, &messages) {
        if l.locale != "en" {
            println!(
                "{}: {} missing, {} unused{}",
                l.locale,
                l.missing.len(),
                l.unused.len(),
                if l.unused.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", l.unused.join(", "))
                }
            );
        }
    }
    Ok(files.len())
}
