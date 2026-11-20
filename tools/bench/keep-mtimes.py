#!/usr/bin/env python3
"""Give the unchanged files of a bench box job's source their old times.

Run in the source tree a job builds (tools/bench/Benchfile), after it is
copied and before cargo runs. The box gives each job a fresh clone, so
every file is newer than the branch's kept target, and cargo, which
judges a path crate by its files' times, rebuilds every workspace crate
on every job.

The manifest (kept in the branch's target, which one job uses at a time)
holds each file's content hash and time from the previous job. A file
with the same content gets its old time back; a changed or new file keeps
the copy's time, newer than any earlier build, so cargo rebuilds what
changed and only that. Times only move forward for changed content, so
this is the same as building in one tree kept between jobs.

Never fails the job: on any error the tree keeps the copy's times (a full
rebuild, as before).

Usage: keep-mtimes.py <manifest> [tree]
"""

import hashlib
import json
import os
import sys


def files(tree):
    for top, dirs, names in os.walk(tree):
        dirs.sort()
        for name in sorted(names):
            path = os.path.join(top, name)
            if os.path.isfile(path) and not os.path.islink(path):
                yield os.path.relpath(path, tree), path


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def keep(manifest, tree):
    try:
        with open(manifest) as f:
            before = json.load(f)
    except (OSError, ValueError):
        before = {}
    after = {}
    kept = changed = 0
    for rel, path in files(tree):
        sha = digest(path)
        old = before.get(rel)
        if isinstance(old, list) and len(old) == 2 and old[0] == sha:
            os.utime(path, ns=(old[1], old[1]))
            kept += 1
        else:
            changed += 1
        after[rel] = [sha, os.stat(path).st_mtime_ns]
    tmp = manifest + ".tmp"
    with open(tmp, "w") as f:
        json.dump(after, f)
    os.replace(tmp, manifest)
    return kept, changed


def main(argv):
    if len(argv) < 2:
        print("usage: keep-mtimes.py <manifest> [tree]", file=sys.stderr)
        return 0
    manifest = argv[1]
    tree = argv[2] if len(argv) > 2 else "."
    try:
        kept, changed = keep(manifest, tree)
        print(
            f"keep-mtimes: {kept} files kept their time, {changed} new or changed",
            file=sys.stderr,
        )
    except Exception as e:  # noqa: BLE001 - never fail the job over it
        print(f"keep-mtimes: skipped ({e}); every file counts as changed", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
