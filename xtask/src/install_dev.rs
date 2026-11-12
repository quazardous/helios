//! `cargo xtask install-dev [--branch B | --local] | --list | --uninstall`:
//! installs a dev build of Helios for the current user, outside the
//! checkout, so cleaning `target/` never breaks the launcher.
//!
//! The build is the bench box's latest `build-gui` of a branch (`next` by
//! default), or with `--local` this checkout's own built binaries (the
//! newer of `target/release` and `target/debug`). It is copied into
//! `${XDG_DATA_HOME:-~/.local/share}/helios-dev/<version>/`, `<version>`
//! being the program's own full version (`0.79.1-dev.4+1a2b3c4d`):
//! `helios-gui`, the `helios` command line when there is one, and the
//! programs of the plugins Helios ships beside it, where Helios looks for
//! them. A `current` link points at it, the "Helios Dev" launcher starts
//! `current/helios-gui`, and the three latest installs are kept.
//!
//! The program is never started: its version is read from its bytes, or,
//! for a release build that carries no commit, from `helios-gui --version`,
//! which prints and exits before any window.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::SystemTime;

use crate::remote::Bench;

pub const USAGE: &str =
    "install-dev [--branch BRANCH | --local] | install-dev --list | install-dev --uninstall";

/// The desktop's application id, shared with `packaging/linux/install.sh`.
const APP_ID: &str = "io.github.quazardous.Helios";
/// How many installs stay.
const KEEP: usize = 3;
const ICON_SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

enum Source {
    Box(String),
    Local,
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let action = match args {
        [] => Action::Install(Source::Box("next".into())),
        ["--branch", b] => Action::Install(Source::Box((*b).to_string())),
        ["--local"] => Action::Install(Source::Local),
        ["--list"] => Action::List,
        ["--uninstall"] => Action::Uninstall,
        _ => return None,
    };
    let Some(data) = data_dir() else {
        eprintln!("install-dev: neither XDG_DATA_HOME nor HOME is set");
        return Some(ExitCode::FAILURE);
    };
    let result = match action {
        Action::Install(source) => install(root, &data, &source),
        Action::List => {
            print!("{}", list(&data.join("helios-dev")));
            Ok(())
        }
        Action::Uninstall => uninstall(&data),
    };
    Some(match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("install-dev: {e}");
            ExitCode::FAILURE
        }
    })
}

