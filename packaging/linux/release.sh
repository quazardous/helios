#!/usr/bin/env bash
# Packages a release build of Helios for Linux x86_64, as the release
# workflow does (.github/workflows/release.yml):
#
#   packaging/linux/release.sh VERSION BUILD_DIR OUT_DIR
#
# BUILD_DIR holds the programs built with `cargo build --release`. OUT_DIR
# receives:
# - helios-VERSION-linux-x86_64.tar.gz: a folder that runs in place
#   (bin/helios-gui, bin/helios...);
# - Helios-VERSION-x86_64.AppImage: one file, the window by default, the
#   command line as `Helios.AppImage helios ...`;
# - SHA256SUMS of both.
# Both carry COPYING (the GPLv3) and THIRD-PARTY-LICENSES.txt (the crates
# built in and their licenses, from `cargo deny list`, which must be
# installed). The AppImage tools are downloaded from their releases and
# checked against the checksums below.
set -euo pipefail

version="$1"
build="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
id=io.github.quazardous.Helios
programs="helios helios-node helios-mcp helios-gui helios-darktable-library"
sizes="16 24 32 48 64 128 256 512"

# appimagetool and the AppImage runtime it puts in front of the image (the
# static one: no libfuse2 needed on the user's system).
appimagetool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
appimagetool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
runtime_url=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
runtime_sha256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The licences, shared by both packages.
cp "$root/LICENSE" "$work/COPYING"
{
  echo "Helios $version is free software under the GNU General Public License,"
  echo "version 3 or later (COPYING). It is built with the Rust crates below,"
  echo "listed by license (from \`cargo deny list\`); each crate's own license"
  echo "text comes with its sources on crates.io."
  echo
  (cd "$root" && cargo deny --exclude xtask list --layout license --format human)
} >"$work/THIRD-PARTY-LICENSES.txt"

# The tarball: bin/, share/ (desktop entry, icons), the licences.
name="helios-$version-linux-x86_64"
tree="$work/$name"
mkdir -p "$tree/bin"
for p in $programs; do
  install -m755 "$build/$p" "$tree/bin/$p"
  strip "$tree/bin/$p"
done
install -Dm644 "$here/$id.desktop" "$tree/share/applications/$id.desktop"
install -Dm644 "$here/$id.metainfo.xml" "$tree/share/metainfo/$id.metainfo.xml"
install -Dm644 "$here/icons/$id.svg" "$tree/share/icons/hicolor/scalable/apps/$id.svg"
for n in $sizes; do
  install -Dm644 "$here/icons/$id-$n.png" "$tree/share/icons/hicolor/${n}x${n}/apps/$id.png"
done
cp "$work/COPYING" "$work/THIRD-PARTY-LICENSES.txt" "$root/README.md" "$tree/"
tar -C "$work" -czf "$out/$name.tar.gz" --owner=0 --group=0 "$name"

# The AppImage: the same tree as usr/, the desktop entry and icon at the
# top, and AppRun.
app="$work/Helios.AppDir"
mkdir -p "$app"
cp -a "$tree" "$app/usr"
cp "$here/$id.desktop" "$app/$id.desktop"
cp "$here/icons/$id-256.png" "$app/$id.png"
ln -s "$id.png" "$app/.DirIcon"
install -m755 "$here/AppRun" "$app/AppRun"

fetch() {
  curl -fsSL --retry 3 -o "$2" "$1"
  echo "$3  $2" | sha256sum -c -
}
fetch "$appimagetool_url" "$work/appimagetool" "$appimagetool_sha256"
fetch "$runtime_url" "$work/runtime" "$runtime_sha256"
chmod +x "$work/appimagetool"
appimage="Helios-$version-x86_64.AppImage"
# The tool is an AppImage itself: extracted and run, so no FUSE is needed.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$work/appimagetool" --no-appstream \
  --runtime-file "$work/runtime" "$app" "$out/$appimage"

(cd "$out" && sha256sum "$name.tar.gz" "$appimage" >SHA256SUMS && cat SHA256SUMS)
