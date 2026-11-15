//! The `log` facade's logger for the node's binaries (`helios-node`,
//! `helios`, `helios-mcp`): each line goes to standard error, as the
//! binaries' own messages do, so a spawned node's log file
//! (`.helios/NAME.node.log`, where `connect` sends its standard error)
//! holds both, in order.
//!
//! A line reads as egui-pop's log file writes it on the desktop:
//! `2026-11-15T08:35:22.384Z WARN  helios::node: message`. The node logs
//! at `info` by default, as the desktop does; the `helios` and
//! `helios-mcp` tools, whose standard error a person reads, at `warn`.
//! `HELIOS_LOG_LEVEL` sets another (`off`, `error`, `warn`, `info`,
//! `debug`, `trace`).
//!
//! Standard error rather than a file of its own: the node does not know
//! where its log goes (its host chose), and one writer keeps the lines
//! of `eprintln!` and of `log` in one order. A headless node also stays
//! free of egui, which egui-pop's `Logs` would bring.

use std::io::Write as _;

use log::{Level, LevelFilter, Log, Metadata, Record};

/// The environment variable that sets the level.
pub const LEVEL_ENV: &str = "HELIOS_LOG_LEVEL";

/// A level as `HELIOS_LOG_LEVEL` names it (case ignored).
pub fn parse_level(text: &str) -> Option<LevelFilter> {
    match text.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" | "warning" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

/// The level `env` sets, `default` when unset or unread.
pub fn level_with(env: impl Fn(&str) -> Option<String>, default: LevelFilter) -> LevelFilter {
    env(LEVEL_ENV)
        .and_then(|v| parse_level(&v))
        .unwrap_or(default)
}

/// `2026-11-15T08:35:22.384Z` for a time since the epoch.
pub fn timestamp(since_epoch: std::time::Duration) -> String {
    let secs = since_epoch.as_secs();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    let (y, m, d) = helios_vault::capture::civil(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600,
        rest / 60 % 60,
        rest % 60,
        since_epoch.subsec_millis()
    )
}

/// A log line, as written.
pub fn line(at: std::time::Duration, level: Level, target: &str, message: &str) -> String {
    format!("{} {level:<5} {target}: {message}", timestamp(at))
}

struct Stderr;

static LOGGER: Stderr = Stderr;

impl Log for Stderr {
    fn enabled(&self, m: &Metadata<'_>) -> bool {
        m.level() <= log::max_level()
    }

    fn log(&self, r: &Record<'_>) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let text = line(at, r.level(), r.target(), &r.args().to_string());
        // One write per line, so threads do not interleave inside one.
        let _ = std::io::stderr()
            .lock()
            .write_all(format!("{text}\n").as_bytes());
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Makes standard error the process's logger, at the level the
/// environment sets, else `default`. Called first thing by each binary;
/// a second call (or another logger installed first) changes nothing.
pub fn install(default: LevelFilter) {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level_with(|n| std::env::var(n).ok(), default));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_comes_from_the_environment_else_the_default() {
        let info = LevelFilter::Info;
        assert_eq!(level_with(|_| None, info), info);
        assert_eq!(level_with(|_| None, LevelFilter::Warn), LevelFilter::Warn);
        assert_eq!(level_with(|_| Some("WARN".into()), info), LevelFilter::Warn);
        assert_eq!(level_with(|_| Some("off".into()), info), LevelFilter::Off);
        assert_eq!(level_with(|_| Some("loud".into()), info), info);
    }

    #[test]
    fn a_line_reads_as_the_desktop_writes_it() {
        let at = std::time::Duration::from_millis(1_763_195_722_384);
        assert_eq!(
            line(at, Level::Warn, "helios::node", "hello"),
            "2025-11-15T08:35:22.384Z WARN  helios::node: hello"
        );
    }
}
