#!/usr/bin/env bash
# Builds the Flatpak of Helios from this source tree into a single-file
# bundle, as the release workflow (.github/workflows/release.yml) and the
# bench box's `flatpak` job do:
#
#   packaging/flatpak/build.sh all VERSION OUT_DIR [WORK_DIR]
#   packaging/flatpak/build.sh fetch WORK_DIR                  (network)
#   packaging/flatpak/build.sh build VERSION OUT_DIR WORK_DIR  (no network)
#
# `all` does both. `fetch` generates cargo-sources.json, installs the
# runtime, the SDK and its Rust extension for the user, and downloads every
# source; `build` then builds offline from WORK_DIR and writes OUT_DIR/
# io.github.quazardous.Helios-VERSION-x86_64.flatpak. WORK_DIR (default for
# `all`: a temporary folder, removed at the end) keeps flatpak-builder's
# downloads; the build's own files are removed after each build.
#
# Needs flatpak and flatpak-builder (the distribution's packages or the
# org.flatpak.Builder Flatpak), appstreamcli, and for `fetch` python3 with
# aiohttp, tomlkit and PyYAML (or uv or pipx, which install them). The
# crates are listed with their checksums in cargo-sources.json, generated
# from Cargo.lock by flatpak-builder-tools' cargo generator, pinned by
# commit and SHA-256, and never committed.
set -euo pipefail

phase="$1"
shift
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
id=io.github.quazardous.Helios
manifest="$here/$id.yml"
flathub=https://dl.flathub.org/repo/flathub.flatpakrepo

generator_url=https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/9d86d336a39b9af38b86f114b6ce5820fe260f91/cargo/flatpak-cargo-generator.py
generator_sha256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380

case "$phase" in
  fetch) version="" out="" dir="$1" ;;
  build) version="$1" out="$2" dir="$3" ;;
  all) version="$1" out="$2" dir="${3:-}" ;;
  *)
    echo "usage: build.sh all VERSION OUT_DIR [WORK_DIR] | fetch WORK_DIR | build VERSION OUT_DIR WORK_DIR" >&2
    exit 2
    ;;
esac
if [ -n "$dir" ]; then
  mkdir -p "$dir"
  work="$(cd "$dir" && pwd)"
else
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
fi
state="$work/state"

if command -v flatpak-builder >/dev/null; then
  builder=(flatpak-builder)
elif flatpak info org.flatpak.Builder >/dev/null 2>&1; then
  # Its host calls would otherwise look for the runtimes in its own data
  # folder, not the user's installation.
  builder=(flatpak run --env=FLATPAK_USER_DIR="${FLATPAK_USER_DIR:-$HOME/.local/share/flatpak}"
    --command=flatpak-builder org.flatpak.Builder)
else
  echo "flatpak-builder is missing: install it, or org.flatpak.Builder from Flathub" >&2
  exit 1
fi

fetch() {
  echo "== crates"
  curl -fsSL --retry 3 -o "$work/flatpak-cargo-generator.py" "$generator_url"
  echo "$generator_sha256  $work/flatpak-cargo-generator.py" | sha256sum -c -
  if command -v uv >/dev/null; then
    uv run -q "$work/flatpak-cargo-generator.py" "$root/Cargo.lock" -o "$work/cargo-sources.json"
  elif command -v pipx >/dev/null; then
    pipx run "$work/flatpak-cargo-generator.py" "$root/Cargo.lock" -o "$work/cargo-sources.json"
  else
    python3 "$work/flatpak-cargo-generator.py" "$root/Cargo.lock" -o "$work/cargo-sources.json"
  fi
  cp "$work/cargo-sources.json" "$here/cargo-sources.json"

  echo "== runtime"
  # The runtime, the SDK and its Rust extension at the manifest's version
  # (installed here rather than by flatpak-builder, which cannot from
  # inside the org.flatpak.Builder sandbox).
  local branch
  branch="$(sed -n "s/^runtime-version: '\(.*\)'$/\1/p" "$manifest")"
  flatpak remote-add --user --if-not-exists flathub "$flathub"
  flatpak install --user -y --noninteractive flathub org.freedesktop.Platform//"$branch" \
    org.freedesktop.Sdk//"$branch" org.freedesktop.Sdk.Extension.rust-stable//"$branch"
  flatpak update --user -y --noninteractive

  echo "== sources"
  "${builder[@]}" --user --download-only --state-dir="$state" "$work/build" "$manifest"
}

build() {
  echo "== metadata"
  appstreamcli validate --no-net "$root/packaging/linux/$id.metainfo.xml"
  if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$root/packaging/linux/$id.desktop"
  fi
  if ! grep -q "<release version=\"$version\"" "$root/packaging/linux/$id.metainfo.xml"; then
    echo "warning: the metainfo has no <release> for $version" >&2
  fi

  echo "== build"
  # The crates' list of the fetch, wherever this tree was checked out.
  cp "$work/cargo-sources.json" "$here/cargo-sources.json"
  mkdir -p "$out"
  "${builder[@]}" --user --disable-download --disable-cache --force-clean --disable-rofiles-fuse \
    --state-dir="$state" --repo="$work/repo" "$work/build" "$manifest"

  echo "== bundle"
  local bundle="$id-$version-x86_64.flatpak"
  flatpak build-bundle --runtime-repo="$flathub" "$work/repo" "$out/$bundle" "$id"
  ls -l "$out/$bundle"
  # Only the downloads are worth keeping.
  rm -rf "$work/build" "$work/repo" "$state/build" "$state/cache" "$state/rofiles"
}

case "$phase" in
  fetch) fetch ;;
  build) build ;;
  all)
    fetch
    build
    ;;
esac
