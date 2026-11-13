//! The GPU checks: which adapter Helios renders on (or the CPU), and
//! whether every GPU kernel compiles there. They read a [`Gpu`] gathered
//! once per run; the kernels are compiled by a function the desktop gives
//! ([`Kernels::new`]), once per adapter and driver.

use std::sync::{Arc, Mutex, PoisonError};

use helios_core::Message;
use serde_json::json;

use crate::{Check, Context, Declaration, Finding, Fix, Level};

/// The owner of the GPU checks.
pub const OWNER: &str = "gpu";

/// The GPU module's checks known without the desktop: the adapter in use
/// (the kernels' check, [`Kernels`], is added by the desktop).
pub fn checks() -> Vec<Arc<dyn Check>> {
    vec![Arc::new(AdapterCheck)]
}

pub const ADAPTER: &str = "gpu.adapter";
pub const KERNELS: &str = "gpu.kernels";

/// Where the documentation of the GPU choice lives.
pub const DOCS: &str = "https://github.com/quazardous/helios/blob/main/docs/gui.md#the-gpu";

/// What kind of device an adapter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterKind {
    Discrete,
    Integrated,
    Virtual,
    Other,
    /// A rasteriser on the CPU (llvmpipe, lavapipe, WARP).
    Software,
}

/// One adapter wgpu sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    /// The driver and its version, as the driver gives them.
    pub driver: String,
    /// Vulkan, Metal, Dx12, Gl.
    pub backend: String,
    pub kind: AdapterKind,
}

impl Adapter {
    /// Whether it renders on the CPU: its kind says so, or its name is a
    /// known software rasteriser.
    pub fn software(&self) -> bool {
        let name = self.name.to_lowercase();
        self.kind == AdapterKind::Software
            || [
                "llvmpipe",
                "lavapipe",
                "warp",
                "microsoft basic render",
                "swiftshader",
            ]
            .iter()
            .any(|s| name.contains(s))
    }

    /// The adapter and its driver, for people.
    pub fn described(&self) -> String {
        if self.driver.is_empty() {
            format!("{} ({})", self.name, self.backend)
        } else {
            format!("{} ({}, {})", self.name, self.backend, self.driver)
        }
    }

    /// What its kernels' results are cached by.
    fn key(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}", self.name, self.backend, self.driver)
    }
}

/// The adapters seen, and the one Helios renders on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gpu {
    pub adapters: Vec<Adapter>,
    /// The adapter of the device opened, by its index in `adapters`;
    /// `None`: Helios renders on the CPU.
    pub used: Option<usize>,
    /// Why there is no device, when there is none.
    pub why_cpu: Option<String>,
}

impl Gpu {
    pub fn adapter(&self) -> Option<&Adapter> {
        self.adapters.get(self.used?)
    }

    /// The first hardware adapter, if any.
    pub fn hardware(&self) -> Option<&Adapter> {
        self.adapters.iter().find(|a| !a.software())
    }
}

/// The fix of a GPU finding: the documentation of the GPU choice.
fn docs_fix() -> Fix {
    Fix {
        command: "links.open".into(),
        args: json!({ "target": DOCS }),
        label: Message::new("health-fix-gpu-docs", "How to choose the GPU"),
    }
}

/// The adapter Helios renders on, with its driver: a software one while
/// a hardware one is present, or the CPU while a GPU is there, is a
/// warning.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdapterCheck;

impl Check for AdapterCheck {
    fn id(&self) -> &str {
        ADAPTER
    }

    fn title(&self) -> Message {
        Message::new("health-gpu-adapter", "The GPU")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("The adapter Helios renders on, while a hardware one is present")
            .levels(&[Level::Ok, Level::Warning])
            .fixes(&["links.open"])
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(gpu) = &ctx.gpu else {
            return Vec::new();
        };
        let hardware = gpu.hardware();
        let finding = match (gpu.adapter(), hardware) {
            (Some(a), Some(h)) if a.software() => Finding::new(
                ADAPTER,
                "",
                Level::Warning,
                Message::new(
                    "health-gpu-software",
                    "Helios renders on a software adapter, { $adapter }",
                )
                .with("adapter", a.name.clone()),
                Message::new(
                    "health-gpu-software-detail",
                    "{ $used } runs on the CPU while { $hardware } is present: choose the GPU in the settings.",
                )
                .with("used", a.described())
                .with("hardware", h.described()),
            )
            .fix(docs_fix()),
            (Some(a), _) => Finding::new(
                ADAPTER,
                "",
                Level::Ok,
                Message::new("health-gpu-ok", "Helios renders on { $adapter }")
                    .with("adapter", a.name.clone()),
                Message::new("health-gpu-ok-detail", "{ $used }").with("used", a.described()),
            ),
            (None, Some(h)) => Finding::new(
                ADAPTER,
                "",
                Level::Warning,
                Message::new("health-gpu-cpu", "Helios renders on the CPU"),
                Message::new(
                    "health-gpu-cpu-detail",
                    "{ $hardware } is present but was not opened: { $why }",
                )
                .with("hardware", h.described())
                .with("why", gpu.why_cpu.clone().unwrap_or_default()),
            )
            .fix(docs_fix()),
            (None, None) => Finding::new(
                ADAPTER,
                "",
                Level::Ok,
                Message::new("health-gpu-cpu", "Helios renders on the CPU"),
                Message::new("health-gpu-none-detail", "No GPU was found."),
            ),
        };
        vec![finding]
    }
}

