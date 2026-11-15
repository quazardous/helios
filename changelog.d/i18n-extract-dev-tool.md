kind: internal

The i18n extractor crate is named for what it does (`helios-i18n-extract`, was `helios-declarations`), and `cargo xtask deps-worlds` calls the developer tools' world `dev-tool` and checks it never ships: each of its crates is `publish = false`, and no crate of another world depends on one (a dev-dependency may).
