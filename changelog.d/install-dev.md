kind: internal

`cargo xtask install-dev` installs a dev build of Helios outside the checkout, with its "Helios Dev" launcher, so cleaning `target/` no longer breaks it; `install.sh --dev` and `tools/bench/fetch-gui.sh` are gone.
