#!/usr/bin/env bash
# Packages a release build of Helios for macOS on Apple silicon (arm64), as
# the release workflow's macOS job does (.github/workflows/release.yml):
#
#   packaging/macos/release.sh VERSION BUILD_DIR OUT_DIR
#
# BUILD_DIR holds the programs built with `cargo build --release` for
# aarch64-apple-darwin. OUT_DIR receives:
# - helios-VERSION-macos-arm64.dmg: a disk image holding Helios.app (the
#   window, Contents/MacOS/helios-gui; the command line and the plugins'
#   programs beside it), a link to /Applications, COPYING (the GPLv3),
#   THIRD-PARTY-LICENSES.txt (the crates built in, from `cargo deny list`,
#   which must be installed) and README.md;
# - SHA256SUMS-macos of it.
#
# Signing. By default the programs and the app are signed ad hoc only
# (Apple silicon runs nothing unsigned): no Developer ID, no notarisation,
# so Gatekeeper asks the user to confirm the first start. Two hooks, both
# skipped when their variables are unset:
# - MACOS_SIGN_IDENTITY: a Developer ID Application identity in the
#   keychain (its name or SHA-1); the code is then signed with it, with the
#   hardened runtime and a secure timestamp.
# - Notarisation of the disk image, then stapling, when the image is signed
#   with a Developer ID and either APPLE_NOTARY_KEY (path to an App Store
#   Connect API key, .p8), APPLE_NOTARY_KEY_ID and APPLE_NOTARY_ISSUER_ID,
#   or APPLE_ID, APPLE_APP_PASSWORD and APPLE_TEAM_ID are set.
set -euo pipefail

version="$1"
build="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
programs="helios helios-node helios-mcp helios-gui helios-darktable-library helios-lightroom-catalog helios-google-photos"
identity="${MACOS_SIGN_IDENTITY:-}"
# The bundle's version is the workspace's, whatever the file is named (a
# dry run names it after its tag).
bundle_version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1)"

mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

sign() {
  if [ -n "$identity" ]; then
    codesign --force --options runtime --timestamp --sign "$identity" "$@"
  else
    codesign --force --sign - "$@"
  fi
}

# Helios.app: Info.plist, the icon, the programs side by side in
# Contents/MacOS (Helios finds its programs and bundled plugins beside its
# own executable), the licences in Contents/Resources.
stage="$work/stage"
app="$stage/Helios.app"
macos="$app/Contents/MacOS"
res="$app/Contents/Resources"
mkdir -p "$macos" "$res"
sed "s/@VERSION@/$bundle_version/g" "$here/Info.plist" >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' >"$app/Contents/PkgInfo"

# The icon, from the Linux icon set's sizes.
iconset="$work/Helios.iconset"
icons="$root/packaging/linux/icons/io.github.quazardous.Helios"
mkdir -p "$iconset"
for n in 16 32 128 256 512; do
  cp "$icons-$n.png" "$iconset/icon_${n}x${n}.png"
done
cp "$icons-32.png" "$iconset/icon_16x16@2x.png"
cp "$icons-64.png" "$iconset/icon_32x32@2x.png"
cp "$icons-256.png" "$iconset/icon_128x128@2x.png"
cp "$icons-512.png" "$iconset/icon_256x256@2x.png"
iconutil -c icns -o "$res/Helios.icns" "$iconset"

for p in $programs; do
  install -m755 "$build/$p" "$macos/$p"
  strip -x "$macos/$p"
done

# Libraries outside the system's (today there are none: the C Helios
# carries is linked in statically) are copied into Contents/Frameworks and
# the programs pointed at them. A library that needs another one outside
# the system stops the packaging: bundle it here first.
system_lib() {
  case "$1" in
  /System/Library/* | /usr/lib/* | @executable_path/* | @rpath/* | @loader_path/*) return 0 ;;
  *) return 1 ;;
  esac
}
for p in $programs; do
  otool -L "$macos/$p" | tail -n +2 | awk '{print $1}' | while read -r lib; do
    system_lib "$lib" && continue
    name="$(basename "$lib")"
    mkdir -p "$app/Contents/Frameworks"
    if [ ! -e "$app/Contents/Frameworks/$name" ]; then
      install -m644 "$lib" "$app/Contents/Frameworks/$name"
      install_name_tool -id "@rpath/$name" "$app/Contents/Frameworks/$name"
      otool -L "$app/Contents/Frameworks/$name" | tail -n +3 | awk '{print $1}' | while read -r dep; do
        system_lib "$dep" || {
          echo "$name needs $dep, which is not bundled" >&2
          exit 1
        }
      done
    fi
    echo "bundled $lib for $p"
    install_name_tool -change "$lib" "@executable_path/../Frameworks/$name" "$macos/$p"
  done
done
if [ -d "$app/Contents/Frameworks" ]; then
  sign "$app/Contents/Frameworks"/*
fi

cp "$root/LICENSE" "$res/COPYING"
{
  echo "Helios $version is free software under the GNU General Public License,"
  echo "version 3 or later (COPYING). It is built with the Rust crates below,"
  echo "listed by license (from \`cargo deny list\`); each crate's own license"
  echo "text comes with its sources on crates.io."
  echo
  (cd "$root" && cargo deny --exclude xtask list --layout license --format human)
} >"$res/THIRD-PARTY-LICENSES.txt"

# The helpers first, then the app, which seals Info.plist and resources.
for p in $programs; do
  [ "$p" = helios-gui ] || sign "$macos/$p"
done
sign "$app"
codesign --verify --strict --verbose=2 "$app"

# The disk image: the app, a link to drop it on, the licences and README.
ln -s /Applications "$stage/Applications"
cp "$res/COPYING" "$res/THIRD-PARTY-LICENSES.txt" "$root/README.md" "$stage/"
dmg="helios-$version-macos-arm64.dmg"
rm -f "$out/$dmg"
# hdiutil fails now and then on CI runners ("Resource busy"): try again.
for try in 1 2 3; do
  if hdiutil create -volname "Helios $version" -srcfolder "$stage" -fs HFS+ \
    -format UDZO -imagekey zlib-level=9 -ov "$out/$dmg"; then
    break
  fi
  [ "$try" = 3 ] && exit 1
  sleep 10
done
if [ -n "$identity" ]; then
  codesign --force --timestamp --sign "$identity" "$out/$dmg"
fi

# Notarisation: only with a Developer ID and the notary's credentials.
notary=()
if [ -n "${APPLE_NOTARY_KEY:-}" ] && [ -n "${APPLE_NOTARY_KEY_ID:-}" ] && [ -n "${APPLE_NOTARY_ISSUER_ID:-}" ]; then
  notary=(--key "$APPLE_NOTARY_KEY" --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID")
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  notary=(--apple-id "$APPLE_ID" --password "$APPLE_APP_PASSWORD" --team-id "$APPLE_TEAM_ID")
fi
if [ -n "$identity" ] && [ "${#notary[@]}" -gt 0 ]; then
  xcrun notarytool submit "$out/$dmg" "${notary[@]}" --wait
  xcrun stapler staple "$out/$dmg"
else
  echo "not notarised: no Developer ID identity or no notary credentials (docs/releases.md, \"macOS\")"
fi

(cd "$out" && shasum -a 256 "$dmg" >SHA256SUMS-macos && cat SHA256SUMS-macos)
