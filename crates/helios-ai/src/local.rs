//! The local runtime: what this machine can run, which models fit, and
//! how to install Ollama for this user.
//!
//! Everything is read through a [`System`] (the real one runs programs and
//! reads files; tests give a fake): `nvidia-smi`, `/sys/class/drm`,
//! `/proc/cpuinfo`, `/proc/meminfo`, `/etc/os-release`, `sysctl`, `df`,
//! `id -u`, and `PowerShell` on Windows. Parsing is separate from reading,
//! so each parser is tested on real outputs.
//!
//! **Setup never runs as root and never runs what was not confirmed.**
//! [`plan`] gives the steps for this OS; [`setup`] runs a step only when
//! its id is in `confirm`, never a step marked `root` (shown so the user
//! can run it themselves), and nothing at all when Helios itself runs as
//! root.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::{Value, json};

use crate::Capability;

/// What the machine is asked through.
pub trait System: Send + Sync {
    /// Runs a program; its standard output when it succeeds.
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
    fn read(&self, path: &Path) -> Option<String>;
    fn exists(&self, path: &Path) -> bool;
    fn write(&self, path: &Path, text: &str) -> Result<(), String>;
    /// A program on `PATH`.
    fn which(&self, program: &str) -> Option<PathBuf>;
    fn env(&self, name: &str) -> Option<String>;
    /// `linux`, `macos`, `windows`...
    fn os(&self) -> String;
    /// `x86_64`, `aarch64`...
    fn arch(&self) -> String;
}

/// This machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealSystem;

impl System for RealSystem {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let out = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| format!("cannot run `{program}`: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            Err(format!(
                "`{program} {}` failed ({}): {}",
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn write(&self, path: &Path, text: &str) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        let dirs = std::env::var_os("PATH")?;
        std::env::split_paths(&dirs).find_map(|d| {
            let plain = d.join(program);
            let exe = d.join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
            [plain, exe].into_iter().find(|p| p.is_file())
        })
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn os(&self) -> String {
        std::env::consts::OS.to_string()
    }

    fn arch(&self) -> String {
        std::env::consts::ARCH.to_string()
    }
}

/// A graphics processor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gpu {
    /// `nvidia`, `amd`, `apple`.
    pub vendor: String,
    pub name: String,
    /// Its own memory; none when it shares the machine's.
    pub vram_mib: Option<u64>,
    /// Shares the machine's memory (Apple silicon).
    pub unified: bool,
}

/// What the machine has, and what a model may take.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Probe {
    pub os: String,
    /// The distribution or version, when known.
    pub os_name: Option<String>,
    pub arch: String,
    pub cpu: Option<String>,
    pub threads: Option<u64>,
    pub ram_mib: Option<u64>,
    pub gpus: Vec<Gpu>,
    /// Where the runtime keeps its models.
    pub models_dir: PathBuf,
    pub disk_free_mib: Option<u64>,
    /// The memory a model may take, runtime overhead included.
    pub budget_mib: u64,
    /// Whether models run on a GPU (or unified memory), not the CPU alone.
    pub accelerated: bool,
    /// Whether this process runs as root (setup refuses to).
    pub root: bool,
    /// The catalogue's models that fit, by name.
    pub fits: Vec<String>,
    /// What could not be read.
    pub notes: Vec<String>,
}

/// `MemTotal` of `/proc/meminfo`, in mebibytes.
pub fn parse_meminfo(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|kib| kib.parse::<u64>().ok())
        .map(|kib| kib / 1024)
}

/// The CPU's model name and its thread count, from `/proc/cpuinfo`.
pub fn parse_cpuinfo(text: &str) -> (Option<String>, Option<u64>) {
    let name = text
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string());
    let threads = text.lines().filter(|l| l.starts_with("processor")).count() as u64;
    (name, (threads > 0).then_some(threads))
}

/// `PRETTY_NAME` of `/etc/os-release`.
pub fn parse_os_release(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
}

