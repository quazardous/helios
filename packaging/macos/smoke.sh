#!/usr/bin/env bash
# Smoke test of the macOS disk image, run by the release workflow on an
# Apple silicon runner, with no display session:
#
#   packaging/macos/smoke.sh helios-VERSION-macos-arm64.dmg PICTURE.jpg
#
# Mounts the image read-only, checks Helios.app (its Info.plist, icon and
# signature), says what each program links to, then runs from the mounted
# app both programs' versions, an import of the picture into a new vault,
# and a batch that exports it as a JPEG through a network with one module.
# Mirrors packaging/linux/smoke.sh.
set -euo pipefail

dmg="$1"
picture="$2"
work="$(mktemp -d)"
mnt="$work/mnt"
mkdir -p "$mnt"
cleanup() {
  hdiutil detach "$mnt" -force >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

echo "== disk image"
hdiutil verify "$dmg"
hdiutil attach "$dmg" -readonly -nobrowse -noautoopen -mountpoint "$mnt"
ls -la "$mnt"
app="$mnt/Helios.app"
bin="$app/Contents/MacOS"
for f in Applications COPYING THIRD-PARTY-LICENSES.txt README.md; do
  [ -e "$mnt/$f" ] || {
    echo "the image has no $f"
    exit 1
  }
done

echo "== app bundle"
plutil -lint "$app/Contents/Info.plist"
[ "$(defaults read "$app/Contents/Info.plist" CFBundleIdentifier)" = io.github.quazardous.Helios ]
[ "$(defaults read "$app/Contents/Info.plist" CFBundleExecutable)" = helios-gui ]
defaults read "$app/Contents/Info.plist" CFBundleShortVersionString
[ -s "$app/Contents/Resources/Helios.icns" ]
codesign --verify --deep --strict --verbose=2 "$app"
codesign --display --verbose=2 "$app" 2>&1 | grep -E '^(Identifier|Signature|TeamIdentifier)'
# Gatekeeper's verdict, for the log: an app signed ad hoc is rejected
# until it is signed with a Developer ID and notarised.
spctl --assess --type execute --verbose=2 "$app" || true

echo "== libraries"
for p in "$bin"/*; do
  echo "-- $(basename "$p")"
  file "$p"
  otool -L "$p"
done

echo "== version"
"$bin/helios" --version
"$bin/helios" --version | grep -q '^helios [0-9]'
# The window's program says its version before it opens anything.
"$bin/helios-gui" --version
"$bin/helios-gui" --version | grep -q '^helios-gui [0-9]'

echo "== import"
mkdir -p "$work/photos"
cp "$picture" "$work/photos/"
"$bin/helios" import --vault "$work/library.helios" "$work/photos" | tee "$work/import.txt"
grep -q '"report"' "$work/import.txt"

echo "== export"
cat >"$work/network.json" <<'EOF'
{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}
EOF
"$bin/helios" batch --network "$work/network.json" --out "$work/out" --format jpeg \
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
echo "exported $(wc -c <"$out" | tr -d ' ') bytes to $(basename "$out")"
echo "smoke test passed"
