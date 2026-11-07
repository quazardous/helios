#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""An importer plugin for Helios's tests: it lists the JPEGs of a folder
with the ratings, labels, tags and titles its ratings.json gives, as
`importer.scan` answers. Helios imports the files itself.

A folder holding a file named `hang` makes the scan wait, after a first
progress notice, until Helios cancels it (`$/cancelRequest`)."""

import json
import os
import sys

CANCELLED = -32800
NOT_A_SOURCE = -32001


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def progress(token, done, total, message):
    send(
        {
            "jsonrpc": "2.0",
            "method": "$/progress",
            "params": {"token": token, "done": done, "total": total, "message": message},
        }
    )


def scan(params):
    source = params["source"]
    token = params.get("progress")
    try:
        with open(os.path.join(source, "ratings.json"), encoding="utf-8") as f:
            ratings = json.load(f)
    except (OSError, ValueError) as e:
        raise LookupError(f"ratings.json: {e}") from e
    names = sorted(n for n in os.listdir(source) if n.lower().endswith((".jpg", ".jpeg")))
    images = []
    for done, name in enumerate(names):
        progress(token, done, len(names), name)
        image = {"path": name}
        image.update(ratings.get(name, {}))
        images.append(image)
    progress(token, len(names), len(names), "")
    warnings = [
        f"ratings.json names {n}, which the folder does not hold"
        for n in sorted(ratings)
        if n not in names
    ]
    return {"images": images, "warnings": warnings}


hanging = None
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    params = message.get("params") or {}
    if "id" not in message:
        if method == "exit":
            break
        if method == "$/cancelRequest" and params.get("id") == hanging:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": hanging,
                    "error": {"code": CANCELLED, "message": "cancelled"},
                }
            )
            hanging = None
        continue
    reply = {"jsonrpc": "2.0", "id": message["id"]}
    if method == "initialize":
        reply["result"] = {"name": "ratings-folder"}
    elif method == "shutdown":
        reply["result"] = None
    elif method == "importer.scan":
        if os.path.exists(os.path.join(params["source"], "hang")):
            hanging = message["id"]
            progress(params.get("progress"), 0, 1, "waiting")
            continue
        try:
            reply["result"] = scan(params)
        except LookupError as e:
            reply["error"] = {"code": NOT_A_SOURCE, "message": str(e)}
    else:
        reply["error"] = {"code": -32601, "message": f"no method {method}"}
    send(reply)