/// `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`.
pub fn parse_nvidia_smi(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|l| {
            let (name, mib) = l.rsplit_once(',')?;
            Some(Gpu {
                vendor: "nvidia".into(),
                name: name.trim().to_string(),
                vram_mib: mib.trim().parse().ok(),
                unified: false,
            })
        })
        .collect()
}

/// The available space of `df -Pk`'s output, in mebibytes.
pub fn parse_df(text: &str) -> Option<u64> {
    let line = text.lines().nth(1)?;
    let available: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(available / 1024)
}

/// The nearest directory of `path` that exists (models go in one that may
/// not exist yet).
fn existing(system: &dyn System, path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    while !system.exists(&p) {
        match p.parent() {
            Some(parent) if parent != p => p = parent.to_path_buf(),
            _ => break,
        }
    }
    p
}

/// The user's home directory.
pub fn home(system: &dyn System) -> PathBuf {
    system
        .env("HOME")
        .or_else(|| system.env("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Where Ollama keeps its models for this user.
pub fn models_dir(system: &dyn System) -> PathBuf {
    system.env("OLLAMA_MODELS").map_or_else(
        || home(system).join(".ollama").join("models"),
        PathBuf::from,
    )
}

/// The free space under `path`, in mebibytes.
pub fn disk_free(system: &dyn System, path: &Path) -> Option<u64> {
    let dir = existing(system, path);
    if system.os() == "windows" {
        let drive = dir.to_string_lossy().chars().next()?;
        let out = system
            .run(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    &format!("(Get-PSDrive -Name {drive}).Free"),
                ],
            )
            .ok()?;
        return out.trim().parse::<u64>().ok().map(|b| b / (1024 * 1024));
    }
    let dir = dir.to_string_lossy().to_string();
    parse_df(&system.run("df", &["-Pk", &dir]).ok()?)
}

fn amd_gpus(system: &dyn System) -> Vec<Gpu> {
    let mut gpus = Vec::new();
    for card in 0..8 {
        let device = PathBuf::from(format!("/sys/class/drm/card{card}/device"));
        if system
            .read(&device.join("vendor"))
            .map(|v| v.trim().to_string())
            != Some("0x1002".into())
        {
            continue;
        }
        let vram = system
            .read(&device.join("mem_info_vram_total"))
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(|b| b / (1024 * 1024));
        let name = system
            .read(&device.join("product_name"))
            .map_or_else(|| "AMD GPU".to_string(), |n| n.trim().to_string());
        gpus.push(Gpu {
            vendor: "amd".into(),
            name,
            vram_mib: vram,
            unified: false,
        });
    }
    gpus
}

/// What this machine has.
pub fn probe(system: &dyn System) -> Probe {
    let os = system.os();
    let arch = system.arch();
    let mut notes = Vec::new();
    let os_name: Option<String>;
    let ram_mib: Option<u64>;
    let (mut cpu, mut threads) = (None, None);
    match os.as_str() {
        "macos" => {
            let sysctl = |key: &str| {
                system
                    .run("sysctl", &["-n", key])
                    .ok()
                    .map(|v| v.trim().to_string())
            };
            os_name = system
                .run("sw_vers", &["-productVersion"])
                .ok()
                .map(|v| format!("macOS {}", v.trim()));
            cpu = sysctl("machdep.cpu.brand_string");
            threads = sysctl("hw.ncpu").and_then(|v| v.parse().ok());
            ram_mib = sysctl("hw.memsize")
                .and_then(|v| v.parse::<u64>().ok())
                .map(|b| b / (1024 * 1024));
        }
        "windows" => {
            let ps = |command: &str| {
                system
                    .run("powershell", &["-NoProfile", "-Command", command])
                    .ok()
                    .map(|v| v.trim().to_string())
            };
            os_name = ps("(Get-CimInstance Win32_OperatingSystem).Caption");
            cpu = ps("(Get-CimInstance Win32_Processor).Name");
            threads = system
                .env("NUMBER_OF_PROCESSORS")
                .and_then(|v| v.parse().ok());
            ram_mib = ps("(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory")
                .and_then(|v| v.parse::<u64>().ok())
                .map(|b| b / (1024 * 1024));
        }
        _ => {
            os_name = system
                .read(Path::new("/etc/os-release"))
                .and_then(|t| parse_os_release(&t));
            if let Some(text) = system.read(Path::new("/proc/cpuinfo")) {
                (cpu, threads) = parse_cpuinfo(&text);
            }
            ram_mib = system
                .read(Path::new("/proc/meminfo"))
                .and_then(|t| parse_meminfo(&t));
        }
    }
    let mut gpus = Vec::new();
    if system.which("nvidia-smi").is_some() {
        match system.run(
            "nvidia-smi",
            &[
                "--query-gpu=name,memory.total",
                "--format=csv,noheader,nounits",
            ],
        ) {
            Ok(out) => gpus.extend(parse_nvidia_smi(&out)),
            Err(e) => notes.push(format!("nvidia-smi: {e}")),
        }
    }
    if os == "linux" {
        gpus.extend(amd_gpus(system));
    }
    if os == "macos" && arch == "aarch64" {
        gpus.push(Gpu {
            vendor: "apple".into(),
            name: cpu.clone().unwrap_or_else(|| "Apple silicon".into()),
            vram_mib: None,
            unified: true,
        });
    }
    if ram_mib.is_none() {
        notes.push("the memory size could not be read".into());
    }
    let models_dir = models_dir(system);
    let disk_free_mib = disk_free(system, &models_dir);
    if disk_free_mib.is_none() {
        notes.push(format!(
            "the free disk under {} could not be read",
            models_dir.display()
        ));
    }
    let root = os != "windows" && system.run("id", &["-u"]).is_ok_and(|uid| uid.trim() == "0");
    let (budget_mib, accelerated) = budget(&gpus, ram_mib);
    let fits = CATALOG
        .iter()
        .filter(|m| fits(m, budget_mib, disk_free_mib))
        .map(|m| m.name.to_string())
        .collect();
    Probe {
        os,
        os_name,
        arch,
        cpu,
        threads,
        ram_mib,
        gpus,
        models_dir,
        disk_free_mib,
        budget_mib,
        accelerated,
        root,
        fits,
        notes,
    }
}

/// The memory a model may take: most of the largest GPU's own memory, two
/// thirds of unified memory, or half the RAM on the CPU alone.
pub fn budget(gpus: &[Gpu], ram_mib: Option<u64>) -> (u64, bool) {
    if let Some(vram) = gpus.iter().filter_map(|g| g.vram_mib).max()
        && vram >= 2048
    {
        return (vram * 9 / 10, true);
    }
    let ram = ram_mib.unwrap_or(0);
    if gpus.iter().any(|g| g.unified) {
        return (ram * 2 / 3, true);
    }
    (ram / 2, false)
}

/// A model the recommendations know.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Known {
    /// Its name in Ollama's library.
    pub name: &'static str,
    pub capabilities: &'static [Capability],
    /// Its download, approximately (the default quantization).
    pub size_mib: u64,
    /// Higher is better, among models of one capability.
    pub quality: u8,
}

const VISION: &[Capability] = &[Capability::Chat, Capability::Vision, Capability::Structured];
const TEXT: &[Capability] = &[Capability::Chat, Capability::Structured];
const EMBEDDINGS: &[Capability] = &[Capability::Embeddings];

/// Models worth recommending, with approximate sizes: the runtime has the
/// last word when it pulls.
pub const CATALOG: &[Known] = &[
    Known {
        name: "qwen2.5vl:3b",
        capabilities: VISION,
        size_mib: 3_200,
        quality: 2,
    },
    Known {
        name: "qwen2.5vl:7b",
        capabilities: VISION,
        size_mib: 6_000,
        quality: 4,
    },
    Known {
        name: "qwen2.5vl:32b",
        capabilities: VISION,
        size_mib: 21_000,
        quality: 5,
    },
    Known {
        name: "gemma3:4b",
        capabilities: VISION,
        size_mib: 3_300,
        quality: 2,
    },
    Known {
        name: "gemma3:12b",
        capabilities: VISION,
        size_mib: 8_100,
        quality: 3,
    },
    Known {
        name: "gemma3:27b",
        capabilities: VISION,
        size_mib: 17_000,
        quality: 4,
    },
    Known {
        name: "llama3.2:3b",
        capabilities: TEXT,
        size_mib: 2_000,
        quality: 1,
    },
    Known {
        name: "qwen3:4b",
        capabilities: TEXT,
        size_mib: 2_500,
        quality: 2,
    },
    Known {
        name: "qwen3:8b",
        capabilities: TEXT,
        size_mib: 5_200,
        quality: 3,
    },
    Known {
        name: "qwen3:14b",
        capabilities: TEXT,
        size_mib: 9_300,
        quality: 4,
    },
    Known {
        name: "qwen3:32b",
        capabilities: TEXT,
        size_mib: 20_000,
        quality: 5,
    },
    Known {
        name: "nomic-embed-text",
        capabilities: EMBEDDINGS,
        size_mib: 274,
        quality: 2,
    },
    Known {
        name: "mxbai-embed-large",
        capabilities: EMBEDDINGS,
        size_mib: 670,
        quality: 3,
    },
    Known {
        name: "embeddinggemma",
        capabilities: EMBEDDINGS,
        size_mib: 622,
        quality: 3,
    },
];

/// A model of the catalogue, by name (`qwen3:8b`, or `nomic-embed-text`
/// for `nomic-embed-text:latest`).
pub fn known(name: &str) -> Option<&'static Known> {
    let bare = name.strip_suffix(":latest").unwrap_or(name);
    CATALOG.iter().find(|k| k.name == bare)
}