enum Action {
    Install(Source),
    List,
    Uninstall,
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn launcher_path(data: &Path) -> PathBuf {
    data.join("applications")
        .join(format!("{APP_ID}.Devel.desktop"))
}

/// The plugins' programs Helios ships, as `crates/helios-plugin/src/bundled.rs`
/// lists them (`program: "helios-..."`).
fn plugin_programs(root: &Path) -> Vec<String> {
    let bundled = std::fs::read_to_string(root.join("crates/helios-plugin/src/bundled.rs"))
        .unwrap_or_default();
    bundled
        .lines()
        .filter_map(|l| l.trim().strip_prefix("program: \""))
        .filter_map(|l| l.split('"').next())
        .map(str::to_string)
        .collect()
}

fn install(root: &Path, data: &Path, source: &Source) -> Result<(), String> {
    let base = data.join("helios-dev");
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let staging = base.join(".incoming");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let got = match source {
        Source::Box(branch) => fetch_box(root, branch, &staging),
        Source::Local => copy_local(root, &staging),
    };
    let commit = match got {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    let gui = staging.join("helios-gui");
    let version = read_version(&gui).ok_or("cannot read the version of the fetched helios-gui")?;
    let name = folder_name(&version, commit.as_deref());
    let dest = base.join(&name);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    point_current(&base, &name)?;
    write_launcher(root, data, &version)?;
    let removed = prune(&base)?;
    println!("installed Helios Dev {version} in {}", dest.display());
    for r in removed {
        println!("removed the older {r}");
    }
    Ok(())
}

/// The folder of a version: the version itself when it names its commit
/// (`0.79.1-dev.4+1a2b3c4d`), else with the commit added when known.
fn folder_name(version: &str, commit: Option<&str>) -> String {
    match commit {
        Some(c) if !version.contains('+') && !c.is_empty() => format!("{version}+{c}"),
        _ => version.to_string(),
    }
}

/// The commit a dev version was built from (`…+1a2b3c4d`).
fn version_commit(version: &str) -> Option<&str> {
    version.split_once('+').map(|(_, c)| c)
}

/// The version `helios-gui` carries: a `X.Y.Z-dev.N+<hash>` found in its
/// bytes, else what `--version` prints (a release build).
fn read_version(gui: &Path) -> Option<String> {
    let bytes = std::fs::read(gui).ok()?;
    if let Some(v) = find_dev_version(&bytes) {
        return Some(v);
    }
    let out = Command::new(gui)
        .arg("--version")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().nth(1).map(str::to_string)
}

/// The first `X.Y.Z-dev.N+<hex, 7 or more>` in `bytes`.
fn find_dev_version(bytes: &[u8]) -> Option<String> {
    const MARK: &[u8] = b"-dev.";
    let mut from = 0;
    while let Some(at) = bytes[from..].windows(MARK.len()).position(|w| w == MARK) {
        let at = from + at;
        from = at + 1;
        // back over `X.Y.Z`
        let mut start = at;
        while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
            start -= 1;
        }
        let head = std::str::from_utf8(&bytes[start..at]).ok()?;
        let nums: Vec<&str> = head.split('.').collect();
        if nums.len() != 3 || nums.iter().any(|n| n.is_empty()) {
            continue;
        }
        let mut end = at + MARK.len();
        let count_start = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == count_start || bytes.get(end) != Some(&b'+') {
            continue;
        }
        end += 1;
        let hash_start = end;
        while end < bytes.len() && matches!(bytes[end], b'0'..=b'9' | b'a'..=b'f') {
            end += 1;
        }
        if end - hash_start < 7 {
            continue;
        }
        return std::str::from_utf8(&bytes[start..end])
            .ok()
            .map(str::to_string);
    }
    None
}

/// Fetches the box's latest `build-gui` of `branch` into `dir`; says when
/// it is not the branch's head and how to queue a fresh one.
fn fetch_box(root: &Path, branch: &str, dir: &Path) -> Result<Option<String>, String> {
    let bench = Bench::configure(root)?;
    let queue = format!("cargo xtask bench-remote build-gui branch={branch}");
    let fetch = |output: &str, file: &str| -> bool {
        let Ok(mut ssh) = bench
            .ssh(
                10,
                &[
                    "fetch",
                    "latest",
                    "build-gui",
                    &format!("branch={branch}"),
                    output,
                ],
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let extracted = Command::new("tar")
            .arg("-x")
            .arg("-C")
            .arg(dir)
            .arg(file)
            .stdin(ssh.stdout.take().expect("piped"))
            .stderr(Stdio::null())
            .status();
        let fetched = ssh.wait().is_ok_and(|s| s.success());
        fetched && extracted.is_ok_and(|s| s.success()) && dir.join(file).is_file()
    };
    if !fetch("gui", "helios-gui") {
        return Err(format!(
            "the box has no build-gui of {branch} to fetch; queue one with `{queue}`"
        ));
    }
    for program in plugin_programs(root) {
        let output = program.strip_prefix("helios-").unwrap_or(&program);
        if !fetch(output, &program) {
            eprintln!("note: the box's build has no {program}: its plugin stays off");
        }
    }
    // is it the branch's head?
    let _ = Command::new("git")
        .args(["fetch", "-q", "origin", branch])
        .current_dir(root)
        .status();
    let head = Command::new("git")
        .args(["rev-parse", &format!("origin/{branch}")])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let built = find_dev_version(&std::fs::read(dir.join("helios-gui")).unwrap_or_default());
    if let (Some(head), Some(built)) = (head, built.as_deref().and_then(version_commit))
        && !head.starts_with(built)
    {
        eprintln!(
            "note: the box's latest build of {branch} is {built}, not its head {}; queue a fresh one with `{queue}`",
            &head[..8.min(head.len())]
        );
    }
    Ok(None)
}

/// Copies this checkout's built binaries (the newer of release and debug)
/// into `dir`; returns the checkout's commit.
fn copy_local(root: &Path, dir: &Path) -> Result<Option<String>, String> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map_or_else(|| root.join("target"), |t| root.join(t));
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let profile = ["release", "debug"]
        .iter()
        .map(|p| target.join(p))
        .filter(|p| p.join("helios-gui").is_file())
        .max_by_key(|p| modified(&p.join("helios-gui")))
        .ok_or_else(|| {
            format!(
                "no helios-gui built in {} (cargo build -p helios-gui)",
                target.display()
            )
        })?;
    let mut programs = vec!["helios-gui".to_string(), "helios".to_string()];
    programs.extend(plugin_programs(root));
    for program in programs {
        let from = profile.join(&program);
        if from.is_file() {
            std::fs::copy(&from, dir.join(&program))
                .map_err(|e| format!("{}: {e}", from.display()))?;
        } else if program != "helios" {
            eprintln!("note: no {program} in {}", profile.display());
        }
    }
    let commit = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    Ok(commit)
}

/// Moves `base/current` to `name`, in one rename.
fn point_current(base: &Path, name: &str) -> Result<(), String> {
    let tmp = base.join(".current.new");
    let _ = std::fs::remove_file(&tmp);
    #[cfg(unix)]
    std::os::unix::fs::symlink(name, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(name, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, base.join("current")).map_err(|e| format!("current: {e}"))
}

fn current(base: &Path) -> Option<String> {
    std::fs::read_link(base.join("current"))
        .ok()
        .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// The installed versions, oldest first.
fn installed(base: &Path) -> Vec<(String, SystemTime)> {
    let mut all: Vec<(String, SystemTime)> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .map(|n| {
            let at = std::fs::metadata(base.join(&n))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (n, at)
        })
        .collect();
    all.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    all
}

/// What goes beyond the `keep` latest installs (oldest first), never the
/// current one.
fn to_prune(installed: &[(String, SystemTime)], current: Option<&str>, keep: usize) -> Vec<String> {
    let extra = installed.len().saturating_sub(keep);
    let mut out: Vec<String> = installed
        .iter()
        .take(extra)
        .map(|(n, _)| n.clone())
        .filter(|n| Some(n.as_str()) != current)
        .collect();
    out.sort();
    out
}

fn prune(base: &Path) -> Result<Vec<String>, String> {
    let gone = to_prune(&installed(base), current(base).as_deref(), KEEP);
    for name in &gone {
        let dir = base.join(name);
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(gone)
}

fn list(base: &Path) -> String {
    let current = current(base);
    let all = installed(base);
    if all.is_empty() {
        return "no dev build installed\n".into();
    }
    let mut out = String::new();
    for (n, _) in all.iter().rev() {
        let mark = if Some(n) == current.as_ref() {
            "* "
        } else {
            "  "
        };
        out.push_str(mark);
        out.push_str(n);
        out.push('\n');
    }
    out
}

/// The "Helios Dev" launcher, from the released one: its name with the
/// version, starting `exec`, and opening no files by type.
fn desktop_file(template: &str, version: &str, exec: &Path) -> String {
    let mut out = String::new();
    for line in template.lines() {
        if line.starts_with("MimeType=") {
            continue;
        }
        let line = if line.starts_with("Name=") {
            format!("Name=Helios Dev {version}")
        } else if line.starts_with("Comment=") {
            "Comment=A development build of Helios".to_string()
        } else if let Some(rest) = line.strip_prefix("Exec=helios-gui") {
            format!("Exec=\"{}\"{rest}", exec.display())
        } else {
            line.to_string()
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn write_launcher(root: &Path, data: &Path, version: &str) -> Result<(), String> {
    let linux = root.join("packaging/linux");
    let template = std::fs::read_to_string(linux.join(format!("{APP_ID}.desktop")))
        .map_err(|e| format!("the desktop file template: {e}"))?;
    let exec = data.join("helios-dev/current/helios-gui");
    let launcher = launcher_path(data);
    let apps = launcher.parent().expect("in applications/");
    std::fs::create_dir_all(apps).map_err(|e| format!("{}: {e}", apps.display()))?;
    std::fs::write(&launcher, desktop_file(&template, version, &exec))
        .map_err(|e| format!("{}: {e}", launcher.display()))?;
    // the icons, shared with the released launcher
    let icons = data.join("icons/hicolor");
    let mut copies = vec![(
        linux.join(format!("icons/{APP_ID}.svg")),
        icons.join(format!("scalable/apps/{APP_ID}.svg")),
    )];
    for n in ICON_SIZES {
        copies.push((
            linux.join(format!("icons/{APP_ID}-{n}.png")),
            icons.join(format!("{n}x{n}/apps/{APP_ID}.png")),
        ));
    }
    for (from, to) in copies {
        if let Some(dir) = to.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::copy(&from, &to) {
            eprintln!("note: icon {}: {e}", from.display());
        }
    }
    refresh_caches(data);
    Ok(())
}

fn refresh_caches(data: &Path) {
    let quiet = |cmd: &mut Command| {
        let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    quiet(
        Command::new("gtk-update-icon-cache")
            .args(["-q", "-t"])
            .arg(data.join("icons/hicolor")),
    );
    quiet(
        Command::new("update-desktop-database")
            .arg("-q")
            .arg(data.join("applications")),
    );
}

fn uninstall(data: &Path) -> Result<(), String> {
    let launcher = launcher_path(data);
    let _ = std::fs::remove_file(&launcher);
    let base = data.join("helios-dev");
    if base.exists() {
        std::fs::remove_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    }
    refresh_caches(data);
    println!("removed the Helios Dev launcher and {}", base.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn install_dev_folder_names_carry_the_commit() {
        assert_eq!(
            folder_name("0.79.1-dev.4+1a2b3c4d", Some("ffffffff")),
            "0.79.1-dev.4+1a2b3c4d"
        );
        assert_eq!(folder_name("0.79.0", Some("1a2b3c4d")), "0.79.0+1a2b3c4d");
        assert_eq!(folder_name("0.79.0", None), "0.79.0");
        let bin = b"\0junk 1.2\0helios-gui 0.79.1-dev.4+1a2b3c4d\0more 9.9.9-dev.1+xyz";
        assert_eq!(
            find_dev_version(bin).as_deref(),
            Some("0.79.1-dev.4+1a2b3c4d")
        );
        assert_eq!(find_dev_version(b"1.2-dev.3+abcdef12 0.1.0-dev.x"), None);
        assert_eq!(version_commit("0.79.1-dev.4+1a2b3c4d"), Some("1a2b3c4d"));
    }

    #[test]
    fn install_dev_keeps_the_three_latest() {
        let at = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let all: Vec<(String, SystemTime)> = ["a", "b", "c", "d", "e"]
            .iter()
            .enumerate()
            .map(|(i, n)| ((*n).to_string(), at(i as u64)))
            .collect();
        assert_eq!(to_prune(&all, Some("e"), 3), ["a", "b"]);
        // the current one stays even when old
        assert_eq!(to_prune(&all, Some("a"), 3), ["b"]);
        assert!(to_prune(&all[..3], Some("c"), 3).is_empty());

        // on disk: folders, a current link, the listing
        let base = std::env::temp_dir().join(format!("xtask-install-dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for n in ["0.1.1-dev.1+aaaaaaaa", "0.1.1-dev.2+bbbbbbbb"] {
            std::fs::create_dir_all(base.join(n)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::create_dir_all(base.join(".incoming")).unwrap();
        point_current(&base, "0.1.1-dev.2+bbbbbbbb").unwrap();
        point_current(&base, "0.1.1-dev.1+aaaaaaaa").unwrap();
        assert_eq!(current(&base).as_deref(), Some("0.1.1-dev.1+aaaaaaaa"));
        assert_eq!(
            list(&base),
            "  0.1.1-dev.2+bbbbbbbb\n* 0.1.1-dev.1+aaaaaaaa\n"
        );
        assert!(prune(&base).unwrap().is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn install_dev_desktop_file() {
        let template = "[Desktop Entry]\nName=Helios\nComment=x\nExec=helios-gui %U\nIcon=io.github.quazardous.Helios\nMimeType=image/jpeg;\nStartupWMClass=helios-gui\n";
        let file = desktop_file(
            template,
            "0.79.1-dev.4+1a2b3c4d",
            Path::new("/d/helios-dev/current/helios-gui"),
        );
        assert_eq!(
            file,
            "[Desktop Entry]\nName=Helios Dev 0.79.1-dev.4+1a2b3c4d\nComment=A development build of Helios\nExec=\"/d/helios-dev/current/helios-gui\" %U\nIcon=io.github.quazardous.Helios\nStartupWMClass=helios-gui\n"
        );
        assert_eq!(
            launcher_path(Path::new("/d")),
            Path::new("/d/applications/io.github.quazardous.Helios.Devel.desktop")
        );
    }

    #[test]
    fn install_dev_finds_the_bundled_programs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert!(plugin_programs(root).contains(&"helios-darktable-library".to_string()));
    }
}
