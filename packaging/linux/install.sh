#!/usr/bin/env bash
# Installs Helios's launcher and icons for the current user, where GNOME and
# other freedesktop desktops look for them; `--uninstall` removes them.
#
#   packaging/linux/install.sh [--uninstall]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
id=io.github.quazardous.Helios
data="${XDG_DATA_HOME:-$HOME/.local/share}"
icons="$data/icons/hicolor"
sizes="16 24 32 48 64 128 256 512"

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$data/applications/$id.desktop" "$icons/scalable/apps/$id.svg"
  for n in $sizes; do rm -f "$icons/${n}x${n}/apps/$id.png"; done
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