/// Whether a model fits in memory (with a fifth more for the runtime and
/// its context) and on disk.
pub fn fits(model: &Known, budget_mib: u64, disk_free_mib: Option<u64>) -> bool {
    model.size_mib * 6 / 5 <= budget_mib && disk_free_mib.is_none_or(|d| model.size_mib < d)
}

/// Per capability, the catalogue's models that fit, best first, and those
/// that do not; `installed` says which the runtime already has.
pub fn recommend(probe: &Probe, installed: &[String]) -> Value {
    let has = |name: &str| {
        installed
            .iter()
            .any(|i| i == name || i.strip_suffix(":latest") == Some(name))
    };
    let mut per = serde_json::Map::new();
    for capability in [
        Capability::Vision,
        Capability::Chat,
        Capability::Structured,
        Capability::Embeddings,
    ] {
        let mut models: Vec<&Known> = CATALOG
            .iter()
            .filter(|k| k.capabilities.contains(&capability))
            .collect();
        models.sort_by(|a, b| b.quality.cmp(&a.quality).then(a.size_mib.cmp(&b.size_mib)));
        let (fitting, too_big): (Vec<&Known>, Vec<&Known>) = models
            .into_iter()
            .partition(|k| fits(k, probe.budget_mib, probe.disk_free_mib));
        let shown = |k: &&Known| json!({ "name": k.name, "size_mib": k.size_mib, "installed": has(k.name) });
        per.insert(
            capability.name().into(),
            json!({
                "recommended": fitting.first().map(|k| k.name),
                "fitting": fitting.iter().map(shown).collect::<Vec<_>>(),
                "too_big": too_big.iter().map(shown).collect::<Vec<_>>(),
            }),
        );
    }
    per.insert(
        Capability::Segmentation.name().into(),
        json!({
            "recommended": null,
            "reason": "no local runtime serves a segmentation model over a standard API yet: \
                       add a `command` provider wrapping one (a SAM-family model)",
        }),
    );
    json!({
        "budget_mib": probe.budget_mib,
        "accelerated": probe.accelerated,
        "note": if probe.accelerated { "" } else {
            "no GPU found: models run on the CPU, slowly; prefer the smallest"
        },
        "sizes": "approximate downloads; the runtime has the last word",
        "capabilities": per,
    })
}

