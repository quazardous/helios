#!/bin/sh
# Run at the end of every bench box job that mounts a branch's target
# (tools/bench/Benchfile): keeps the target within its budget so that
# several branches fit the project's disk quota (docs/bench.md, "The
# target's budget").
#
# A full test build links each test target into its own executable, and
# each one carries the whole GUI and GPU stack: they are most of a
# target, and the cheapest part to make again (a link, plus the test
# file's own compile). So the oldest executables of the debug profile go
# first, until the target is back under the budget; the compiled crates,
# the build scripts and the release builds stay.
#
# Usage: trim-target.sh [target dir] [budget in GB]
t="${1:-/target}"
budget_gb="${2:-${HELIOS_TARGET_BUDGET_GB:-12}}"
[ -d "$t/debug/deps" ] || exit 0
used=$(du -sb "$t" | cut -f1)
budget=$((budget_gb * 1000000000))
before=$used
freed=0
if [ "$used" -gt "$budget" ]; then
  # oldest first; an executable cargo also hard-linked out of deps/ frees
  # nothing until both links go, so only single links count
  find "$t/debug/deps" "$t/debug/examples" -maxdepth 1 -type f -perm -u+x \
    ! -name '*.so' ! -name '*.d' -links 1 -printf '%T@ %s %p\n' 2> /dev/null \
    | sort -n > /tmp/trim-target.list
  while read -r _ size path; do
    [ "$used" -le "$budget" ] && break
    rm -f "$path" && used=$((used - size)) && freed=$((freed + size))
  done < /tmp/trim-target.list
  rm -f /tmp/trim-target.list
fi
echo "target: $(awk "BEGIN { printf \"%.1f GB\", $before / 1e9 }") at the job's end, $(awk "BEGIN { printf \"%.1f GB\", $freed / 1e9 }") of old test executables removed, budget ${budget_gb} GB" >&2
exit 0
