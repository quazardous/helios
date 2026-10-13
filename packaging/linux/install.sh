#!/usr/bin/env bash
# Installs Helios's launcher and icons for the current user, where GNOME and
# other freedesktop desktops look for them; `--uninstall` removes them.
# `--dev` installs "Helios Dev" instead, launching this checkout's debug
# build (`cargo build -p helios-gui`), beside a released Helios; with
# `--box`, the bench box's latest build instead (target/box/helios-gui,
# fetched by tools/bench/fetch-gui.sh).
#
#   packaging/linux/install.sh [--dev [--box]] [--uninstall]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
id=io.github.quazardous.Helios
dev=""
built=target/debug/helios-gui
if [ "${1:-}" = "--dev" ]; then
  dev=1
  shift
  if [ "${1:-}" = "--box" ]; then
    built=target/box/helios-gui
    shift
  fi
fi
data="${XDG_DATA_HOME:-$HOME/.local/share}"
icons="$data/icons/hicolor"
sizes="16 24 32 48 64 128 256 512"

if [ -n "$dev" ]; then
  launcher="$data/applications/$id.Devel.desktop"
  if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$launcher"
  else
    bin="$(cd "$here/../.." && pwd)/$built"
    [ -x "$bin" ] || echo "note: $bin is not there yet (cargo build -p helios-gui, or tools/bench/fetch-gui.sh)" >&2
    mkdir -p "$(dirname "$launcher")"
    sed -e "s|^Name=Helios$|Name=Helios Dev|" \
        -e "s|^Exec=helios-gui|Exec=\"$bin\"|" \
        -e "s|^Comment=.*|Comment=Helios built from a source checkout|" \
        -e "/^MimeType=/d" \
        "$here/$id.desktop" >"$launcher"
  fi
fi

if [ -n "$dev" ] && [ "${1:-}" = "--uninstall" ]; then
  : # the icons stay for the released launcher
elif [ "${1:-}" = "--uninstall" ]; then
  rm -f "$data/applications/$id.desktop" "$icons/scalable/apps/$id.svg"
  for n in $sizes; do rm -f "$icons/${n}x${n}/apps/$id.png"; done
elif [ -n "$dev" ]; then
  install -Dm644 "$here/icons/$id.svg" "$icons/scalable/apps/$id.svg"
  for n in $sizes; do
    install -Dm644 "$here/icons/$id-$n.png" "$icons/${n}x${n}/apps/$id.png"
  done
else
  install -Dm644 "$here/$id.desktop" "$data/applications/$id.desktop"
  install -Dm644 "$here/icons/$id.svg" "$icons/scalable/apps/$id.svg"
  for n in $sizes; do
    install -Dm644 "$here/icons/$id-$n.png" "$icons/${n}x${n}/apps/$id.png"
  done
fi
# Refresh the caches when the tools are there.
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$icons" || true
command -v update-desktop-database >/dev/null && update-desktop-database -q "$data/applications" || true
