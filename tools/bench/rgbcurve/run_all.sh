#!/usr/bin/env bash
# Both builds, both cases, one summary.
#
#   run_all.sh <buggy prefix> <fixed prefix> <image> <outdir>
#
# Writes <outdir>/<build>-<case>/ (summary.txt, history-*.json, asan.*,
# darktable.log, screenshots) and <outdir>/SUMMARY.txt.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
buggy="$1" fixed="$2" image="$3" out="$(realpath -m "$4")"
mkdir -p "$out"
: > "$out/SUMMARY.txt"
for build in buggy:"$buggy" fixed:"$fixed"; do
  for c in B C; do
    dir="$out/${build%%:*}-$c"
    echo "== ${build%%:*} case $c" >> "$out/SUMMARY.txt"
    "$here/run_case.sh" "${build#*:}" "$c" "$image" "$dir" > "$dir.stdout" 2>&1
    cat "$dir/summary.txt" >> "$out/SUMMARY.txt" 2> /dev/null \
      || { echo "no summary, see $dir.stdout"; tail -20 "$dir.stdout"; } >> "$out/SUMMARY.txt"
    echo >> "$out/SUMMARY.txt"
  done
done
cat "$out/SUMMARY.txt"
