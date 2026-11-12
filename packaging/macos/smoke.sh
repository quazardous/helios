#!/usr/bin/env bash
# Smoke test of a macOS release tarball, run by the release workflow on an
# Apple silicon runner, with no display session:
#
#   packaging/macos/smoke.sh helios-VERSION-macos-arm64.tar.gz PICTURE.jpg
#
# Unpacks it, says what each program links to and how it is signed, then
# runs both programs' versions, an import of the picture into a new vault,
# and a batch that exports it as a JPEG through a network with one module.
# Mirrors packaging/linux/smoke.sh.
set -euo pipefail

tarball="$1"
picture="$2"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

tar -C "$work" -xzf "$tarball"
bin="$(echo "$work"/helios-*/bin)"

echo "== libraries and signatures"
for p in "$bin"/*; do
  echo "-- $(basename "$p")"
  file "$p"
  otool -L "$p"
  codesign --verify --verbose=1 "$p"
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
