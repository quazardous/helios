//! The controls the desktop uses (`helios_controls`): Helios's catalogue
//! with the modules' patches (exposure's `exposure` is the exposure
//! control; every other parameter keeps its implicit control, the row
//! `params` draws from its descriptor), and the `controls.change` step's
//! hooks. One of each for the process: the panel, the `controls.*`
//! commands and the `controls` collector share them.

use std::sync::{Arc, LazyLock, Mutex};

use helios_controls::Registry;
use helios_controls::flow::{SharedSteps, Steps};

static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| {
    let mut r = Registry::standard();
    let errors = r.patch(helios_module_exposure::patches());
    debug_assert!(errors.is_empty(), "{errors:?}");
    Arc::new(r)
});

static STEPS: LazyLock<SharedSteps> = LazyLock::new(|| Arc::new(Mutex::new(Steps::new())));

/// The registry: controls and patches.
pub fn registry() -> &'static Arc<Registry> {
    &REGISTRY
}

/// The `controls.change` step: hooks that may veto or redirect a finished
/// change of a control.
pub fn steps() -> &'static SharedSteps {
    &STEPS
}
