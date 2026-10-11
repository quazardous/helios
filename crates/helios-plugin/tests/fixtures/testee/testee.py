#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""A plugin for the broker's tests: it answers, crashes or hangs on demand."""

import json
import os
import sys


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


granted = []
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    params = message.get("params") or {}
    if "id" not in message:
        if method == "exit":
            break
        continue
    reply = {"jsonrpc": "2.0", "id": message["id"]}
    if method == "initialize":
        granted = params["capabilities"]["granted"]
        reply["result"] = {"name": "testee"}
    elif method == "shutdown":
        reply["result"] = None
    elif method == "metadata.read":
        reply["result"] = {
            "fields": [
                {"name": "path", "value": params["image"], "source": "testee"},
                {"name": "granted", "value": granted, "source": "testee"},
            ]
        }
    elif method == "echo":
        reply["result"] = {"params": params, "pid": os.getpid()}
    elif method == "crash":
        sys.stderr.write("testee: crashing on demand\n")
        sys.stderr.flush()
        os._exit(3)
    elif method == "hang":
        continue  # never answers
    else:
        reply["error"] = {"code": -32601, "message": f"no method {method}"}
    send(reply)
