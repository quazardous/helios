//! Project tasks, run with `cargo xtask <task>`.
//!
//! - `changelog`: preview the next release: its version and its changelog section.
//! - `release [--date YYYY-MM-DD]`: assemble the changelog fragments into
//!   CHANGELOG.md, bump the workspace version, remove the fragments. Stops
//!   there: reviewing, committing and tagging stay a human's call.
//! - `gate <PR>…`: the tester's gate: pull requests merged on a temporary
//!   branch from `next`, their manifests run on the box, `next` fast-forwarded
//!   when green (see `gate.rs`).
//! - `manifest check [FILE|-]`: the test manifest of a pull request into
//!   `devel` (see `manifest.rs`).
//! - `promote <SHA> [--remote NAME]`: what `promote.yml` runs once CI passed
//!   on a commit of `next`: the release of its fragments committed on top
//!   of it, then `main` (and `next`) moved to that (see `promote.rs`).
//! - `check [--base REF]`: fragments are well formed and readable by an
//!   outsider; with `--base`, code changed since REF comes with a fragment.
//!   The in-tree plugins' Helios range and the workspace's own Cargo.lock
//!   lines follow the workspace version, so a branch that lands after a
//!   version bump fails its own CI rather than `--locked` builds on `next`.
//!   Only the vault driver and the node open the vault file or a raw SQLite
//!   connection (see `vault_access.rs`).
//! - `i18n extract | check | stamp LOCALE [--all]`: the translation
//!   catalogs of `i18n/` (see `i18n.rs`).
//! - `lint [--fix] [--require-all]`: rustfmt, clippy and the optional
//!   linters that are installed (see `lint.rs`).
//! - `bench masks [--modules N] ...`: the masks bench, one JSON line per
//!   render mode, recorded in `target/bench/masks.jsonl` (see `bench.rs`).
//! - `test-times [--profile P] [--top N]`: run the tests under
//!   cargo-nextest, record their durations and show the slowest and the
//!   ones getting slower (see `test_times.rs`).
//! - `check-commits [--full] [RANGE]`: every commit of a series, but its
//!   head, builds, passes clippy, the tests it touches (every fast test
//!   with `--full`) and `check` (see `commits.rs`).
//! - `bench-remote <job> [key=value...] [--push]`: a job of
//!   `tools/bench/Benchfile` on the bench box, its results fetched to
//!   `target/bench-remote/<id>/`; exit 75 when the box cannot serve it
//!   (see `remote.rs` and docs/bench.md); `bench-remote --affected
//!   [base=REF]` runs the jobs `affected` selects; `bench-remote --chain
//!   egui-pop|helios` a branch's usual sequence; `bench-remote queue`
//!   prints the box's queue.
//! - `affected [BASE] [--filter | --critical]`: the crates, tests and box
//!   jobs a change since BASE touches (see `affected.rs`).
//! - `install-dev [--branch B | --local] | --list | --uninstall`: a dev
//!   build (the box's latest of a branch, or this checkout's) installed
//!   outside the checkout with its "Helios Dev" launcher (see
//!   `install_dev.rs`).
//!
//! A fragment is a file in `changelog.d/`, written in the same commit as the
//! change it describes: a first line `kind: <kind>`, an optional line
//! `release: <false|patch|minor|major>` (does this change ship a release),
//! then one to three sentences saying what changes for a user and why it
//! matters. See CONTRIBUTING.md.

mod affected;
mod bench;
mod commits;
mod gate;
mod i18n;
mod install_dev;
mod lint;
mod manifest;
mod promote;
mod remote;
mod test_times;
mod vault_access;

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

impl Bump {
    /// The words a fragment's `release:` line takes, `false` for none.
    fn parse_release(s: &str) -> Option<Bump> {
        match s {
            "false" => Some(Bump::None),
            "patch" => Some(Bump::Patch),
            "minor" => Some(Bump::Minor),
            "major" => Some(Bump::Major),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Bump::None => "false",
            Bump::Patch => "patch",
            Bump::Minor => "minor",
            Bump::Major => "major",
        }
    }
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
    /// The release this change asks for: `Bump::None` (`release: false`,
    /// the default) waits for another fragment to ask for one.
    release: Bump,
    text: String,
}

