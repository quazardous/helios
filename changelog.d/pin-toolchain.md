kind: internal
CI and local builds use the same Rust, 1.95, pinned in rust-toolchain.toml, so a new Clippy release cannot turn the build red without a commit that moves the pin.
