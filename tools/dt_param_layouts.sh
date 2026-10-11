#!/usr/bin/env bash
# Builds tools/dt_param_layouts.c against a darktable source and build tree
# and prints the parameter layouts of its modules (JSON). PLUGINS reads
# another darktable's modules instead, such as an installed release (its
# libraries' directory goes on the library path), as long as its
# introspection API matches the source's.
#
#   tools/dt_param_layouts.sh DARKTABLE_SRC DARKTABLE_BUILD [PLUGINS] > layouts.json
#   tools/dt_param_layouts.sh ../darktable ../darktable/build \
#     /usr/lib64/darktable/plugins > crates/helios-parity/layouts/darktable-5.6.json
set -euo pipefail
src="$(cd "$1" && pwd)"
build="$(cd "$2" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
bin="$(mktemp -d)/dt_param_layouts"
# pkg-config's flags are split into words on purpose.
# shellcheck disable=SC2046
cc -O1 -o "$bin" "$here/dt_param_layouts.c" -I"$src/src" -I"$build/src" \
  $(pkg-config --cflags glib-2.0) -ldl
if [ -n "${3:-}" ]; then
  plugins="$(cd "$3" && pwd)"
  libs="$(dirname "$plugins")"
else
  plugins="$(dirname "$(find "$build" -name libexposure.so -path '*plugins*' | head -1)")"
  libs="$build/bin"
fi
LD_LIBRARY_PATH="$libs:${LD_LIBRARY_PATH:-}" "$bin" "$plugins"
