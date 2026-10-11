//! Project tasks, run with `cargo xtask <task>`.
//!
//! - `changelog`: preview the next release: its version and its changelog section.
//! - `release [--date YYYY-MM-DD]`: assemble the changelog fragments into
//!   CHANGELOG.md, bump the workspace version, remove the fragments. Stops
//!   there: reviewing, committing and tagging stay a human's call.
//! - `check [--base REF]`: fragments are well formed and readable by an
//!   outsider; with `--base`, code changed since REF comes with a fragment.
//! - `i18n extract | check | stamp LOCALE [--all]`: the translation
//!   catalogs of `i18n/` (see `i18n.rs`).
//!
//! A fragment is a file in `changelog.d/`, written in the same commit as the
//! change it describes: a first line `kind: <kind>`, then one to three
//! sentences saying what changes for a user and why it matters. See
//! CONTRIBUTING.md.

mod i18n;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The kinds of change, in the order a changelog section lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Breaking,
    Added,
    Changed,
    Deprecated,
    Removed,
    Fixed,
    Security,
    /// A change with nothing to tell a user: satisfies the check, never printed.
    Internal,
}

impl Kind {
    const ALL: [Kind; 8] = [
        Kind::Breaking,
        Kind::Added,
        Kind::Changed,
        Kind::Deprecated,
        Kind::Removed,
        Kind::Fixed,
        Kind::Security,
        Kind::Internal,
    ];

    fn name(self) -> &'static str {
        match self {
            Kind::Breaking => "breaking",
            Kind::Added => "added",
            Kind::Changed => "changed",
            Kind::Deprecated => "deprecated",
            Kind::Removed => "removed",
            Kind::Fixed => "fixed",
            Kind::Security => "security",
            Kind::Internal => "internal",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Kind::Breaking => "Breaking",
            Kind::Added => "Added",
            Kind::Changed => "Changed",
            Kind::Deprecated => "Deprecated",
            Kind::Removed => "Removed",
            Kind::Fixed => "Fixed",
            Kind::Security => "Security",
            Kind::Internal => "Internal",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }

    fn bump(self) -> Bump {
        match self {
            Kind::Breaking => Bump::Major,
            Kind::Added | Kind::Changed | Kind::Deprecated | Kind::Removed => Bump::Minor,
            Kind::Fixed | Kind::Security => Bump::Patch,
            Kind::Internal => Bump::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bump {
    None,
    Patch,
    Minor,
    Major,
}

type Version = (u64, u64, u64);

fn bumped((major, minor, patch): Version, bump: Bump) -> Version {
    match bump {
        Bump::None => (major, minor, patch),
        Bump::Patch => (major, minor, patch + 1),
        // Before 1.0, a breaking change bumps the minor version.
        Bump::Major if major == 0 => (0, minor + 1, 0),
        Bump::Major => (major + 1, 0, 0),
        Bump::Minor => (major, minor + 1, 0),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Fragment {
    name: String,
    kind: Kind,
    text: String,
}

fn parse_fragment(name: &str, content: &str) -> Result<Fragment, String> {
    let mut lines = content.lines().skip_while(|l| l.trim().is_empty());
    let first = lines.next().unwrap_or("");
    let kind = first
        .strip_prefix("kind:")
        .map(str::trim)
        .ok_or_else(|| format!("{name}: the first line must be `kind: <kind>`"))?;
    let kind = Kind::parse(kind).ok_or_else(|| {
        let all: Vec<_> = Kind::ALL.iter().map(|k| k.name()).collect();
        format!("{name}: unknown kind `{kind}` (one of: {})", all.join(", "))
    })?;
    let text = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    if text.is_empty() {
        return Err(format!("{name}: no text after the kind line"));
    }
    Ok(Fragment {
        name: name.to_string(),
        kind,
        text,
    })
}

/// What an outside reader could not follow, or what is not written for a user.
fn hygiene(text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for private in ["/home/", "/Users/", "C:\\"] {
        if text.contains(private) {
            problems.push(format!("a private path (`{private}`)"));
        }
    }
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let before = if i == 0 { ' ' } else { chars[i - 1] };
        // `org/repo#123` is a public reference; a bare `#123` points to a private tracker.
        if *c == '#'
            && chars.get(i + 1).is_some_and(char::is_ascii_digit)
            && !(before.is_alphanumeric() || "/-_.".contains(before))
        {
            let id: String = chars[i..]
                .iter()
                .take_while(|c| **c == '#' || c.is_ascii_digit())
                .collect();
            problems.push(format!(
                "a tracker id (`{id}`): link to a public issue instead"
            ));
        }
    }
    if text.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.');
        Path::new(w).extension().is_some_and(|e| e == "rs")
    }) {
        problems.push("a source file name: say what changes for a user, not where".into());
    }
    if text.chars().count() > 600 {
        problems.push("more than 600 characters: one to three sentences".into());
    }
    problems
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_path_buf()
}

fn read_fragments(dir: &Path) -> Result<Vec<Fragment>, Vec<String>> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(_) => return Ok(Vec::new()),
    };
    entries.retain(|p| {
        p.extension().is_some_and(|e| e == "md") && p.file_name().is_some_and(|n| n != "README.md")
    });
    entries.sort();
    let mut fragments = Vec::new();
    let mut problems = Vec::new();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        match parse_fragment(&name, &content) {
            Ok(f) => {
                if f.kind != Kind::Internal {
                    problems.extend(hygiene(&f.text).into_iter().map(|p| format!("{name}: {p}")));
                }
                fragments.push(f);
            }
            Err(e) => problems.push(e),
        }
    }
    if problems.is_empty() {
        Ok(fragments)
    } else {
        Err(problems)
    }
}

