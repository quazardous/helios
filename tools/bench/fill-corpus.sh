#!/usr/bin/env bash
# Fills a directory with the raw corpus: the "core" files of
# tests/raw-corpus.toml, downloaded from raw.pixls.us and checked against
# their sha256. Idempotent: a file already there with the right sha256 is
# kept, a wrong or partial one is fetched again, and a file the manifest no
# longer lists is removed.
#
#   tools/bench/fill-corpus.sh <dir>
#
# The bench box runs it as the provision step of the raw-corpus volume
# (`provision <branch> raw-corpus`, with network); it works the same on a
# workstation, for the local mode of the checks (HELIOS_RAW_CORPUS=<dir>).
#
# Licences: the manifest admits CC0 files only, and the script reads each
# file's licence again from raw.pixls.us's repository table before fetching
# it, since a licence on the site can change: a file that is no longer CC0
# is skipped, and the run fails.
set -euo pipefail

dir=${1:?usage: fill-corpus.sh <dir>}
here=$(cd "$(dirname "$0")" && pwd)
manifest=${HELIOS_RAW_CORPUS_MANIFEST:-$here/../../tests/raw-corpus.toml}
# the volume stays under this size (bytes): 3 GB
limit=${HELIOS_RAW_CORPUS_LIMIT:-3000000000}

mkdir -p "$dir"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -sSfL --retry 3 -o "$tmp/repo.json" \
  'https://raw.pixls.us/json/getrepository.php?set=all'

# one line per core file: name, sha256, url, licence on the site
python3 -I - "$manifest" "$tmp/repo.json" > "$tmp/list" <<'EOF'
import json, re, sys, tomllib

manifest = tomllib.load(open(sys.argv[1], "rb"))
rows = json.load(open(sys.argv[2]))["data"]
licence = {}
for row in rows:
    cells = " ".join(str(c) for c in row)
    m = re.search(r"getfile\.php\\?/(\d+)/", cells)
    if m:
        cc0 = "publicdomain/zero/1.0" in cells.replace("\\/", "/")
        licence[int(m.group(1))] = "CC0-1.0" if cc0 else "other"
for f in manifest["file"]:
    if f["tier"] != "core":
        continue
    ext = f["filename"].rsplit(".", 1)[1]
    site = licence.get(f["pixls_id"], "missing")
    print(f"{f['id']}.{ext}\t{f['sha256']}\t{f['url']}\t{f['licence']}\t{site}")
EOF

fail=0
: > "$tmp/keep"
while IFS=$'\t' read -r name sha url declared site; do
  echo "$name" >> "$tmp/keep"
  if [ "$declared" != CC0-1.0 ] || [ "$site" != CC0-1.0 ]; then
    echo "SKIP $name: licence $declared in the manifest, $site on raw.pixls.us (CC0 only)" >&2
    rm -f "$dir/$name"
    fail=1
    continue
  fi
  if [ -f "$dir/$name" ] && echo "$sha  $dir/$name" | sha256sum -c --status -; then
    echo "ok    $name"
    continue
  fi
  curl -sSfL --retry 3 -o "$dir/.$name.part" "$url"
  if echo "$sha  $dir/.$name.part" | sha256sum -c --status -; then
    mv "$dir/.$name.part" "$dir/$name"
    echo "fetch $name"
  else
    rm -f "$dir/.$name.part"
    echo "FAIL  $name: sha256 differs from the manifest" >&2
    fail=1
  fi
done < "$tmp/list"

# files the manifest no longer lists (and leftovers of an interrupted run)
for f in "$dir"/* "$dir"/.*.part; do
  [ -e "$f" ] || continue
  base=$(basename "$f")
  if ! grep -qxF "$base" "$tmp/keep"; then
    rm -f "$f"
    echo "drop  $base"
  fi
done

size=$(du -sb "$dir" | cut -f1)
count=$(wc -l < "$tmp/keep")
echo "corpus: $count files, $size bytes in $dir"
if [ "$size" -gt "$limit" ]; then
  echo "FAIL  the corpus is over its $limit-byte budget" >&2
  fail=1
fi
exit $fail
