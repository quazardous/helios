#!/usr/bin/env python3
"""Read the rgb curve history out of a darktable library.db.

    readback.py LIBRARY_DB LABEL OUT_JSON

Writes OUT_JSON with every history row of every image, and, for the rgb
curve rows, the decoded params (dt_iop_rgbcurve_params_t, version 1,
516 bytes): per channel the node count and nodes, the curve types, the
mode (0 = RGB linked channels, 1 = independent), compensate middle gray
and preserve colors. Prints a one-line summary.

Run it after darktable flushed its history (dt.gui.views.darkroom.
display_image() with no argument calls dt_dev_write_history) or after it
quit. Opened read-only, with a busy timeout, so it can read a running
darktable's library.
"""

import hashlib
import json
import sqlite3
import struct
import sys

MAXNODES = 20
CHANNELS = 3
PARAMS_SIZE = CHANNELS * MAXNODES * 8 + 3 * 4 + 3 * 4 + 4 + 4 + 4  # 516


def decode_rgbcurve(blob):
    if blob is None or len(blob) != PARAMS_SIZE:
        return {"error": "unexpected size %s" % (None if blob is None else len(blob))}
    floats = struct.unpack_from("<%df" % (CHANNELS * MAXNODES * 2), blob, 0)
    off = CHANNELS * MAXNODES * 8
    num = struct.unpack_from("<3i", blob, off)
    off += 12
    types = struct.unpack_from("<3i", blob, off)
    off += 12
    autoscale, compensate, preserve = struct.unpack_from("<3i", blob, off)
    channels = {}
    for c, name in enumerate("RGB"):
        n = max(1, min(num[c], MAXNODES))
        base = c * MAXNODES * 2
        channels[name] = {
            "num_nodes": num[c],
            "nodes": [[round(floats[base + 2 * k], 6), round(floats[base + 2 * k + 1], 6)]
                      for k in range(n)],
            # unused slots: a stray write lands here for G/B (curve[-1] is the
            # previous channel's last slot), so report them when not zero
            "nonzero_unused": [[k, floats[base + 2 * k], floats[base + 2 * k + 1]]
                               for k in range(n, MAXNODES)
                               if floats[base + 2 * k] or floats[base + 2 * k + 1]],
        }
    return {
        "channels": channels,
        "curve_type": list(types),
        "mode": {0: "linked", 1: "independent"}.get(autoscale, autoscale),
        "compensate_middle_grey": compensate,
        "preserve_colors": preserve,
    }


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    db, label, out = sys.argv[1:]
    con = sqlite3.connect("file:%s?mode=ro" % db, uri=True, timeout=30)
    con.row_factory = sqlite3.Row
    images = {r["id"]: {"filename": r["filename"], "history_end": r["history_end"]}
              for r in con.execute("SELECT id, filename, history_end FROM main.images")}
    rows = []
    for r in con.execute("SELECT imgid, num, operation, enabled, op_params, multi_priority "
                         "FROM main.history ORDER BY imgid, num"):
        row = {
            "imgid": r["imgid"], "num": r["num"], "operation": r["operation"],
            "enabled": r["enabled"], "multi_priority": r["multi_priority"],
            "op_params_sha1": hashlib.sha1(r["op_params"] or b"").hexdigest(),
        }
        if r["operation"] == "rgbcurve":
            row["rgbcurve"] = decode_rgbcurve(r["op_params"])
        rows.append(row)
    con.close()

    result = {"label": label, "images": images, "history": rows}
    with open(out, "w") as f:
        json.dump(result, f, indent=1)

    curves = [r for r in rows if r["operation"] == "rgbcurve"]
    last = curves[-1]["rgbcurve"] if curves else None
    summary = {
        "label": label,
        "history_rows": len(rows),
        "rgbcurve_rows": len(curves),
        "history_end": {i: v["history_end"] for i, v in images.items()},
        "last_rgbcurve_R": last["channels"]["R"]["nodes"] if last and "channels" in last else None,
        "last_rgbcurve_sha1": curves[-1]["op_params_sha1"] if curves else None,
    }
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