fn read_version(cargo_toml: &str) -> Result<Version, String> {
    let mut in_package = false;
    for line in cargo_toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[workspace.package]";
        } else if in_package
            && let Some(v) = line
                .strip_prefix("version = \"")
                .and_then(|r| r.strip_suffix('"'))
        {
            let parts: Vec<u64> = v
                .split('.')
                .map(|p| p.parse().map_err(|_| format!("bad version `{v}`")))
                .collect::<Result<_, _>>()?;
            if let [major, minor, patch] = parts[..] {
                return Ok((major, minor, patch));
            }
            return Err(format!("bad version `{v}`"));
        }
    }
    Err("no version in [workspace.package]".into())
}

fn write_version(cargo_toml: &str, old: Version, new: Version) -> String {
    let from = format!("version = \"{}.{}.{}\"", old.0, old.1, old.2);
    let to = format!("version = \"{}.{}.{}\"", new.0, new.1, new.2);
    cargo_toml.replacen(&from, &to, 1)
}

fn render_section(version: Version, date: &str, fragments: &[Fragment]) -> String {
    let mut out = format!("## [{}.{}.{}] - {date}\n", version.0, version.1, version.2);
    for kind in Kind::ALL.into_iter().filter(|k| *k != Kind::Internal) {
        let entries: Vec<_> = fragments.iter().filter(|f| f.kind == kind).collect();
        if entries.is_empty() {
            continue;
        }
        let _ = write!(out, "\n### {}\n\n", kind.heading());
        for f in entries {
            let text = f
                .text
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>()
                .join("\n  ");
            let _ = writeln!(out, "- {text}");
        }
    }
    out
}

/// Puts a section above the previous releases, below the file's header.
fn insert_section(changelog: &str, section: &str) -> String {
    match changelog.find("\n## [") {
        Some(i) => format!("{}\n{section}\n{}", &changelog[..i], &changelog[i + 1..]),
        None => format!("{}\n\n{section}", changelog.trim_end()),
    }
}

/// Today's date in UTC, from the system clock.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs();
    civil((secs / 86_400) as i64)
}

/// The calendar date of a number of days since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The next release.
struct Release {
    old: Version,
    new: Version,
    section: String,
    fragments: Vec<PathBuf>,
}

/// The next release, or `None` when nothing is to tell a user.
fn plan(root: &Path, date: &str) -> Result<Option<Release>, Vec<String>> {
    let fragments = read_fragments(&root.join("changelog.d"))?;
    let bump = fragments
        .iter()
        .map(|f| f.kind.bump())
        .max()
        .unwrap_or(Bump::None);
    if bump == Bump::None {
        return Ok(None);
    }
    let cargo =
        std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| vec![e.to_string()])?;
    let old = read_version(&cargo).map_err(|e| vec![e])?;
    let new = bumped(old, bump);
    let files = fragments
        .iter()
        .map(|f| root.join("changelog.d").join(&f.name))
        .collect();
    Ok(Some(Release {
        old,
        new,
        section: render_section(new, date, &fragments),
        fragments: files,
    }))
}

