#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""A Helios plugin serving `metadata.read` with the exiftool program.

It speaks JSON-RPC 2.0 over standard input and output, one message per
line, and uses the Python standard library alone. Helios hands it one
image path per call; it answers the fields it found, each with the tag it
came from.
"""

import json
import subprocess
import sys

# Each field, and the tags it is read from, first found wins. Tags are
# exiftool's `Group:Name`, as `-G` prints them.
FIELDS = [
    ("camera_make", ["EXIF:Make", "XMP:Make", "MakerNotes:Make"]),
    ("camera_model", ["EXIF:Model", "XMP:Model"]),
    ("lens", ["EXIF:LensModel", "XMP:LensModel", "Composite:LensID", "XMP:Lens"]),
    ("exposure_time", ["EXIF:ExposureTime", "XMP:ExposureTime"]),
    ("aperture", ["EXIF:FNumber", "XMP:FNumber", "Composite:Aperture"]),
    ("iso", ["EXIF:ISO", "XMP:ISO"]),
    ("focal_length", ["EXIF:FocalLength", "XMP:FocalLength"]),
    ("date_taken", ["EXIF:DateTimeOriginal", "XMP:DateTimeOriginal", "XMP:CreateDate"]),
    ("gps_latitude", ["Composite:GPSLatitude", "EXIF:GPSLatitude", "XMP:GPSLatitude"]),
    ("gps_longitude", ["Composite:GPSLongitude", "EXIF:GPSLongitude", "XMP:GPSLongitude"]),
    ("gps_altitude", ["Composite:GPSAltitude", "EXIF:GPSAltitude"]),
    ("keywords", ["IPTC:Keywords", "XMP:Subject"]),
    ("rating", ["XMP:Rating", "EXIF:Rating"]),
    ("width", ["File:ImageWidth", "PNG:ImageWidth", "EXIF:ExifImageWidth"]),
    ("height", ["File:ImageHeight", "PNG:ImageHeight", "EXIF:ExifImageHeight"]),
]

PLUGIN_ID = "io.github.quazardous.helios.exif-exiftool"
state = {"exiftool": "exiftool", "granted": []}


class Failure(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def exiftool(*args):
    try:
        out = subprocess.run(
            [state["exiftool"], *args],
            capture_output=True,
            check=False,
            text=True,
        )
    except FileNotFoundError:
        raise Failure(-32001, "exiftool is not installed (%s)" % state["exiftool"])
    if out.returncode != 0 and not out.stdout:
        raise Failure(-32002, "exiftool failed: %s" % out.stderr.strip())
    return out.stdout


def read_metadata(params):
    if "images.read" not in state["granted"]:
        raise Failure(-32003, "images.read is not granted")
    image = params.get("image")
    if not isinstance(image, str):
        raise Failure(-32602, "`image` is missing")
    tags = json.loads(exiftool("-json", "-G", "-n", "--", image))[0]
    fields = []
    for name, sources in FIELDS:
        for source in sources:
            if source in tags:
                value = tags[source]
                if name == "keywords" and not isinstance(value, list):
                    value = [value]
                fields.append({"name": name, "value": value, "source": source})
                break
    return {"fields": fields}


def execute(params):
    if params.get("command") == "exiftool.version":
        return {"version": exiftool("-ver").strip()}
    raise Failure(-32601, "no command %s" % params.get("command"))


def initialize(params):
    state["granted"] = params.get("capabilities", {}).get("granted", [])
    settings = params.get("settings", {})
    state["exiftool"] = settings.get(PLUGIN_ID + ".exiftool", "exiftool")
    return {"name": "exif-exiftool", "version": "0.1.0"}


METHODS = {
    "initialize": initialize,
    "shutdown": lambda params: None,
    "metadata.read": read_metadata,
    "command/execute": execute,
}


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        message = json.loads(line)
        method = message.get("method")
        if "id" not in message:
            if method == "exit":
                return
            continue  # initialized, $/cancelRequest: nothing to do
        handler = METHODS.get(method)
        try:
            if handler is None:
                raise Failure(-32601, "no method %s" % method)
            result = handler(message.get("params") or {})
            send({"jsonrpc": "2.0", "id": message["id"], "result": result})
        except Failure as f:
            send({"jsonrpc": "2.0", "id": message["id"],
                  "error": {"code": f.code, "message": f.message}})


if __name__ == "__main__":
    main()
