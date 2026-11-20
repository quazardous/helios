//! `cargo xtask i18n …`: the catalogues of `i18n/`, through egui-pop's
//! translation library (`egui-pop-i18n`: its extractor and its checks).
//!
//! - `extract`: writes `i18n/en/*.ftl` from Helios's declarations. They
//!   are gathered by `helios-i18n-extract` (a headless desktop and the
//!   module registry), run as a separate binary so xtask does not build
//!   the window's stack for its other tasks. Then, per locale, the keys
//!   missing from it and the keys it has that English no longer has.
//! - `check`: per locale, coverage and missing, orphan, changed and
//!   unstamped keys; fails on faults only (a file that does not parse, a
//!   key defined twice, no English).
//! - `check --new-since REF`: the same, and fails too on the gaps a change
//!   adds since its merge base with REF: an English key a locale lacks,
//!   whose English changed since it was translated, or that it translates
//!   unstamped. The gaps the base already had are the catch-up's backlog,
//!   not the change's, and do not fail it. A reworded English key fails
//!   like a new one: its translation still says the old text, so it is
//!   reviewed (then `stamp LOCALE --all`) in the same change. pr-lint runs
//!   it, so a pull request that adds English adds its French.
//! - `stamp LOCALE [--all]`: records the English hash of newly translated
//!   keys (of every translated key with `--all`, once the changed ones were
//!   reviewed) in `i18n/LOCALE/english.hashes`.

use std::path::Path;
use std::process::{Command, ExitCode};

use egui_pop_i18n::check::{self, Catalogs};
use egui_pop_i18n::extract;
use helios_i18n::Declarations;

use crate::fail;

pub const USAGE: &str = "i18n extract | i18n check [--new-since REF] | i18n stamp LOCALE [--all]";

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
        ["check", "--new-since", base] => match check_new_since(root, &dir, base) {
            Ok(gaps) if gaps.is_empty() => ExitCode::SUCCESS,
            Ok(gaps) => {
                eprintln!(
                    "\nThis change leaves {} translation gap(s) its base did not have.\n\
                     A change that adds or rewords English translates it too: write the \
                     keys in i18n/<locale>/, then `cargo xtask i18n stamp <locale>` \
                     (`--all` once reworded keys were reviewed).",
                    gaps.len()
                );
                for g in &gaps {
                    eprintln!("  {g}");
                }
                ExitCode::from(1)
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

/// One gap a change adds to a locale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub locale: String,
    /// `missing`, `changed` or `unstamped`.
    pub what: &'static str,
    pub key: String,
}

impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} {}", self.locale, self.what, self.key)
    }
}

/// The gaps of a report, per locale: missing, changed and unstamped keys.
fn gaps(report: &check::Report) -> Vec<Gap> {
    let mut out = Vec::new();
    for l in &report.locales {
        for (what, keys) in [
            ("missing", &l.missing),
            ("changed", &l.changed),
            ("unstamped", &l.unstamped),
        ] {
            out.extend(keys.iter().map(|k| Gap {
                locale: l.locale.clone(),
                what,
                key: k.clone(),
            }));
        }
    }
    out
}

/// The gaps `head` has and `base` has not: what a change adds.
pub fn new_gaps(base: &Catalogs, head: &Catalogs) -> Vec<Gap> {
    let before = gaps(&check::check(base));
    crate::delta::added(&before, &gaps(&check::check(head)), ToString::to_string)
}

/// `check --new-since REF`: the report of the working tree, then the gaps
/// it adds since its merge base with `base`. Faults are errors.
fn check_new_since(root: &Path, dir: &Path, base: &str) -> Result<Vec<Gap>, String> {
    let head = Catalogs::read(dir).map_err(|e| format!("i18n/: {e}"))?;
    let report = check::check(&head);
    print!("{report}");
    if !report.faults.is_empty() {
        return Err(format!("i18n/ has {} fault(s)", report.faults.len()));
    }
    let rev = crate::delta::merge_base(root, base)?;
    let tree = crate::delta::BaseTree::extract(root, &rev, &["i18n"])?;
    let before = match Catalogs::read(&tree.dir.join("i18n")) {
        Ok(c) => c,
        // No catalogues at the base: everything is new.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Catalogs::default(),
        Err(e) => return Err(format!("i18n/ at {rev}: {e}")),
    };
    let added = new_gaps(&before, &head);
    println!(
        "new since {base} ({}): {} gap(s)",
        rev.chars().take(12).collect::<String>(),
        added.len()
    );
    Ok(added)
}

