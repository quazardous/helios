kind: internal

`cargo xtask install-dev` installs "Helios Dev" as a Flatpak for the user (`io.github.quazardous.Helios.Devel`, its own data, the release app's permissions), built from any branch by the bench box's `flatpak-devel` job and checked against its SHA-256; the home folder install stays behind `--home`.
