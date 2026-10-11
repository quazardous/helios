#!/usr/bin/env bash
# Builds tools/dt_param_layouts.c against a darktable source and build tree
# and prints the parameter layouts of its modules (JSON).
#
#   tools/dt_param_layouts.sh DARKTABLE_SRC DARKTABLE_BUILD > crates/helios-xmp/layouts.json
set -euo pipefail
src="$(cd "$1" && pwd)"
build="$(cd "$2" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
bin="$(mktemp -d)/dt_param_layouts"
cc -O1 -o "$bin" "$here/dt_param_layouts.c" -I"$src/src" -I"$build/src" \
  $(pkg-config --cflags glib-2.0) -ldl
plugins="$(dirname "$(find "$build" -name libexposure.so -path '*plugins*' | head -1)")"
LD_LIBRARY_PATH="$build/bin:${LD_LIBRARY_PATH:-}" "$bin" "$plugins"
