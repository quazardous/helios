kind: internal

`cargo xtask deps-worlds`, run by `cargo xtask lint`, holds every crate to its UI world (no egui in modules, plugins or the engine; forms through helios-ui, visual modules through helios-rack) and Cargo.lock to a single egui-pop; today's exceptions are listed with their reasons.
