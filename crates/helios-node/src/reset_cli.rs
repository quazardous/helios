//! `helios --reset [everything|caches|settings]`: the Settings Danger
//! zone's reset from the command line, for a Helios that no longer
//! starts. The same kernel ([`helios_node_core::reset`]) and the same
//! plan, said before anything moves:
//!
//! ```text
//!     helios --reset [everything|caches|settings] [--vault FILE] [--photos DIR]... [--confirm RESET]
//! ```
//!
//! - The folders are this machine's ([`Places::from_env`]), the library
//!   `--vault FILE` or the default one (`library.db` in the data folder).
//! - The photo folders the plan is checked against are read from the
//!   library itself ([`reset::library_photos`]); `--photos DIR` adds one.
//!   A library that cannot be read refuses the reset unless `--photos`
//!   names the photo folders.
//! - It refuses while Helios is running: a node holds the library's lock,
//!   or one is starting.
//! - It asks for the word [`WORD`] typed in the terminal; with no
//!   terminal (a script, a pipe) it refuses unless `--confirm RESET` is
//!   given.
//!
//! Exit codes: 0 reset (or nothing to do), 1 the reset failed part way
//! (the report says where), 2 usage, 3 refused.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use helios_node_core::reset::{self, Fate, Places, Plan, Scope};

/// The word typed to confirm a reset, as in the desktop's Danger zone.
pub const WORD: &str = "RESET";

/// The library file's name in the data folder.
const LIBRARY_FILE: &str = "library.db";

/// What the command line asks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    pub scope: Option<Scope>,
    pub vault: Option<PathBuf>,
    pub photos: Vec<PathBuf>,
    pub confirm: Option<String>,
}

/// Reads the arguments after `--reset`.
///
/// # Errors
/// An unknown argument, a scope that is not one, a flag missing its value.
pub fn parse(args: &[String]) -> Result<Ask, String> {
    let mut ask = Ask::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{flag} takes a value"))
        };
        match arg.as_str() {
            "--vault" => ask.vault = Some(PathBuf::from(value("--vault")?)),
            "--photos" => ask.photos.push(PathBuf::from(value("--photos")?)),
            "--confirm" => ask.confirm = Some(value("--confirm")?),
            name if !name.starts_with('-') && ask.scope.is_none() => {
                ask.scope = Some(Scope::parse(name).ok_or_else(|| {
                    format!("`{name}` is not a scope: everything, caches or settings")
                })?);
            }
            other => return Err(format!("--reset does not take `{other}`")),
        }
    }
    Ok(ask)
}

/// How a reset is confirmed: `--confirm RESET`, else the word typed in a
/// terminal; no terminal and no `--confirm` refuses.
///
/// # Errors
/// The word given or typed is not [`WORD`], or there is no terminal to
/// ask in.
pub fn confirmed(
    confirm: Option<&str>,
    terminal: bool,
    ask: impl FnOnce() -> Option<String>,
) -> Result<(), String> {
    let typed = match confirm {
        Some(word) => word.to_owned(),
        None if terminal => ask().unwrap_or_default(),
        None => {
            return Err(format!(
                "no terminal to type {WORD} in: pass --confirm {WORD} to reset from a script"
            ));
        }
    };
    if typed.trim() == WORD {
        Ok(())
    } else {
        Err(format!("not reset: the word typed was not {WORD}"))
    }
}

