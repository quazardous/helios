#!/usr/bin/env bash
# Fetches the latest helios-gui the bench box built (job `build-gui`) into
# target/box/helios-gui, where `packaging/linux/install.sh --dev --box`
# points the "Helios Dev" launcher: a current desktop with no local build.
#
#   tools/bench/fetch-gui.sh [branch]        (main by default)
#
# The box is HELIOS_BENCH_HOST (`user@host`, or `host` for bench-helios)
# and the key HELIOS_BENCH_KEY (~/.ssh/bench-helios by default).
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
host="${HELIOS_BENCH_HOST:?set HELIOS_BENCH_HOST to the bench box}"
case "$host" in *@*) ;; *) host="bench-helios@$host" ;; esac
key="${HELIOS_BENCH_KEY:-$HOME/.ssh/bench-helios}"
dest="$root/target/box/helios-gui"
mkdir -p "$(dirname "$dest")"
# the output comes as a tar holding the one file
tmp="$(mktemp -d "$(dirname "$dest")/.fetch.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
ssh -o IdentitiesOnly=yes -i "$key" "$host" fetch latest build-gui "branch=${1:-main}" gui |
  tar -x -C "$tmp" helios-gui
mv -f "$tmp/helios-gui" "$dest"
echo "$dest"
