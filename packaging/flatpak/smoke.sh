#!/usr/bin/env bash
# Smoke test of the Flatpak bundle, run by the release workflow on a
# runner with no GPU and no display:
#
#   packaging/flatpak/smoke.sh io.github.quazardous.Helios-VERSION-x86_64.flatpak PICTURE.jpg
#
# Installs the bundle for the current user (its runtime from Flathub),
# runs the command line in the sandbox as packaging/linux/smoke.sh runs
# the tarball's (its version, an import of the picture into a new vault,
# a batch that exports it as a JPEG on the CPU), checks the window's
# program says its version, and uninstalls it.
set -euo pipefail

bundle="$1"
picture="$2"
id=io.github.quazardous.Helios
mkdir -p "$HOME"
work="$(mktemp -d "$HOME/helios-flatpak-smoke.XXXXXX")"
cleanup() {
  flatpak uninstall --user -y --noninteractive "$id" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

echo "== install"
# The runtime comes from Flathub, unless it is installed already (the
# bench box runs this offline, after `build.sh fetch`).
flatpak remotes --user | grep -q '^flathub' ||
  flatpak remote-add --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y --noninteractive "$bundle"
flatpak info --user "$id"
flatpak info --user --show-permissions "$id"

# The sandbox sees the test's folder only when it is granted.
helios() {
  flatpak run --user --filesystem="$work" --command=helios "$id" "$@"
}

echo "== version"
helios --version | tee "$work/version.txt"
grep -q '^helios [0-9]' "$work/version.txt"

echo "== import"
mkdir -p "$work/photos"
cp "$picture" "$work/photos/"
helios import --vault "$work/library.helios" "$work/photos" | tee "$work/import.txt"
grep -q '"report"' "$work/import.txt"

echo "== export"
cat >"$work/network.json" <<'EOF'
{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}
EOF
helios batch --network "$work/network.json" --out "$work/out" --format jpeg \
  "$work/photos/$(basename "$picture")" | tee "$work/batch.txt"
out="$work/out/$(basename "${picture%.*}").jpg"
[ -s "$out" ] || {
  echo "no export at $out"
  exit 1
}
# A JPEG starts with FF D8 FF.
[ "$(head -c 3 "$out" | od -An -tx1 | tr -d ' \n')" = ffd8ff ] || {
  echo "$out is not a JPEG"
  exit 1
}
echo "exported $(stat -c %s "$out") bytes to $(basename "$out")"

echo "== the window's program"
# Its version is printed before it opens anything.
flatpak run --user "$id" --version | tee "$work/gui.txt"
grep -q '^helios-gui [0-9]' "$work/gui.txt"
echo "smoke test passed"
