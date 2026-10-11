#!/usr/bin/env bash
# Builds tools/dt_legacy_oracle.c against a darktable source and build tree
# and runs it on stdin (lines "MODULE VERSION HEX").
#
#   tools/dt_legacy_oracle.sh DARKTABLE_SRC DARKTABLE_BUILD < pairs
set -euo pipefail
src="$(cd "$1" && pwd)"
build="$(cd "$2" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
bin="$(mktemp -d)/dt_legacy_oracle"
# pkg-config's flags are split into words on purpose.
# shellcheck disable=SC2046
cc -O1 -o "$bin" "$here/dt_legacy_oracle.c" -I"$src/src" -I"$build/src" \
  $(pkg-config --cflags glib-2.0) -ldl
plugins="$(dirname "$(find "$build" -name libexposure.so -path '*plugins*' | head -1)")"
LD_LIBRARY_PATH="$build/bin:${LD_LIBRARY_PATH:-}" "$bin" "$plugins"
