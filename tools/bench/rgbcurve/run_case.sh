#!/usr/bin/env bash
# One rgbcurve scroll session against one darktable build.
#
#   run_case.sh <prefix> <B|C> <image> <outdir>
#
# Starts darktable (ASan build) headless in wbox on a private D-Bus and a
# throwaway library, opens <image> in the darkroom, enables and expands
# rgb curve through Lua, reads the history back (before), runs the input
# step for the case, flushes and reads the history back (after), quits,
# reads it once more (final), and collects the ASan reports.
#
# The input step runs drive.py: `locate` finds the rgb curve graph (or takes
# RGBC_GRAPH="x0,y0,x1,y1") and writes graph.json, then `caseB` or `caseC`
# sends the pointer, wheel and key events and takes a screenshot. Their JSON
# lines go to drive.jsonl.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
prefix="$(realpath "$1")"
case_id="$2"
image="$(realpath "$3")"
out="$(realpath -m "$4")"
wbox_python="${WBOX_PYTHON:?set WBOX_PYTHON to the wbox interpreter}"
config="$here/wbox.yaml"
case "$case_id" in
  B) drive_step=caseB ;;
  C) drive_step=caseC ;;
  *) echo "case must be B or C, not '$case_id'" >&2; exit 2 ;;
esac
export LC_ALL=C.UTF-8 LANG=C.UTF-8

rm -rf "$out"
mkdir -p "$out/config" "$out/cache"
work="$out"

# darktablerc: no password store on the private bus (libsecret would wait
# 25 s), no background thumbnail crawler, no sidecar beside the read-only
# image (its error toast would cover the screenshots)
cat > "$work/config/darktablerc" << RC
plugins/pwstorage/pwstorage_backend=none
backthumbs_mipsize=never
write_sidecar_files=never
RC

command -v dbus-daemon > /dev/null || { echo "status: dbus-daemon missing from the image"; exit 1; }
bus=$(dbus-daemon --session --fork --print-address=1 --print-pid=1)
bus_address=$(echo "$bus" | sed -n 1p)
bus_pid=$(echo "$bus" | sed -n 2p)

cat > "$here/run.env" << ENV
RGBC_PREFIX='$prefix'
RGBC_CONFIGDIR='$work/config'
RGBC_CACHEDIR='$work/cache'
RGBC_OUT='$out'
RGBC_BUS='$bus_address'
ENV
export RGBC_ENV="$here/run.env"

cleanup() {
  "$wbox_python" -I "$here/wbox_life.py" down "$config" > /dev/null 2>&1 || true
  kill "$bus_pid" 2> /dev/null || true
}
trap cleanup EXIT

dtlua() {
  local res
  if ! res=$(DBUS_SESSION_BUS_ADDRESS="$bus_address" gdbus call --session \
               --dest org.darktable.service --object-path /darktable --timeout 60 \
               --method org.darktable.service.Remote.Lua "$1" 2>&1); then
    echo "$res" >&2
    return 1
  fi
  python3 -I -c 'import ast, sys; print(ast.literal_eval(sys.argv[1])[0])' "$res"
}

dt_running() { pgrep -f -- "--configdir $work/config" > /dev/null; }

readback() {
  python3 -I "$here/readback.py" "$work/config/library.db" "$1" "$out/history-$1.json" \
    | tee -a "$out/readback.jsonl"
}

status=ok
"$wbox_python" -I "$here/wbox_life.py" up "$config" > "$out/wbox-up.json"
deadline=$((SECONDS + 600))
until dtlua "return 'up'" > /dev/null 2>&1; do
  dt_running || { status="darktable exited early"; break; }
  [ $SECONDS -lt $deadline ] || { status="start timeout"; break; }
  sleep 0.5
done

# the darkroom on the image, rgb curve enabled, focused and expanded; its
# defaults are RGB linked channels, R tab, the 2-node identity curve
if [ "$status" = ok ] && ! dtlua "
local dt = require 'darktable'
local img = dt.database.import('$image')
dt.gui.selection({img})
dt.gui.current_view(dt.gui.views.darkroom)
return 'ok'" > "$out/lua-setup.txt" 2>&1; then
  status="darkroom setup failed"
fi
[ "$status" = ok ] && sleep 8
if [ "$status" = ok ] && ! dtlua "
local dt = require 'darktable'
-- without a size the action only reads the state
local e = dt.gui.action('iop/rgbcurve', 'enable', 'on', 1.0)
local f = dt.gui.action('iop/rgbcurve', 'focus', 'on', 1.0)
local s = dt.gui.action('iop/rgbcurve', 'show', 'on', 1.0)
dt.gui.views.darkroom.display_image()
return string.format('enable=%s focus=%s show=%s', e, f, s)" >> "$out/lua-setup.txt" 2>&1; then
  status="rgb curve setup failed"
fi
[ "$status" = ok ] && sleep 4
[ "$status" = ok ] && readback before > /dev/null

# the input step: locate the graph, then the case; each prints one JSON line
drive() {
  local step="$1" line
  if ! line=$(timeout 180 "$wbox_python" -I "$here/drive.py" "$step" "$config" "$out" \
                2>> "$out/drive.err"); then
    echo "$line" >> "$out/drive.jsonl"
    status="$step failed: $(echo "$line" | tail -1)"
    return 1
  fi
  echo "$line" >> "$out/drive.jsonl"
}
if [ "$status" = ok ]; then
  drive locate && drive "$drive_step" || true
fi

if [ "$status" = ok ]; then
  dtlua "local dt = require 'darktable'; dt.gui.views.darkroom.display_image(); return 'ok'" \
    > /dev/null 2>&1 || status="flush failed (darktable alive: $(dt_running && echo yes || echo no))"
  readback after > /dev/null || true
fi

# quit the way a user does, then read the library once more
if dt_running; then
  # the global "quit" action, as ctrl+q; the call does not return once
  # darktable quits, so its error is expected
  dtlua "local dt = require 'darktable'; dt.gui.action('global/quit', 1.0); return 'ok'" > /dev/null 2>&1 || true
  for _ in $(seq 120); do dt_running || break; sleep 0.5; done
fi
[ -f "$work/config/library.db" ] && readback final > /dev/null || true

asan_files=$(ls "$out"/asan.* 2> /dev/null || true)
asan_reports=0
move_point=0
if [ -n "$asan_files" ]; then
  asan_reports=$(grep -h -c "ERROR: AddressSanitizer" $asan_files | awk "{ s += \$1 } END { print s + 0 }")
  move_point=$(grep -h -c "_move_point_internal" $asan_files | awk "{ s += \$1 } END { print s + 0 }")
fi
{
  echo "case=$case_id prefix=$prefix commit=$(cat "$prefix/.commit" 2> /dev/null || echo unknown)"
  echo "status=$status"
  echo "asan_reports=$asan_reports asan_frames_in_move_point_internal=$move_point"
  cat "$out/drive.jsonl" 2> /dev/null || true
  cat "$out/readback.jsonl" 2> /dev/null || true
} > "$out/summary.txt"
cat "$out/summary.txt"
[ "$status" = ok ]