fn changed_since(base: &str) -> Result<Vec<String>, String> {
    let run = |args: &[&str]| -> Result<Vec<String>, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(root())
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect())
    };
    let mut paths = run(&["diff", "--name-only", base])?;
    paths.extend(run(&["ls-files", "--others", "--exclude-standard"])?);
    Ok(paths)
}

fn missing_fragment(paths: &[String]) -> bool {
    let code = paths
        .iter()
        .any(|p| p.starts_with("crates/") || p.starts_with("modules/"));
    let fragment = paths.iter().any(|p| {
        p.starts_with("changelog.d/") && Path::new(p).extension().is_some_and(|e| e == "md")
    });
    code && !fragment
}

fn fail(problems: &[String]) -> ExitCode {
    for p in problems {
        eprintln!("xtask: {p}");
    }
    ExitCode::from(1)
}

const USAGE: &str = "usage: cargo xtask changelog | release [--date YYYY-MM-DD] | check [--base REF] | i18n extract | i18n check | i18n stamp LOCALE [--all]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = root();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["changelog"] => match plan(&root, &today()) {
            Ok(None) => {
                println!("No fragment to release in changelog.d/.");
                ExitCode::SUCCESS
            }
            Ok(Some(Release {
                old, new, section, ..
            })) => {
                println!(
                    "Next release: {}.{}.{} -> {}.{}.{}\n\n{section}",
                    old.0, old.1, old.2, new.0, new.1, new.2
                );
                ExitCode::SUCCESS
            }
            Err(p) => fail(&p),
        },
        ["release"] | ["release", "--date", _] => {
            let date = words
                .get(2)
                .map_or_else(today, std::string::ToString::to_string);
            match plan(&root, &date) {
                Ok(None) => {
                    fail(&["nothing to release: no user-facing fragment in changelog.d/".into()])
                }
                Ok(Some(Release {
                    old,
                    new,
                    section,
                    fragments: files,
                })) => {
                    let changelog =
                        std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
                    let cargo = std::fs::read_to_string(root.join("Cargo.toml"))
                        .expect("workspace manifest");
                    std::fs::write(
                        root.join("CHANGELOG.md"),
                        insert_section(&changelog, &section),
                    )
                    .expect("write CHANGELOG.md");
                    std::fs::write(root.join("Cargo.toml"), write_version(&cargo, old, new))
                        .expect("write Cargo.toml");
                    for f in files {
                        std::fs::remove_file(f).expect("remove a fragment");
                    }
                    let v = format!("{}.{}.{}", new.0, new.1, new.2);
                    println!(
                        "Prepared {v}. Run `cargo check` to refresh Cargo.lock, review the diff, then commit \"Release {v}\" and tag v{v}."
                    );
                    ExitCode::SUCCESS
                }
                Err(p) => fail(&p),
            }
        }
        ["check"] | ["check", "--base", _] => {
            let mut problems = read_fragments(&root.join("changelog.d"))
                .err()
                .unwrap_or_default();
            if let Some(base) = words.get(2) {
                match changed_since(base) {
                    Ok(paths) if missing_fragment(&paths) => problems.push(format!(
                        "code changed since {base} but no fragment in changelog.d/ (use `kind: internal` when there is nothing to tell a user)"
                    )),
                    Ok(_) => {}
                    Err(e) => problems.push(e),
                }
            }
            if problems.is_empty() {
                println!("Changelog fragments OK.");
                ExitCode::SUCCESS
            } else {
                fail(&problems)
            }
        }
        ["i18n", rest @ ..] => i18n::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", i18n::USAGE);
            ExitCode::from(2)
        }),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_parse_and_explain_errors() {
        let f = parse_fragment(
            "a.md",
            "kind: fixed\n\nExports no longer stop at the first full disk.\n",
        )
        .unwrap();
        assert_eq!(
            (f.kind, f.text.as_str()),
            (
                Kind::Fixed,
                "Exports no longer stop at the first full disk."
            )
        );
        assert_eq!(
            parse_fragment("b.md", "Fixed it").unwrap_err(),
            "b.md: the first line must be `kind: <kind>`"
        );
        assert!(
            parse_fragment("c.md", "kind: tweak\nx")
                .unwrap_err()
                .contains("unknown kind `tweak`")
        );
        assert_eq!(
            parse_fragment("d.md", "kind: added\n\n").unwrap_err(),
            "d.md: no text after the kind line"
        );
    }

    #[test]
    fn hygiene_catches_what_an_outsider_cannot_follow() {
        assert!(hygiene("Ratings no longer get lost (darktable-org/darktable#22569).").is_empty());
        assert_eq!(hygiene("Fixes #42.").len(), 1);
        assert_eq!(hygiene("See /home/me/notes.").len(), 1);
        assert_eq!(hygiene("Changed engine.rs to cache.").len(), 1);
        assert!(
            hygiene("Settings live in helios.toml.").is_empty(),
            "a user-facing file name is fine"
        );
        assert_eq!(hygiene(&"word ".repeat(150)).len(), 1);
    }

    #[test]
    fn the_bump_follows_the_strongest_kind() {
        assert_eq!(bumped((0, 0, 1), Kind::Added.bump()), (0, 1, 0));
        assert_eq!(bumped((0, 3, 2), Kind::Fixed.bump()), (0, 3, 3));
        assert_eq!(
            bumped((0, 3, 2), Kind::Breaking.bump()),
            (0, 4, 0),
            "breaking bumps minor before 1.0"
        );
        assert_eq!(bumped((1, 3, 2), Kind::Breaking.bump()), (2, 0, 0));
        assert_eq!(bumped((1, 3, 2), Kind::Internal.bump()), (1, 3, 2));
    }

    #[test]
    fn a_section_groups_by_kind_and_skips_internal() {
        let fragments = vec![
            Fragment {
                name: "b.md".into(),
                kind: Kind::Fixed,
                text: "A fix.".into(),
            },
            Fragment {
                name: "a.md".into(),
                kind: Kind::Added,
                text: "A feature,\non two lines.".into(),
            },
            Fragment {
                name: "c.md".into(),
                kind: Kind::Internal,
                text: "Refactor.".into(),
            },
        ];
        assert_eq!(
            render_section((0, 1, 0), "2026-10-11", &fragments),
            "## [0.1.0] - 2026-10-11\n\n### Added\n\n- A feature,\n  on two lines.\n\n### Fixed\n\n- A fix.\n"
        );
    }

    #[test]
    fn a_section_goes_above_previous_releases() {
        let log = "# Changelog\n\nHeader.\n\n## [0.1.0] - 2026-01-01\n\n- old\n";
        let out = insert_section(log, "## [0.2.0] - 2026-02-01\n\n- new\n");
        assert_eq!(
            out,
            "# Changelog\n\nHeader.\n\n## [0.2.0] - 2026-02-01\n\n- new\n\n## [0.1.0] - 2026-01-01\n\n- old\n"
        );
        assert_eq!(
            insert_section("# Changelog\n", "## [0.1.0] - x\n"),
            "# Changelog\n\n## [0.1.0] - x\n"
        );
    }

    #[test]
    fn the_workspace_version_is_read_and_written() {
        let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.0.1\"\nedition = \"2024\"\n";
        assert_eq!(read_version(toml), Ok((0, 0, 1)));
        assert!(write_version(toml, (0, 0, 1), (0, 1, 0)).contains("version = \"0.1.0\""));
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(19_723), "2024-01-01");
        assert_eq!(civil(11_016), "2000-02-29");
    }

    #[test]
    fn code_without_a_fragment_is_flagged() {
        let p = |v: &[&str]| {
            v.iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        };
        assert!(missing_fragment(&p(&["crates/helios-core/src/engine.rs"])));
        assert!(!missing_fragment(&p(&[
            "crates/helios-core/src/engine.rs",
            "changelog.d/cache.md"
        ])));
        assert!(!missing_fragment(&p(&["docs/gui.md"])));
    }
}
