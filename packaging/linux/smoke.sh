#!/usr/bin/env bash
# Smoke test of a Linux release tarball, run by the release workflow in a
# bare Ubuntu container (no Vulkan loader, no GPU):
#
#   packaging/linux/smoke.sh helios-VERSION-linux-x86_64.tar.gz PICTURE.jpg
#
# Unpacks it, says what each program links to, then runs the command line:
# its version, an import of the picture into a new vault, and a batch that
# exports it as a JPEG through a network with one module (the processing
# runs on the CPU, as there is no GPU).
set -euo pipefail

tarball="$1"
picture="$2"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

tar -C "$work" -xzf "$tarball"
bin="$(echo "$work"/helios-*/bin)"

echo "== libraries"
for p in "$bin"/*; do
  echo "-- $(basename "$p")"
  ldd "$p" || true
done
if ldconfig -p | grep -q libvulkan; then
  echo "a Vulkan loader is installed"
else
  echo "no Vulkan loader here: the processing runs on the CPU"
fi

echo "== version"
"$bin/helios" --version
"$bin/helios" --version | grep -q '^helios [0-9]'

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
echo "exported $(stat -c %s "$out") bytes to $(basename "$out")"

echo "== the window's program"
# It needs the desktop's libraries (listed above); its version runs before
# it opens anything.
"$bin/helios-gui" --version || echo "helios-gui does not start here (see its libraries above)"
echo "smoke test passed"