fn parse_fragment(name: &str, content: &str) -> Result<Fragment, String> {
    let mut lines = content
        .lines()
        .skip_while(|l| l.trim().is_empty())
        .peekable();
    let first = lines.next().unwrap_or("");
    let kind = first
        .strip_prefix("kind:")
        .map(str::trim)
        .ok_or_else(|| format!("{name}: the first line must be `kind: <kind>`"))?;
    let kind = Kind::parse(kind).ok_or_else(|| {
        let all: Vec<_> = Kind::ALL.iter().map(|k| k.name()).collect();
        format!("{name}: unknown kind `{kind}` (one of: {})", all.join(", "))
    })?;
    let mut release = Bump::None;
    if let Some(word) = lines
        .peek()
        .and_then(|l| l.strip_prefix("release:"))
        .map(str::trim)
    {
        release = Bump::parse_release(word).ok_or_else(|| {
            format!("{name}: unknown release `{word}` (one of: false, patch, minor, major)")
        })?;
        lines.next();
        // A release asked below what the change calls for would hide it.
        if release != Bump::None && release < kind.bump() {
            return Err(format!(
                "{name}: `release: {}` is below what a `{}` change calls for (`{}`)",
                release.name(),
                kind.name(),
                kind.bump().name()
            ));
        }
    }
    let text = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    if text.is_empty() {
        return Err(format!("{name}: no text after the kind line"));
    }
    Ok(Fragment {
        name: name.to_string(),
        kind,
        release,
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

/// The Helios versions an in-tree plugin works with: the release's own
/// compatible range (before 1.0 a minor bump breaks, so `>=0.Y.0, <0.Y+1.0`).
fn plugin_range(v: Version) -> String {
    if v.0 == 0 {
        format!(">=0.{}.0, <0.{}.0", v.1, v.1 + 1)
    } else {
        format!(">={}.0.0, <{}.0.0", v.0, v.0 + 1)
    }
}

/// A plugin manifest whose `helios = "..."` line follows the new release.
fn write_plugin_range(manifest: &str, new: Version) -> String {
    manifest
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("helios = \"") {
                format!("helios = \"{}\"", plugin_range(new))
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if manifest.ends_with('\n') { "\n" } else { "" }
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

/// Helios's `AppStream` metadata (packaging/flatpak, the tarball).
const METAINFO: &str = "packaging/linux/io.github.quazardous.Helios.metainfo.xml";

/// The metainfo with a `<release>` for `version` at the top of
/// `<releases>`, unless it lists that version already.
fn insert_metainfo_release(metainfo: &str, version: &str, date: &str) -> String {
    if metainfo.contains(&format!("<release version=\"{version}\"")) {
        return metainfo.to_string();
    }
    let entry = format!(
        "    <release version=\"{version}\" date=\"{date}\">\n      <url type=\"details\">https://github.com/quazardous/helios/releases/tag/v{version}</url>\n    </release>\n"
    );
    match metainfo.find("<releases>\n") {
        Some(i) => {
            let at = i + "<releases>\n".len();
            format!("{}{entry}{}", &metainfo[..at], &metainfo[at..])
        }
        None => metainfo.to_string(),
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

/// When the pending fragments make a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    /// Only when a fragment asks for one (`release:` other than `false`):
    /// a promotion.
    Asked,
    /// Whenever a fragment has something to tell a user: `cargo xtask
    /// release` run by hand, and the promotion's fallback for fragments
    /// left waiting too long.
    Always,
}

/// The next release, or `None` when there is none to make. Its bump is the
/// strongest of what the pending fragments' kinds call for and what they
/// ask for: a release takes every pending fragment.
fn plan(root: &Path, date: &str, gate: Gate) -> Result<Option<Release>, Vec<String>> {
    let fragments = read_fragments(&root.join("changelog.d"))?;
    let asked = fragments
        .iter()
        .map(|f| f.release)
        .max()
        .unwrap_or(Bump::None);
    if gate == Gate::Asked && asked == Bump::None {
        return Ok(None);
    }
    let bump = fragments
        .iter()
        .map(|f| f.kind.bump())
        .max()
        .unwrap_or(Bump::None)
        .max(asked);
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

/// Writes a planned release into the tree at `root`: the CHANGELOG.md
/// section, the workspace version, the in-tree plugins' Helios range and
/// the `AppStream` metadata; removes the fragments released. Returns the
/// new version. Cargo.lock is left to cargo to refresh.
fn apply_release(root: &Path, date: &str, release: Release) -> String {
    let Release {
        old,
        new,
        section,
        fragments: files,
    } = release;
    let changelog = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest");
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
    // the in-tree plugins (examples, and the importers Helios ships)
    // follow the release, or the next build refuses them as made for
    // another Helios
    for manifest in plugin_manifests(root) {
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            std::fs::write(&manifest, write_plugin_range(&text, new))
                .expect("write a plugin manifest");
        }
    }
    let v = format!("{}.{}.{}", new.0, new.1, new.2);
    // the AppStream metadata lists the release, for the software centres
    // and Flathub
    let metainfo = root.join(METAINFO);
    let text = std::fs::read_to_string(&metainfo).expect("the metainfo");
    std::fs::write(&metainfo, insert_metainfo_release(&text, &v, date))
        .expect("write the metainfo");
    v
}

/// The folders of the in-tree plugins whose manifest follows the release.
const PLUGIN_DIRS: [&str; 3] = ["plugins/examples", "plugins/importers", "plugins/sources"];

/// The manifests (`helios-plugin.toml`) of the in-tree plugins.
fn plugin_manifests(root: &Path) -> Vec<PathBuf> {
    PLUGIN_DIRS
        .iter()
        .flat_map(|d| std::fs::read_dir(root.join(d)).into_iter().flatten())
        .flatten()
        .map(|e| e.path().join("helios-plugin.toml"))
        .filter(|p| p.is_file())
        .collect()
}

/// Why an in-tree plugin manifest's Helios range is not the one the version
/// bump writes for `v` (`write_plugin_range`, so the check and the bump agree).
fn range_problem(path: &str, manifest: &str, v: Version) -> Option<String> {
    (write_plugin_range(manifest, v) != manifest).then(|| {
        format!(
            "{path}: its Helios range does not follow version {}.{}.{}: set `helios = \"{}\"` (what the version bump writes)",
            v.0,
            v.1,
            v.2,
            plugin_range(v)
        )
    })
}

/// The Cargo.lock lines of workspace members (`(name, version)`, from their
/// manifests) that lag behind: what `cargo update --workspace` would change.
fn lock_problems(lock: &str, members: &[(String, String)]) -> Vec<String> {
    let mut locked = std::collections::HashMap::new();
    for block in lock.split("\n\n") {
        // a package of the workspace has no `source`
        if !block.starts_with("[[package]]") || block.lines().any(|l| l.starts_with("source = ")) {
            continue;
        }
        let field = |key: &str| {
            block
                .lines()
                .find_map(|l| l.strip_prefix(key))
                .map(|v| v.trim_matches('"'))
        };
        if let (Some(name), Some(version)) = (field("name = "), field("version = ")) {
            locked.insert(name, version);
        }
    }
    members
        .iter()
        .filter_map(|(name, version)| match locked.get(name.as_str()) {
            Some(l) if l == version => None,
            Some(l) => Some(format!(
                "Cargo.lock: `{name}` is locked at {l} but its manifest says {version}: run `cargo update --workspace` and commit (or `git add`) Cargo.lock"
            )),
            None => Some(format!(
                "Cargo.lock: no line for `{name}`: run `cargo update --workspace` and commit (or `git add`) Cargo.lock"
            )),
        })
        .collect()
}

/// The plugin ranges and Cargo.lock lines that do not follow the workspace
/// version (see `range_problem` and `lock_problems`).
fn version_problems(root: &Path) -> Vec<String> {
    let cargo_toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    let v = match read_version(&cargo_toml) {
        Ok(v) => v,
        Err(e) => return vec![format!("Cargo.toml: {e}")],
    };
    let mut problems: Vec<String> = plugin_manifests(root)
        .iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            let path = p.strip_prefix(root).unwrap_or(p).to_string_lossy();
            range_problem(&path, &text, v)
        })
        .collect();
    match workspace_members(root) {
        Ok(members) => {
            problems.extend(lock_problems(&staged_lock(root), &members));
        }
        Err(e) => problems.push(e),
    }
    problems
}

/// Cargo.lock as git holds it (the index: what a commit takes), else the
/// file. Not the file first: any cargo command run without `--locked`,
/// `cargo xtask` itself included, rewrites a stale lock on disk before the
/// check could see it, while the commit still carries the stale one.
fn staged_lock(root: &Path) -> String {
    Command::new("git")
        .args(["show", ":Cargo.lock"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || std::fs::read_to_string(root.join("Cargo.lock")).unwrap_or_default(),
            |o| String::from_utf8_lossy(&o.stdout).into_owned(),
        )
}

/// The workspace members' names and versions, as `cargo metadata` resolves
/// them (`version.workspace = true` included), without touching Cargo.lock.
fn workspace_members(root: &Path) -> Result<Vec<(String, String)>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    Ok(meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((
                p["name"].as_str()?.to_string(),
                p["version"].as_str()?.to_string(),
            ))
        })
        .collect())
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

/// The survival scenarios (quit, `kill -9` and reopen: the edit, the
/// ratings and the labels are where the last saved step left them) gate a
/// release: see docs/persistence.md.
fn survives(root: &Path) -> bool {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .arg("survival")
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

fn fail(problems: &[String]) -> ExitCode {
    for p in problems {
        eprintln!("xtask: {p}");
    }
    ExitCode::from(1)
}

const USAGE: &str = "usage: cargo xtask changelog | release [--date YYYY-MM-DD] | check [--base REF] | promote <SHA> [--remote NAME] | lint [--fix] [--require-all] | test-times [--profile P] [--top N] [--from-junit FILE [--summary]] | check-commits [--full] [BASE..HEAD | BASE] | bench masks [--modules N] [--budget MIB] | bench-remote <job> [key=value...] [--priority high|normal] [--push] | bench-remote --affected [base=REF] [key=value...] [--push] | bench-remote --chain egui-pop|helios [--release] [key=value...] [--priority high|normal] [--push] | bench-remote queue | affected [BASE] [--filter | --critical] | i18n extract | i18n check | i18n stamp LOCALE [--all] | install-dev [--branch B | --local] | install-dev --list | install-dev --uninstall";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = root();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["changelog"] => match plan(&root, &today(), Gate::Always) {
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
                if matches!(plan(&root, &today(), Gate::Asked), Ok(None)) {
                    println!(
                        "No fragment asks for a release (`release:`): the next promotion makes none."
                    );
                }
                ExitCode::SUCCESS
            }
            Err(p) => fail(&p),
        },
        ["release"] | ["release", "--date", _] => {
            let date = words
                .get(2)
                .map_or_else(today, std::string::ToString::to_string);
            if !survives(&root) {
                return fail(
                    &["the survival scenarios fail (`cargo survival`): no release".into()],
                );
            }
            match plan(&root, &date, Gate::Always) {
                Ok(None) => {
                    fail(&["nothing to release: no user-facing fragment in changelog.d/".into()])
                }
                Ok(Some(release)) => {
                    let v = apply_release(&root, &date, release);
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
            problems.extend(version_problems(&root));
            problems.extend(vault_access::problems(&root));
            if problems.is_empty() {
                println!("Changelog fragments, plugin ranges, Cargo.lock and vault access OK.");
                ExitCode::SUCCESS
            } else {
                fail(&problems)
            }
        }
        ["gate", rest @ ..] => gate::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", gate::USAGE);
            ExitCode::from(2)
        }),
        ["manifest", rest @ ..] => manifest::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", manifest::USAGE);
            ExitCode::from(2)
        }),
        ["promote", rest @ ..] => promote::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", promote::USAGE);
            ExitCode::from(2)
        }),
        ["lint", rest @ ..] => lint::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", lint::USAGE);
            ExitCode::from(2)
        }),
        ["test-times", rest @ ..] => test_times::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", test_times::USAGE);
            ExitCode::from(2)
        }),
        ["check-commits", rest @ ..] => commits::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", commits::USAGE);
            ExitCode::from(2)
        }),
        ["bench", rest @ ..] => bench::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", bench::USAGE);
            ExitCode::from(2)
        }),
        ["bench-remote", rest @ ..] => remote::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", remote::USAGE);
            ExitCode::from(2)
        }),
        ["affected", rest @ ..] => affected::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", affected::USAGE);
            ExitCode::from(2)
        }),
        ["install-dev", rest @ ..] => install_dev::run(&root, rest).unwrap_or_else(|| {
            eprintln!("usage: cargo xtask {}", install_dev::USAGE);
            ExitCode::from(2)
        }),
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
    fn in_tree_plugins_follow_the_release() {
        assert_eq!(plugin_range((0, 2, 0)), ">=0.2.0, <0.3.0");
        assert_eq!(plugin_range((1, 4, 2)), ">=1.0.0, <2.0.0");
        let manifest = "id = \"x\"\n\n[requires]\nhelios = \">=0.0.1, <0.1.0\"\n";
        assert_eq!(
            write_plugin_range(manifest, (0, 1, 0)),
            "id = \"x\"\n\n[requires]\nhelios = \">=0.1.0, <0.2.0\"\n"
        );
    }

    #[test]
    fn plugin_ranges_follow_the_workspace_version() {
        let manifest = "id = \"x\"\n\n[compat]\nhelios = \">=0.78.0, <0.79.0\"\n";
        assert_eq!(range_problem("x.toml", manifest, (0, 78, 2)), None);
        let stale = manifest.replace("0.78.0, <0.79.0", "0.77.0, <0.78.0");
        let problem = range_problem("x.toml", &stale, (0, 78, 0)).expect("a stale range");
        assert!(problem.starts_with("x.toml: "), "{problem}");
        assert!(
            problem.contains("`helios = \">=0.78.0, <0.79.0\"`"),
            "{problem}"
        );
    }

    #[test]
    fn lock_lines_follow_the_members() {
        let lock = "version = 4\n\n[[package]]\nname = \"helios-a\"\nversion = \"0.78.0\"\n\n[[package]]\nname = \"helios-b\"\nversion = \"0.77.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"0.77.0\"\nsource = \"registry+https://example.org/index\"\n";
        let member = |n: &str, v: &str| (n.to_string(), v.to_string());
        assert!(lock_problems(lock, &[member("helios-a", "0.78.0")]).is_empty());
        let stale = lock_problems(
            lock,
            &[member("helios-a", "0.78.0"), member("helios-b", "0.78.0")],
        );
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert!(stale[0].contains("`helios-b` is locked at 0.77.0 but its manifest says 0.78.0"));
        assert!(stale[0].contains("cargo update --workspace"));
        // a registry package of the same name is not the member's line
        assert_eq!(lock_problems(lock, &[member("serde", "0.78.0")]).len(), 1);
    }

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
    fn a_fragment_may_ask_for_a_release_no_lower_than_its_change() {
        let f = parse_fragment("a.md", "kind: fixed\n\nA fix.\n").unwrap();
        assert_eq!(f.release, Bump::None, "no release asked by default");
        let f = parse_fragment("b.md", "kind: added\nrelease: minor\n\nA panel.\n").unwrap();
        assert_eq!((f.release, f.text.as_str()), (Bump::Minor, "A panel."));
        let f = parse_fragment("c.md", "kind: fixed\nrelease: false\n\nA fix.\n").unwrap();
        assert_eq!(f.release, Bump::None);
        let f = parse_fragment("d.md", "kind: internal\nrelease: patch\n\nShip.\n").unwrap();
        assert_eq!(
            f.release,
            Bump::Patch,
            "an internal line may ship what waits"
        );
        assert_eq!(
            parse_fragment("e.md", "kind: added\nrelease: patch\n\nA panel.\n").unwrap_err(),
            "e.md: `release: patch` is below what a `added` change calls for (`minor`)"
        );
        assert!(
            parse_fragment("f.md", "kind: fixed\nrelease: yes\n\nA fix.\n")
                .unwrap_err()
                .contains("unknown release `yes`")
        );
    }

    #[test]
    fn a_promotion_releases_only_when_a_fragment_asks_and_takes_them_all() {
        let dir = std::env::temp_dir().join(format!("xtask-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("changelog.d")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace.package]\nversion = \"0.3.2\"\n",
        )
        .unwrap();
        let add = |name: &str, text: &str| {
            std::fs::write(dir.join("changelog.d").join(name), text).unwrap();
        };
        add("a.md", "kind: added\n\nA panel.\n");
        assert!(plan(&dir, "2026-11-13", Gate::Asked).unwrap().is_none());
        assert_eq!(
            plan(&dir, "2026-11-13", Gate::Always).unwrap().unwrap().new,
            (0, 4, 0)
        );
        add("b.md", "kind: fixed\nrelease: patch\n\nA fix.\n");
        let r = plan(&dir, "2026-11-13", Gate::Asked).unwrap().unwrap();
        assert_eq!(r.new, (0, 4, 0), "the kinds still decide when stronger");
        assert_eq!(r.fragments.len(), 2, "every pending fragment is released");
        add("c.md", "kind: internal\nrelease: major\n\nShip it.\n");
        assert_eq!(
            plan(&dir, "2026-11-13", Gate::Asked).unwrap().unwrap().new,
            (0, 4, 0),
            "major bumps the minor before 1.0"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
                release: Bump::None,
                text: "A fix.".into(),
            },
            Fragment {
                name: "a.md".into(),
                kind: Kind::Added,
                release: Bump::None,
                text: "A feature,\non two lines.".into(),
            },
            Fragment {
                name: "c.md".into(),
                kind: Kind::Internal,
                release: Bump::None,
                text: "Refactor.".into(),
            },
        ];
        assert_eq!(
            render_section((0, 1, 0), "2026-10-11", &fragments),
            "## [0.1.0] - 2026-10-11\n\n### Added\n\n- A feature,\n  on two lines.\n\n### Fixed\n\n- A fix.\n"
        );
    }

    #[test]
    fn a_release_goes_at_the_top_of_the_metainfo() {
        let xml =
            "<releases>\n    <release version=\"0.1.0\" date=\"2026-01-01\"/>\n  </releases>\n";
        let out = insert_metainfo_release(xml, "0.2.0", "2026-02-01");
        assert!(
            out.starts_with("<releases>\n    <release version=\"0.2.0\" date=\"2026-02-01\">\n")
        );
        assert!(
            out.contains(
                "releases/tag/v0.2.0</url>\n    </release>\n    <release version=\"0.1.0\""
            )
        );
        assert_eq!(insert_metainfo_release(&out, "0.2.0", "2026-02-01"), out);
        let shipped = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(METAINFO),
        )
        .expect("the shipped metainfo");
        assert!(shipped.contains("<releases>\n"));
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
