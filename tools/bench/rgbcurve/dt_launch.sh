#!/usr/bin/env bash
# darktable as launched inside the wbox compositor for the rgbcurve scroll
# scenario. Everything it needs comes from the env file the scenario writes
# (RGBC_ENV), so nothing depends on how wbox passes the environment on.
set -u
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1090
. "${RGBC_ENV:-$here/run.env}"

mkdir -p "$RGBC_CONFIGDIR" "$RGBC_CACHEDIR" "$RGBC_OUT"
export DBUS_SESSION_BUS_ADDRESS="$RGBC_BUS"
# ASan: keep running after a report (needs -fsanitize-recover=address at
# build time), one log file per process under the output folder, no leak
# report (darktable leaks at exit by design, it would drown the signal)
export ASAN_OPTIONS="detect_leaks=0:halt_on_error=0:abort_on_error=0:log_path=$RGBC_OUT/asan:symbolize=1"
exec "$RGBC_PREFIX/bin/darktable" \
  --configdir "$RGBC_CONFIGDIR" --cachedir "$RGBC_CACHEDIR" \
  --disable-opencl --conf ui/show_welcome_screen=false \
  > "$RGBC_OUT/darktable.log" 2>&1