/// What a setup step does.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Runs a program, never through a shell.
    Run { argv: Vec<String> },
    /// Writes a file.
    Write { path: PathBuf, text: String },
    /// Only shown: the user does it.
    Manual { text: String },
}

/// One step of the setup.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub action: Action,
    /// Needs root: shown, never run.
    pub root: bool,
    /// Already done (the runtime is installed, or running).
    pub done: bool,
}

fn step(id: &str, title: &str, action: Action, done: bool) -> Step {
    Step {
        id: id.into(),
        title: title.into(),
        action,
        root: false,
        done,
    }
}

fn run(argv: &[&str]) -> Action {
    Action::Run {
        argv: argv.iter().map(|a| (*a).to_string()).collect(),
    }
}

/// What is known of the runtime before the setup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Present {
    pub installed: bool,
    pub running: bool,
}

/// The steps to install and start Ollama for this user on this OS.
pub fn plan(system: &dyn System, cache: &Path, present: Present) -> Vec<Step> {
    let home = home(system);
    let os = system.os();
    match os.as_str() {
        "linux" => {
            let arch = if system.arch() == "aarch64" {
                "arm64"
            } else {
                "amd64"
            };
            let archive = cache.join(format!("ollama-linux-{arch}.tgz"));
            let archive = archive.to_string_lossy().to_string();
            let local = home.join(".local").to_string_lossy().to_string();
            let url = format!("https://ollama.com/download/ollama-linux-{arch}.tgz");
            let service = home.join(".config/systemd/user/ollama.service");
            let exec = home.join(".local/bin/ollama");
            vec![
                step(
                    "download",
                    "Download Ollama's release archive for this user",
                    run(&[
                        "curl",
                        "-fL",
                        "--proto",
                        "=https",
                        "--create-dirs",
                        "-o",
                        &archive,
                        &url,
                    ]),
                    present.installed,
                ),
                step(
                    "prepare",
                    "Create the user's program directory",
                    run(&["mkdir", "-p", &local]),
                    present.installed,
                ),
                step(
                    "unpack",
                    "Unpack it under ~/.local (bin/ollama, lib/ollama)",
                    run(&["tar", "-xzf", &archive, "-C", &local]),
                    present.installed,
                ),
                step(
                    "service",
                    "Write a systemd user service listening on loopback only",
                    Action::Write {
                        path: service,
                        text: format!(
                            "[Unit]\nDescription=Ollama (set up by Helios)\nAfter=network-online.target\n\n\
                             [Service]\nExecStart={} serve\nEnvironment=OLLAMA_HOST=127.0.0.1:11434\n\
                             Restart=on-failure\n\n[Install]\nWantedBy=default.target\n",
                            exec.display()
                        ),
                    },
                    present.running,
                ),
                step(
                    "reload",
                    "Make systemd read the new service",
                    run(&["systemctl", "--user", "daemon-reload"]),
                    present.running,
                ),
                step(
                    "start",
                    "Start it now and at each login",
                    run(&["systemctl", "--user", "enable", "--now", "ollama.service"]),
                    present.running,
                ),
                Step {
                    root: true,
                    ..step(
                        "system-install",
                        "Or, system-wide (asks for sudo; Helios never runs it)",
                        Action::Manual {
                            text: "curl -fsSL https://ollama.com/install.sh | sh".into(),
                        },
                        present.installed,
                    )
                },
            ]
        }
        "macos" if system.which("brew").is_some() => vec![
            step(
                "install",
                "Install Ollama with Homebrew",
                run(&["brew", "install", "ollama"]),
                present.installed,
            ),
            step(
                "start",
                "Start it now and at each login",
                run(&["brew", "services", "start", "ollama"]),
                present.running,
            ),
        ],
        "macos" => vec![step(
            "install",
            "Install Ollama's app",
            Action::Manual {
                text: "Download Ollama from https://ollama.com/download/mac, move it to \
                       Applications and open it once."
                    .into(),
            },
            present.installed,
        )],
        "windows" if system.which("winget").is_some() => vec![step(
            "install",
            "Install Ollama with winget (for this user; it starts at login)",
            run(&[
                "winget",
                "install",
                "--id",
                "Ollama.Ollama",
                "-e",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ]),
            present.installed,
        )],
        _ => vec![step(
            "install",
            "Install Ollama",
            Action::Manual {
                text: format!(
                    "No automatic setup for {os}: install Ollama from https://ollama.com/download"
                ),
            },
            present.installed,
        )],
    }
}

