#!/usr/bin/env bash
# The prototype launched inside the compositor, in bench mode. bench.sh
# chooses it and where its measures go (HELIOS_PROTO_*).
here="$(cd "$(dirname "$0")" && pwd)"
bin="$here/../target/release/helios-proto-${HELIOS_PROTO_KIT:?egui or iced}"
exec "$bin" --bench "${HELIOS_PROTO_FRAMES:-400}" \
  --layout "$HELIOS_PROTO_OUT/layout-$HELIOS_PROTO_KIT.json" \
  > "$HELIOS_PROTO_OUT/bench-$HELIOS_PROTO_KIT.json" \
  2> "$HELIOS_PROTO_OUT/bench-$HELIOS_PROTO_KIT.log"
