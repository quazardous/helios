#!/usr/bin/env bash
# Builds the Flatpak of Helios from this source tree into a single-file
# bundle, as the release workflow (.github/workflows/release.yml) and the
# bench box's `flatpak` job do:
#
#   packaging/flatpak/build.sh all VERSION OUT_DIR [WORK_DIR]
#   packaging/flatpak/build.sh fetch WORK_DIR                  (network)
#   packaging/flatpak/build.sh build VERSION OUT_DIR WORK_DIR  (no network)
#   packaging/flatpak/build.sh version                         (prints it)
#
# With HELIOS_FLATPAK_DEVEL=1, `build` makes the development app instead,
# io.github.quazardous.Helios.Devel ("Helios Dev"): the same manifest under
# its own id, so it installs beside the release app with its own data
# (~/.var/app/<id>), versioned VERSION. `version` prints this tree's:
# `X.Y.Z` on a release tag, else the next patch's `X.Y.Z-dev.N+<commit>`,
# as helios-gui's build.rs counts it. The bench box's `flatpak-devel` job
# builds it for `cargo xtask install-dev`.
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
release_id=io.github.quazardous.Helios
manifest="$here/$release_id.yml"
id=$release_id
[ "${HELIOS_FLATPAK_DEVEL:-}" != 1 ] || id=$release_id.Devel
flathub=https://dl.flathub.org/repo/flathub.flatpakrepo

generator_url=https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/9d86d336a39b9af38b86f114b6ce5820fe260f91/cargo/flatpak-cargo-generator.py
generator_sha256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380

# This tree's version, as helios-gui's build.rs makes it from `git describe`.
dev_version() {
  local pkg d sha count tag major minor patch
  pkg="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1)"
  if ! d="$(git -C "$root" describe --tags --long --match 'v[0-9]*' 2>/dev/null)"; then
    sha="$(git -C "$root" rev-parse --short=8 HEAD 2>/dev/null || echo 0)"
    echo "$pkg-dev.0+$sha"
    return
  fi
  sha="${d##*-g}"
  d="${d%-g*}"
  count="${d##*-}"
  tag="${d%-*}"
  tag="${tag#v}"
  if [ "$count" = 0 ]; then
    echo "$tag"
    return
  fi
  IFS=. read -r major minor patch <<<"$tag"
  patch="${patch%%[-+]*}"
  echo "$major.$minor.$((patch + 1))-dev.$count+$sha"
}

case "$phase" in
  version)
    dev_version
    exit 0
    ;;
  fetch) version="" out="" dir="$1" ;;
  build) version="$1" out="$2" dir="$3" ;;
  all) version="$1" out="$2" dir="${3:-}" ;;
  *)
    echo "usage: build.sh all VERSION OUT_DIR [WORK_DIR] | fetch WORK_DIR | build VERSION OUT_DIR WORK_DIR | version" >&2
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
  # A build that failed leaves its folder: flatpak-builder refuses a full one.
  rm -rf "$work/build"
  "${builder[@]}" --user --download-only --state-dir="$state" "$work/build" "$manifest"
}

# The Devel app's manifest, generated beside the release one (and never
# committed): the release manifest under its own id, its version given to
# the build, and a last module that renames what the app exports (desktop
# entry, metainfo, icons) to that id, named "Helios Dev" and opening no
# file types. Its permissions are the release app's, unchanged.
devel_manifest() {
  local devel="$here/$id.yml"
  # The modules are the release manifest's last key: the module added at
  # the end of the file joins them.
  [ "$(grep -E '^[a-z-]+:' "$manifest" | tail -n1)" = "modules:" ] || {
    echo "the manifest's last key is not modules:" >&2
    exit 1
  }
  {
    sed -e "s/^app-id: .*/app-id: $id/" \
      -e "s/^\(    CARGO_HOME: .*\)/    HELIOS_BUILD_VERSION: '$version'\n\1/" "$manifest"
    cat <<'EOF'
  # The development app: what it exports carries its own id.
  - name: devel
    buildsystem: simple
    build-commands:
      - |
        set -e
        rel=io.github.quazardous.Helios
        cd "$FLATPAK_DEST/share"
        sed -e 's/^Name=.*/Name=Helios Dev/' -e "s/^Icon=.*/Icon=$FLATPAK_ID/" -e '/^MimeType=/d' \
          "applications/$rel.desktop" >"applications/$FLATPAK_ID.desktop"
        sed -e "s|<id>$rel</id>|<id>$FLATPAK_ID</id>|" -e "s|>$rel.desktop<|>$FLATPAK_ID.desktop<|" \
          -e 's|<name>Helios</name>|<name>Helios Dev</name>|' \
          "metainfo/$rel.metainfo.xml" >"metainfo/$FLATPAK_ID.metainfo.xml"
        rm "applications/$rel.desktop" "metainfo/$rel.metainfo.xml"
        for f in $(find icons -name "$rel.*"); do mv "$f" "${f%/*}/$FLATPAK_ID.${f##*.}"; done
        mv "licenses/$rel" "licenses/$FLATPAK_ID"
EOF
  } >"$devel"
  if ! grep -q "^app-id: $id$" "$devel" || ! grep -q "HELIOS_BUILD_VERSION: '$version'" "$devel"; then
    echo "could not make $devel" >&2
    exit 1
  fi
  manifest="$devel"
}

build() {
  echo "== metadata"
  appstreamcli validate --no-net "$root/packaging/linux/$release_id.metainfo.xml"
  if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$root/packaging/linux/$release_id.desktop"
  fi
  [ "$id" = "$release_id" ] || devel_manifest
  if [ "$id" = "$release_id" ] && ! grep -q "<release version=\"$version\"" "$root/packaging/linux/$release_id.metainfo.xml"; then
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
