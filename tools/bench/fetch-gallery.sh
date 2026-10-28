#!/usr/bin/env bash
# Fetches the latest egui-pop gallery the bench box built (job
# `egui-pop-gallery`) into target/release/egui-pop-gallery, where the
# gallery's desktop launcher points: a current gallery with no local build.
#
#   tools/bench/fetch-gallery.sh [egui-pop branch]   (main by default)
#
# The box is HELIOS_BENCH_HOST (`user@host`, or `host` for bench-helios)
# and the key HELIOS_BENCH_KEY (~/.ssh/bench-helios by default).
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
host="${HELIOS_BENCH_HOST:?set HELIOS_BENCH_HOST to the bench box}"
case "$host" in *@*) ;; *) host="bench-helios@$host" ;; esac
key="${HELIOS_BENCH_KEY:-$HOME/.ssh/bench-helios}"
dest="$root/target/release/egui-pop-gallery"
mkdir -p "$(dirname "$dest")"
# the output comes as a tar holding the one file
tmp="$(mktemp -d "$(dirname "$dest")/.fetch.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
ssh -o IdentitiesOnly=yes -i "$key" "$host" fetch latest egui-pop-gallery "egui_pop=${1:-main}" gallery |
  tar -x -C "$tmp" egui-pop-gallery
mv -f "$tmp/egui-pop-gallery" "$dest"
echo "$dest"
