#!/usr/bin/env bash
# Packages a release build of Helios for macOS on Apple silicon (arm64), as
# the release workflow's macOS job does (.github/workflows/release.yml):
#
#   packaging/macos/release.sh VERSION BUILD_DIR OUT_DIR
#
# BUILD_DIR holds the programs built with `cargo build --release` for
# aarch64-apple-darwin. OUT_DIR receives:
# - helios-VERSION-macos-arm64.tar.gz: a folder that runs in place
#   (bin/helios-gui, bin/helios...), with COPYING (the GPLv3),
#   THIRD-PARTY-LICENSES.txt (the crates built in, from `cargo deny list`,
#   which must be installed) and README.md;
# - SHA256SUMS-macos of it.
# The programs are signed ad hoc only (no Developer ID, no notarisation):
# Apple silicon runs nothing unsigned, and stripping drops the linker's own
# ad hoc signature. An app bundle and a disk image come later.
set -euo pipefail

version="$1"
build="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
programs="helios helios-node helios-mcp helios-gui helios-darktable-library"

mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

name="helios-$version-macos-arm64"
tree="$work/$name"
mkdir -p "$tree/bin"
for p in $programs; do
  install -m755 "$build/$p" "$tree/bin/$p"
  strip -x "$tree/bin/$p"
  codesign --force --sign - "$tree/bin/$p"
done

cp "$root/LICENSE" "$tree/COPYING"
cp "$root/README.md" "$tree/"
{
  echo "Helios $version is free software under the GNU General Public License,"
  echo "version 3 or later (COPYING). It is built with the Rust crates below,"
  echo "listed by license (from \`cargo deny list\`); each crate's own license"
  echo "text comes with its sources on crates.io."
  echo
  (cd "$root" && cargo deny --exclude xtask list --layout license --format human)
} >"$tree/THIRD-PARTY-LICENSES.txt"

tar -C "$work" -czf "$out/$name.tar.gz" --uid 0 --gid 0 "$name"
(cd "$out" && shasum -a 256 "$name.tar.gz" >SHA256SUMS-macos && cat SHA256SUMS-macos)
