#!/usr/bin/env bash
# Benchmarks each GUI prototype offscreen in wbox, one after the other, and
# prints their measures (JSON, one line per toolkit).
#
#   proto/wbox/bench.sh [OUTDIR] [FRAMES]
#
# Needs wbox-mcp installed (its Python runs wbox_ctl.py) and the prototypes
# built in release (`cargo build --release` in proto/).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="$(mkdir -p "${1:-$here/out}" && cd "${1:-$here/out}" && pwd)"
frames="${2:-400}"
python="$(head -1 "$(command -v wbox-mcp)" | sed 's/^#!//')"
ctl() { "$python" "$here/wbox_ctl.py" "$@"; }

for kit in egui iced; do
  rm -f "$out/bench-$kit.json"
  export HELIOS_PROTO_KIT="$kit" HELIOS_PROTO_OUT="$out" HELIOS_PROTO_FRAMES="$frames"
  ctl up "$here/config.yaml" > "$out/wbox-$kit.json"
  # The prototype quits by itself once measured; its JSON line is the signal.
  for _ in $(seq 1 600); do
    [ -s "$out/bench-$kit.json" ] && break
    sleep 0.2
  done
  ctl down "$here/config.yaml" > /dev/null || true
  if [ -s "$out/bench-$kit.json" ]; then
    head -1 "$out/bench-$kit.json"
  else
    echo "{\"toolkit\": \"$kit\", \"error\": \"no measures, see $out/bench-$kit.log\"}"
  fi
done