/// What happened to one step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Outcome {
    #[serde(flatten)]
    pub step: Step,
    /// `done`, `ran`, `failed`, `not_confirmed`, `shown_only`,
    /// `not_run` (an earlier step failed), `refused` (running as root).
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Runs the confirmed steps of `steps`, in order, stopping at the first
/// failure. A step marked `root` or manual is never run; nothing runs when
/// `root` (Helios runs as root).
pub fn setup(
    system: &dyn System,
    steps: Vec<Step>,
    confirm: &[String],
    root: bool,
) -> Vec<Outcome> {
    let mut failed = false;
    steps
        .into_iter()
        .map(|step| {
            let (outcome, detail) = if root {
                (
                    "refused",
                    Some("Helios runs as root: it never sets anything up as root".to_string()),
                )
            } else if step.done {
                ("done", None)
            } else if step.root || matches!(step.action, Action::Manual { .. }) {
                ("shown_only", None)
            } else if !confirm.contains(&step.id) {
                ("not_confirmed", None)
            } else if failed {
                ("not_run", None)
            } else {
                let result = match &step.action {
                    Action::Run { argv } => {
                        let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
                        system.run(&argv[0], &args).map(|_| ())
                    }
                    Action::Write { path, text } => system.write(path, text),
                    Action::Manual { .. } => unreachable!("manual steps are only shown"),
                };
                match result {
                    Ok(()) => ("ran", None),
                    Err(e) => {
                        failed = true;
                        ("failed", Some(e))
                    }
                }
            };
            Outcome {
                step,
                outcome,
                detail,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_files_are_read() {
        assert_eq!(
            parse_meminfo("MemTotal:       32768000 kB\nMemFree: 1 kB\n"),
            Some(32000)
        );
        let cpu = "processor\t: 0\nmodel name\t: AMD Ryzen 9 7950X\nprocessor\t: 1\nmodel name\t: AMD Ryzen 9 7950X\n";
        assert_eq!(
            parse_cpuinfo(cpu),
            (Some("AMD Ryzen 9 7950X".into()), Some(2))
        );
        assert_eq!(
            parse_os_release("NAME=Fedora\nPRETTY_NAME=\"Fedora Linux 44\"\n"),
            Some("Fedora Linux 44".into())
        );
    }

    #[test]
    fn nvidia_smi_and_df_are_read() {
        let gpus = parse_nvidia_smi("NVIDIA GeForce RTX 4070, 12282\n");
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(gpus[0].vram_mib, Some(12282));
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/nvme0n1p3 976000000 500000000 476000000 52% /home\n";
        assert_eq!(parse_df(df), Some(464_843));
    }

    #[test]
    fn the_budget_follows_the_hardware() {
        let nvidia = parse_nvidia_smi("RTX, 12000");
        assert_eq!(budget(&nvidia, Some(64000)), (10800, true));
        let apple = vec![Gpu {
            vendor: "apple".into(),
            name: "M3".into(),
            vram_mib: None,
            unified: true,
        }];
        assert_eq!(budget(&apple, Some(36000)), (24000, true));
        assert_eq!(budget(&[], Some(16000)), (8000, false));
    }

    #[test]
    fn a_model_fits_in_memory_and_on_disk() {
        let m = known("qwen2.5vl:7b").unwrap();
        assert!(fits(m, 8000, None));
        assert!(!fits(m, 7000, None));
        assert!(!fits(m, 80000, Some(5000)));
        assert_eq!(known("nomic-embed-text:latest").unwrap().size_mib, 274);
    }
}
