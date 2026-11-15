kind: internal

The controls gallery's presets snapshots render the same built alone and in the workspace: helios-ui names egui-pop's `phosphor` icons and the gallery egui's emoji fonts itself, and `cargo xtask deps-worlds` now refuses a crate with committed snapshots whose drawing features (icon font, font sets) differ between the two builds.
