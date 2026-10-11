//! `cargo xtask i18n …`: the catalogs of `i18n/`.
//!
//! - `extract`: writes `i18n/en/*.ftl` from Helios's declarations. They
//!   are gathered by `helios-declarations` (a headless desktop shell and
//!   the module registry), run as a separate binary so xtask does not
//!   build the GUI stack for its other tasks.
//! - `check`: per locale, coverage and missing, orphan, changed and
//!   unstamped keys; fails on faults only (a file that does not parse, a
//!   key defined twice, no English).
//! - `stamp LOCALE [--all]`: records the English hash of newly translated
//!   keys (of every translated key with `--all`, once the changed ones were
//!   reviewed) in `i18n/LOCALE/english.hashes`.

use std::path::Path;
use std::process::{Command, ExitCode};

use helios_i18n::Declarations;
use helios_i18n::check::{self, Catalogs};

use crate::fail;

pub const USAGE: &str = "i18n extract | i18n check | i18n stamp LOCALE [--all]";

pub fn run(root: &Path, words: &[&str]) -> Option<ExitCode> {
    let dir = root.join("i18n");
    Some(match words {
        ["extract"] => match extract(root, &dir) {
            Ok(n) => {
                println!("Wrote {n} English catalog files in i18n/en.");
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

fn extract(root: &Path, dir: &Path) -> Result<usize, String> {
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
    Ok(files.len())
}