fn extract_english(root: &Path, dir: &Path) -> Result<usize, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["run", "--quiet", "--package", "helios-i18n-extract"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "helios-i18n-extract failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let declarations: Declarations = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("helios-i18n-extract' output: {e}"))?;
    let files = declarations.english_files()?;
    let en = dir.join("en");
    std::fs::create_dir_all(&en).map_err(|e| e.to_string())?;
    for (name, text) in &files {
        std::fs::write(en.join(name), text).map_err(|e| format!("i18n/en/{name}: {e}"))?;
    }
    // What each translation lacks, and keeps that English dropped.
    let english = declarations.found();
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_pop_i18n::check::File;

    /// English and French sources, French stamped from the English of
    /// `stamped_from` (none: unstamped).
    fn catalogs(en: &str, fr: &str, stamped_from: Option<&str>) -> Catalogs {
        let file = |locale: &str, source: &str| File {
            locale: locale.into(),
            name: "a.ftl".into(),
            source: source.into(),
        };
        let at = |en: &str| Catalogs {
            files: vec![file("en", en), file("fr", fr)],
            ..Catalogs::default()
        };
        let mut c = at(en);
        if let Some(from) = stamped_from {
            let hashes = check::stamp(&at(from), "fr", true).unwrap();
            c.hashes.insert("fr".into(), hashes);
        }
        c
    }

    fn keys(gaps: &[Gap]) -> Vec<String> {
        gaps.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn i18n_new_english_without_french_is_a_gap() {
        let base = catalogs("a = A\n", "a = Á\n", Some("a = A\n"));
        let head = catalogs("a = A\nb = B\n", "a = Á\n", Some("a = A\n"));
        assert_eq!(keys(&new_gaps(&base, &head)), ["fr: missing b"]);
    }

    #[test]
    fn i18n_the_base_backlog_is_not_the_change_gap() {
        // `b` lacked French before the change: the catch-up's, not its.
        let base = catalogs("a = A\nb = B\n", "a = Á\n", Some("a = A\n"));
        let head = catalogs("a = A\nb = B2\n", "a = Á\n", Some("a = A\n"));
        assert!(new_gaps(&base, &head).is_empty());
    }

    #[test]
    fn i18n_new_english_with_stamped_french_passes() {
        let en = "a = A\nb = B\n";
        let base = catalogs("a = A\n", "a = Á\n", Some("a = A\n"));
        let head = catalogs(en, "a = Á\nb = B́\n", Some(en));
        assert!(new_gaps(&base, &head).is_empty());
    }

    #[test]
    fn i18n_reworded_or_unstamped_french_is_a_gap() {
        let base = catalogs("a = A\n", "a = Á\n", Some("a = A\n"));
        let reworded = catalogs("a = A2\n", "a = Á\n", Some("a = A\n"));
        assert_eq!(keys(&new_gaps(&base, &reworded)), ["fr: changed a"]);
        let unstamped = catalogs("a = A\nb = B\n", "a = Á\nb = B́\n", Some("a = A\n"));
        assert_eq!(keys(&new_gaps(&base, &unstamped)), ["fr: unstamped b"]);
    }

    #[test]
    fn i18n_a_removed_french_entry_is_a_gap() {
        let base = catalogs("a = A\n", "a = Á\n", Some("a = A\n"));
        let head = catalogs("a = A\n", "", None);
        assert_eq!(keys(&new_gaps(&base, &head)), ["fr: missing a"]);
    }
}
