//! On Windows, embeds the program's resources in `helios-gui.exe`: its
//! icon, its version information and its application manifest
//! (`packaging/windows/`: per-monitor DPI awareness, long paths). Other
//! targets build nothing here.
//!
//! On every target, sets `HELIOS_VERSION`, the version the program shows:
//! the workspace version on a release tag, and between releases a
//! pre-release of the next patch counting the commits since the last tag
//! (`0.76.1-dev.18+be34c639`), so every build has its own number; and
//! `HELIOS_COMMIT`, the commit built (empty without git), for bug reports.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=HELIOS_VERSION={}", version());
    println!("cargo:rustc-env=HELIOS_COMMIT={}", commit());
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if !windows {
        return;
    }
    let icon = "../../packaging/windows/helios.ico";
    let manifest = "../../packaging/windows/helios-gui.manifest";
    println!("cargo:rerun-if-changed={icon}");
    println!("cargo:rerun-if-changed={manifest}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set_manifest_file(manifest)
        .set("ProductName", "Helios")
        .set("FileDescription", "Helios")
        .set("CompanyName", "Helios contributors")
        .set("LegalCopyright", "GNU General Public License v3 or later");
    if let Err(e) = res.compile() {
        panic!("embedding the Windows resources: {e}");
    }
}

/// `HELIOS_BUILD_VERSION` when set (a packager's choice); else from
/// `git describe` against the last `vX.Y.Z` tag; else the crate's version
/// (a source tarball with no git).
fn version() -> String {
    println!("cargo:rerun-if-env-changed=HELIOS_BUILD_VERSION");
    if let Ok(v) = std::env::var("HELIOS_BUILD_VERSION")
        && !v.is_empty()
    {
        return v;
    }
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    // A new commit or checkout moves HEAD's log: build again then.
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(p) = git(&["rev-parse", "--path-format=absolute", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    git(&["describe", "--tags", "--long", "--match", "v[0-9]*"])
        .and_then(|d| dev_version(&d))
        .unwrap_or(pkg)
}

/// The commit built, short; empty outside a git checkout.
fn commit() -> String {
    Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// `v0.76.0-18-gbe34c639` → `0.76.1-dev.18+be34c639`; `v0.76.0-0-g…` → `0.76.0`.
fn dev_version(describe: &str) -> Option<String> {
    let mut parts = describe.rsplitn(3, '-');
    let sha = parts.next()?.strip_prefix('g')?;
    let count: u32 = parts.next()?.parse().ok()?;
    let tag = parts.next()?.strip_prefix('v')?;
    if count == 0 {
        return Some(tag.to_owned());
    }
    let mut nums = tag.splitn(3, '.');
    let (major, minor) = (nums.next()?, nums.next()?);
    let patch: u32 = nums.next()?.split(['-', '+']).next()?.parse().ok()?;
    Some(format!("{major}.{minor}.{}-dev.{count}+{sha}", patch + 1))
}