/// Whether Helios is running on the library at `file`: a node holds its
/// lock, or one is starting on it. Nothing is created or kept.
fn running(file: &Path) -> Result<bool, String> {
    let files = helios_inspect::discovery::files(file);
    for lock in [&files.lock, &files.start] {
        if helios_inspect::discovery::Lock::held(lock)
            .map_err(|e| format!("{}: {e}", lock.display()))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The plan in words: each place and its fate, the backup folder, the
/// photos that stay.
pub fn plan_lines(plan: &Plan, photos: &reset::Photos) -> Vec<String> {
    let mut lines = vec![format!("Reset of Helios ({:?}):", plan.scope)];
    for target in &plan.targets {
        let fate = match target.fate {
            Fate::Backup => "moved to the backup folder",
            Fate::Remove => "removed",
        };
        lines.push(format!(
            "  {:<10} {}: {fate}",
            format!("{:?}", target.what),
            target.path.display()
        ));
    }
    match &plan.backup {
        Some(b) => lines.push(format!(
            "Backup folder (a reset can be undone by hand from it): {}",
            b.display()
        )),
        None => lines.push("Nothing is moved; nothing is kept aside.".to_owned()),
    }
    lines.push(format!(
        "Stay untouched: your {} photos and their sidecars ({} folders), and the backup disks.",
        photos.count,
        photos.roots.len()
    ));
    for root in &photos.roots {
        lines.push(format!("  {}", root.display()));
    }
    lines
}

/// Runs `helios --reset ARGS`, with `env` read for the folders, `terminal`
/// whether stdin is one, `input` to read the word from, `out` for what
/// is said. Returns the exit code.
pub fn run(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
    terminal: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> u8 {
    let mut say = |line: &str| {
        let _ = writeln!(out, "{line}");
    };
    let ask = match parse(args) {
        Ok(ask) => ask,
        Err(e) => {
            say(&format!("helios: {e}"));
            return 2;
        }
    };
    let scope = ask.scope.unwrap_or(Scope::Everything);
    let Some(mut places) = Places::from_env(&env, None) else {
        say("helios: Helios's folders cannot be told here (no HOME)");
        return 3;
    };
    let library = ask
        .vault
        .clone()
        .map(|v| std::path::absolute(&v).unwrap_or(v))
        .unwrap_or_else(|| places.data.join(LIBRARY_FILE));
    places.library = Some(library.clone());
    match running(&library) {
        Ok(false) => {}
        Ok(true) => {
            say(&format!(
                "helios: Helios is running on {}: quit it first, or use Settings > Danger zone",
                library.display()
            ));
            return 3;
        }
        Err(e) => {
            say(&format!("helios: whether Helios is running: {e}"));
            return 3;
        }
    }
    let mut photos = match reset::library_photos(&library) {
        Ok(photos) => photos,
        Err(e) if !ask.photos.is_empty() => {
            say(&format!(
                "helios: the library cannot be read ({e}); the folders of --photos are kept"
            ));
            reset::Photos::default()
        }
        Err(e) => {
            say(&format!(
                "helios: the library cannot be read to find your photo folders ({e}): \
                 name them with --photos DIR (once per folder) to reset anyway"
            ));
            return 3;
        }
    };
    photos.roots.extend(ask.photos.iter().cloned());
    photos.roots.sort();
    photos.roots.dedup();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        .to_string();
    let plan = match reset::plan(&places, scope, &photos.roots, &stamp) {
        Ok(plan) => plan,
        Err(refused) => {
            say(&format!("helios: a reset is not possible here: {refused}"));
            return 3;
        }
    };
    for line in plan_lines(&plan, &photos) {
        say(&line);
    }
    let asked = confirmed(ask.confirm.as_deref(), terminal, || {
        say(&format!("Type {WORD} to reset: "));
        let mut line = String::new();
        input.read_line(&mut line).ok().map(|_| line)
    });
    if let Err(e) = asked {
        say(&format!("helios: {e}"));
        return 3;
    }
    // Asked again just before: Helios started while the word was typed.
    if running(&library).unwrap_or(true) {
        say("helios: Helios started meanwhile: not reset");
        return 3;
    }
    log::warn!(target: "helios::reset", "reset from the command line ({scope:?})");
    let report = reset::run(&plan);
    say(&format!(
        "{} moved, {} removed.",
        report.moved.len(),
        report.removed.len()
    ));
    if let Some((path, why)) = &report.failed {
        say(&format!(
            "helios: the reset stopped at {}: {why}; what is listed above it is done",
            path.display()
        ));
        return 1;
    }
    say("Helios is reset: the next launch is a first launch.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_arguments_name_a_scope_the_library_photos_and_the_word() {
        let ask = parse(&args(&[
            "caches",
            "--vault",
            "/l.db",
            "--photos",
            "/p",
            "--confirm",
            "RESET",
        ]))
        .unwrap();
        assert_eq!(ask.scope, Some(Scope::Caches));
        assert_eq!(ask.vault, Some(PathBuf::from("/l.db")));
        assert_eq!(ask.photos, [PathBuf::from("/p")]);
        assert_eq!(ask.confirm.as_deref(), Some("RESET"));
        assert_eq!(parse(&[]).unwrap().scope, None);
        assert!(parse(&args(&["all"])).is_err());
        assert!(parse(&args(&["caches", "settings"])).is_err());
        assert!(parse(&args(&["--confirm"])).is_err());
    }

    #[test]
    fn the_word_is_typed_in_a_terminal_or_given_and_never_assumed() {
        let never = || -> Option<String> { panic!("asked with no terminal") };
        assert!(confirmed(Some("RESET"), false, never).is_ok());
        assert!(confirmed(Some("reset"), false, never).is_err());
        let refused = confirmed(None, false, never).unwrap_err();
        assert!(refused.contains("--confirm RESET"), "{refused}");
        assert!(confirmed(None, true, || Some("RESET\n".into())).is_ok());
        assert!(confirmed(None, true, || Some("yes\n".into())).is_err());
        assert!(confirmed(None, true, || None).is_err());
    }
}