/// One GPU kernel's compilation on the adapter in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kernel {
    /// The module it is the GPU variant of.
    pub module: String,
    /// Why it does not compile; `None`: it does.
    pub error: Option<String>,
}

/// What compiles every kernel on the device open (called on the runner's
/// thread, never the interface's).
pub type Compile = dyn Fn() -> Vec<Kernel> + Send + Sync;

/// Each GPU kernel compiles on the adapter in use; one that does not
/// runs on the CPU, a warning. Compiled once per adapter and driver, so
/// later runs cost nothing.
pub struct Kernels {
    compile: Arc<Compile>,
    cache: Mutex<Option<(String, Vec<Kernel>)>>,
}

impl std::fmt::Debug for Kernels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kernels").finish_non_exhaustive()
    }
}

impl Kernels {
    pub fn new(compile: impl Fn() -> Vec<Kernel> + Send + Sync + 'static) -> Self {
        Self {
            compile: Arc::new(compile),
            cache: Mutex::default(),
        }
    }

    /// The kernels on `adapter`: compiled the first time, cached after.
    fn on(&self, adapter: &Adapter) -> Vec<Kernel> {
        let key = adapter.key();
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((k, kernels)) = cache.as_ref()
            && *k == key
        {
            return kernels.clone();
        }
        let kernels = (self.compile)();
        *cache = Some((key, kernels.clone()));
        kernels
    }
}

impl Check for Kernels {
    fn id(&self) -> &str {
        KERNELS
    }

    fn title(&self) -> Message {
        Message::new("health-gpu-kernels", "GPU kernels")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("Each GPU kernel compiles on the adapter in use")
            .levels(&[Level::Ok, Level::Warning])
            .fixes(&["links.open"])
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(adapter) = ctx.gpu.as_ref().and_then(Gpu::adapter) else {
            return Vec::new();
        };
        let kernels = self.on(adapter);
        let failed: Vec<&Kernel> = kernels.iter().filter(|k| k.error.is_some()).collect();
        if failed.is_empty() {
            return vec![Finding::new(
                KERNELS,
                "",
                Level::Ok,
                Message::new(
                    "health-gpu-kernels-ok",
                    "Every GPU kernel compiles ({ $count })",
                )
                .with("count", kernels.len()),
                Message::new("health-gpu-kernels-ok-detail", "On { $adapter }")
                    .with("adapter", adapter.described()),
            )];
        }
        failed
            .into_iter()
            .map(|k| {
                Finding::new(
                    KERNELS,
                    &k.module,
                    Level::Warning,
                    Message::new("health-gpu-kernel-cpu", "{ $module } falls back to the CPU")
                        .with("module", k.module.clone()),
                    Message::new(
                        "health-gpu-kernel-cpu-detail",
                        "Its GPU kernel does not compile on { $adapter }: { $error }",
                    )
                    .with("adapter", adapter.described())
                    .with("error", k.error.clone().unwrap_or_default()),
                )
                .fix(docs_fix())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn adapter(name: &str, kind: AdapterKind) -> Adapter {
        Adapter {
            name: name.into(),
            driver: "1.0".into(),
            backend: "Vulkan".into(),
            kind,
        }
    }

    fn ctx(adapters: Vec<Adapter>, used: Option<usize>) -> Context {
        Context {
            gpu: Some(Gpu {
                adapters,
                used,
                why_cpu: None,
            }),
            ..Context::default()
        }
    }

    #[test]
    fn a_software_adapter_beside_a_hardware_one_is_a_warning() {
        let both = vec![
            adapter("llvmpipe (LLVM 19)", AdapterKind::Software),
            adapter("Radeon RX 7600", AdapterKind::Discrete),
        ];
        let found = AdapterCheck.run(&ctx(both.clone(), Some(0)));
        assert_eq!(found[0].level, Level::Warning);
        assert_eq!(found[0].fix.as_ref().unwrap().command, "links.open");
        assert_eq!(AdapterCheck.run(&ctx(both, Some(1)))[0].level, Level::Ok);
        // Alone, a software adapter is all there is.
        let alone = vec![adapter("Microsoft Basic Render Driver", AdapterKind::Other)];
        assert_eq!(AdapterCheck.run(&ctx(alone, Some(0)))[0].level, Level::Ok);
        assert!(AdapterCheck.run(&Context::default()).is_empty());
    }

    #[test]
    fn kernels_compile_once_per_adapter_and_failures_are_warnings() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let check = Kernels::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
            vec![
                Kernel {
                    module: "exposure".into(),
                    error: None,
                },
                Kernel {
                    module: "sigmoid".into(),
                    error: Some("WARP refused it".into()),
                },
            ]
        });
        let ctx = ctx(vec![adapter("WARP", AdapterKind::Software)], Some(0));
        let found = check.run(&ctx);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "gpu.kernels/sigmoid");
        assert_eq!(found[0].level, Level::Warning);
        check.run(&ctx);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "cached");
    }
}
