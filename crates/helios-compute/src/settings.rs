//! Machine-scope settings: what each device may do, kept in a local file
//! that is never synced (`compute.toml` under the machine directory).
//! A device that disappears keeps its settings: it shows as missing and
//! finds them again when it comes back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::device::{Device, DeviceKind};

/// The file, in the machine directory.
pub const FILE: &str = "compute.toml";

/// The machine directory: `$XDG_CONFIG_HOME/helios/machine`, or
/// `~/.config/helios/machine`. Never synced: what it holds describes this
/// machine only.
pub fn default_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("helios/machine")
}

/// What work a device takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// What a person waits for: the viewport, previews.
    Interactive,
    /// What runs in the background: exports, prefetch.
    Batch,
    #[default]
    Both,
}

impl Role {
    pub fn parse(text: &str) -> Result<Role, String> {
        match text {
            "interactive" => Ok(Role::Interactive),
            "batch" => Ok(Role::Batch),
            "both" => Ok(Role::Both),
            _ => Err(format!(
                "`{text}` is not a role: `interactive`, `batch` or `both`"
            )),
        }
    }

    /// Whether a device of this role takes work of `kind`.
    pub fn takes(self, kind: Role) -> bool {
        self == Role::Both || kind == Role::Both || self == kind
    }
}

/// One device's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceSettings {
    /// Its name and kind when last seen, to show it once missing.
    pub name: String,
    pub kind: DeviceKind,
    pub enabled: bool,
    #[serde(default)]
    pub role: Role,
    /// Higher first: a device of higher priority is chosen before a faster one.
    #[serde(default)]
    pub priority: i32,
    /// CPU threads the work routed here may use (`None`: all of them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<usize>,
    /// GPU memory a job may take, in MiB (`None`: what the device allows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_mb: Option<u64>,
    /// The engine's cache budget, in MiB (`None`: the default share of
    /// memory, see `helios_core::engine::DEFAULT_CACHE_SHARE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<Calibration>,
}

impl DeviceSettings {
    /// What a device newly seen starts with: on, except software GPUs.
    pub fn new(device: &Device) -> Self {
        Self {
            name: device.name.clone(),
            kind: device.kind,
            enabled: device.kind != DeviceKind::SoftwareGpu,
            role: Role::Both,
            priority: 0,
            threads: None,
            vram_mb: None,
            cache_mb: None,
            calibration: None,
        }
    }
}

/// A benchmark's result: how fast a device runs each module family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// Megapixels per second, by module family (`tone`, `detail`…).
    pub speeds: BTreeMap<String, f64>,
    /// When it ran, in seconds since the Unix epoch.
    pub at: u64,
}

/// A remote unit: a helios-node reached over its socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteSettings {
    pub name: String,
    /// A socket path, or `tcp://HOST:PORT`.
    pub address: String,
    /// Where the session token of a TCP address is read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<PathBuf>,
}

/// Everything in `compute.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// By device id.
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceSettings>,
    #[serde(default)]
    pub remotes: Vec<RemoteSettings>,
}

impl Settings {
    /// The settings in `dir`; defaults when there is no file yet.
    pub fn load(dir: &Path) -> Result<Settings, String> {
        let path = dir.join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the settings to `dir`, creating it, through a temporary file
    /// so a crash never leaves half a file.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        let path = dir.join(FILE);
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }
}
