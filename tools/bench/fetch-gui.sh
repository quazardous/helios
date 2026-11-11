#!/usr/bin/env bash
# Fetches the latest helios-gui the bench box built (job `build-gui`) into
# target/box/helios-gui, where `packaging/linux/install.sh --dev --box`
# points the "Helios Dev" launcher: a current desktop with no local build.
# The programs of the plugins Helios ships come beside it
# (target/box/helios-darktable-library), where Helios looks for them.
#
#   tools/bench/fetch-gui.sh [branch]        (main by default)
#
# The box is HELIOS_BENCH_HOST (`user@host`, or `host` for bench-helios)
# and the key HELIOS_BENCH_KEY (~/.ssh/bench-helios by default). The
# "Helios Dev" launcher, when installed, is renamed with the version.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
host="${HELIOS_BENCH_HOST:?set HELIOS_BENCH_HOST to the bench box}"
case "$host" in *@*) ;; *) host="bench-helios@$host" ;; esac
key="${HELIOS_BENCH_KEY:-$HOME/.ssh/bench-helios}"
dest="$root/target/box/helios-gui"
mkdir -p "$(dirname "$dest")"
# each output comes as a tar holding the one file
tmp="$(mktemp -d "$(dirname "$dest")/.fetch.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
fetch() { # <branch> <output> <file>
  ssh -o IdentitiesOnly=yes -i "$key" "$host" fetch latest build-gui "branch=${1:-main}" "$2" |
    tar -x -C "$tmp" "$3"
}
fetch "${1:-main}" gui helios-gui
fetch "${1:-main}" darktable-library helios-darktable-library
mv -f "$tmp/helios-darktable-library" "$(dirname "$dest")/helios-darktable-library"
mv -f "$tmp/helios-gui" "$dest"
echo "$dest"

# The "Helios Dev" launcher (packaging/linux/install.sh --dev --box) shows
# the version it starts, as the program computes it (crates/helios-gui/
# build.rs): the release on a tag, else the next patch's dev build counting
# the commits since the last tag. Read from the fetched binary itself (the
# commit the box built, which may be behind the branch's tip), never by
# running it; from git when the binary has no dev version (a release).
branch="${1:-main}"
launcher="${XDG_DATA_HOME:-$HOME/.local/share}/applications/io.github.quazardous.Helios.Devel.desktop"
if [ -f "$launcher" ]; then
  version="$(grep -aoE '[0-9]+\.[0-9]+\.[0-9]+-dev\.[0-9]+\+[0-9a-f]{7,}' "$dest" | head -1 | sed 's/+.*//')"
  if [ -z "$version" ]; then
    git -C "$root" fetch -q --tags origin "$branch" 2>/dev/null || true
    version="$(git -C "$root" describe --tags --long --match 'v[0-9]*' "origin/$branch" 2>/dev/null |
      sed -E 's/^v([0-9]+)\.([0-9]+)\.([0-9]+)-([0-9]+)-g([0-9a-f]+)$/\1 \2 \3 \4 \5/' |
      awk 'NF == 5 { if ($4 == 0) print $1"."$2"."$3; else print $1"."$2"."($3+1)"-dev."$4 }')"
  fi
  if [ -n "$version" ]; then
    label="Helios Dev $version"
    [ "$branch" = main ] || label="$label ($branch)"
    sed -i "s|^Name=.*|Name=$label|" "$launcher"
    command -v update-desktop-database >/dev/null &&
      update-desktop-database -q "$(dirname "$launcher")" || true
    echo "launcher: $label"
  fi
fi
