#!/usr/bin/env python3
"""Inventory of darktable image operation modules (helios module normalization).

For every module in src/iop/, records what is already declared (introspected
parameters and their annotations, widgets bound to parameters, flags,
colorspaces) and what is still imperative code (hand-built widgets, custom
tiling, geometry callbacks, raw GTK), so that the distance to self-describing
modules can be measured and tracked.

usage: iop_inventory.py [--src DIR] [--json FILE] [--markdown FILE]
"""

import argparse
import json
import os
import re
import sys
from collections import Counter

# entry points of src/iop/iop_api.h, grouped by concern
API = {
    "compute": [
        "process",
        "process_cl",
        "process_tiling",
        "tiling_callback",
        "commit_params",
        "init_pipe",
        "cleanup_pipe",
    ],
    "geometry": [
        "modify_roi_in",
        "modify_roi_out",
        "distort_transform",
        "distort_backtransform",
        "distort_mask",
    ],
    "format": [
        "input_format",
        "output_format",
        "default_colorspace",
        "input_colorspace",
        "output_colorspace",
        "blend_colorspace",
    ],
    "params": ["legacy_params", "reload_defaults", "init_presets", "change_image", "init"],
    "gui": [
        "gui_init",
        "gui_update",
        "gui_changed",
        "gui_reset",
        "gui_cleanup",
        "gui_post_expose",
        "gui_focus",
        "color_picker_apply",
        "mouse_moved",
        "button_pressed",
        "button_released",
        "scrolled",
        "mouse_leave",
        "mouse_actions",
        "masks_selection_changed",
    ],
}
FLAG = re.compile(r"\bIOP_FLAGS_(\w+)")
INTRO = re.compile(r"DT_MODULE_INTROSPECTION\s*\(\s*(\d+)\s*,\s*(\w+)\s*\)")
MODULE = re.compile(r"\bDT_MODULE\s*\(\s*(\d+)\s*\)")
FROM_PARAMS = re.compile(r"\bdt_bauhaus_(\w+?)_from_params\s*\(")
TOGGLE_FROM_PARAMS = re.compile(r"\bdt_bauhaus_toggle_from_params\s*\(")
HAND_WIDGET = re.compile(
    r"\b(dt_bauhaus_slider_new\w*|dt_bauhaus_combobox_new\w*|gtk_\w+_new\w*|"
    r"dtgtk_\w+_new\w*|dt_ui_\w+_new\w*|dt_gui_\w+_new\w*)\s*\("
)
GTK_CALL = re.compile(r"\bgtk_\w+\s*\(")
ANNOT = re.compile(r"\$(MIN|MAX|DEFAULT|DESCRIPTION)\s*:")
SCALE = re.compile(r"roi_in->scale|roi_out->scale|piece->iscale|roi->scale")
RASTER = re.compile(r"dt_dev_get_raster_mask|IOP_FLAGS_WRITE_RASTER|raster_mask")
DETAILS = re.compile(r"IOP_FLAGS_WRITE_DETAILS|details_mask|dt_dev_distort_detail_mask")
AI = re.compile(r'#include\s+"(?:common/)?ai/|dt_ai_')


def defined(text, name):
    return re.search(r"^[\w \*]*\b" + name + r"\s*\(", text, re.M) is not None


def params_struct(text, struct):
    m = re.search(r"typedef struct " + struct + r"\s*\{(.*?)\}\s*" + struct + r"\s*;", text, re.S)
    if not m:
        return None
    fields, annotated = 0, Counter()
    for line in m.group(1).splitlines():
        code = line.split("//")[0]
        if ";" not in code:
            continue
        # one line may declare several fields: count the declarators
        fields += code.count(";")
        comment = line.split("//", 1)[1] if "//" in line else ""
        for a in set(ANNOT.findall(comment)):
            annotated[a] += 1
    return {"fields": fields, "annotated": dict(annotated)}


def scan(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        text = f.read()
    intro = INTRO.search(text)
    mod = MODULE.search(text)
    if not intro and not mod:
        return None
    r = {"module": os.path.splitext(os.path.basename(path))[0], "lines": text.count("\n")}
    r["version"] = int((intro or mod).group(1))
    r["introspection"] = bool(intro)
    r["params"] = params_struct(text, intro.group(2)) if intro else None
    r["api"] = {k: [f for f in v if defined(text, f)] for k, v in API.items()}
    flags_body = re.search(r"^int flags\s*\(\s*\)\s*\{(.*?)^\}", text, re.M | re.S)
    r["flags"] = sorted(set(FLAG.findall(flags_body.group(1)))) if flags_body else []
    gui = re.search(r"^void gui_init\s*\(.*?\)\s*\{(.*?)^\}", text, re.M | re.S)
    gui_text = gui.group(1) if gui else ""
    r["widgets_from_params"] = len(FROM_PARAMS.findall(gui_text)) + len(
        TOGGLE_FROM_PARAMS.findall(gui_text)
    )
    r["widgets_by_hand"] = len(HAND_WIDGET.findall(gui_text))
    r["gtk_calls"] = len(GTK_CALL.findall(text))
    r["scale_aware"] = bool(SCALE.search(text))
    r["raster_mask"] = bool(RASTER.search(text))
    r["details_mask"] = bool(DETAILS.search(text))
    r["ai"] = bool(AI.search(text))
    return r


def summarize(mods):
    n = len(mods)
    live = [m for m in mods if "DEPRECATED" not in m["flags"]]
    c = Counter()
    for m in live:
        c["introspection"] += m["introspection"]
        c["opencl"] += "process_cl" in m["api"]["compute"]
        c["tiling_cb"] += "tiling_callback" in m["api"]["compute"]
        c["geometry"] += bool(m["api"]["geometry"])
        c["gui"] += "gui_init" in m["api"]["gui"]
        c["interactive"] += any(
            f in m["api"]["gui"] for f in ("mouse_moved", "button_pressed", "gui_post_expose")
        )
        c["scale_aware"] += m["scale_aware"]
        c["raster"] += m["raster_mask"]
        c["legacy"] += "legacy_params" in m["api"]["params"]
        c["allow_tiling"] += "ALLOW_TILING" in m["flags"]
        p = m["params"]
        if p and p["fields"]:
            c["fields"] += p["fields"]
            for k in ("MIN", "MAX", "DEFAULT", "DESCRIPTION"):
                c["f_" + k] += p["annotated"].get(k, 0)
        c["w_params"] += m["widgets_from_params"]
        c["w_hand"] += m["widgets_by_hand"]
        c["full_decl_gui"] += (
            "gui_init" in m["api"]["gui"]
            and m["widgets_by_hand"] == 0
            and m["widgets_from_params"] > 0
        )
    return n, len(live), c


def markdown(mods, out):
    n, nl, c = summarize(mods)

    def pct(a, b):
        return f"{a} / {b} ({100 * a // b if b else 0}%)"

    w = out.write
    w("# darktable image operation module inventory (helios)\n\n")
    w(
        "Generated by `tools/iop_inventory.py` from the darktable sources. Do not edit by hand.\n"
        "Detection uses source patterns: approximate, see the JSON for per-module details.\n\n"
    )
    w(
        f"- Modules: **{n}**, {n - nl} of them deprecated; figures below cover the **{nl}** active ones\n"
    )
    w(f"- Introspected parameters: {pct(c['introspection'], nl)}\n")
    w(
        f"- Parameter fields: {c['fields']}; annotated $DEFAULT {pct(c['f_DEFAULT'], c['fields'])}, "
        f"$MIN {pct(c['f_MIN'], c['fields'])}, $MAX {pct(c['f_MAX'], c['fields'])}, "
        f"$DESCRIPTION {pct(c['f_DESCRIPTION'], c['fields'])}\n"
    )
    w(
        f"- Widgets in `gui_init`: {c['w_params']} bound to parameters, {c['w_hand']} widgets or containers (boxes, notebooks) built by hand\n"
    )
    w(
        f"- Fully declarative GUI (no widget or container built by hand): {pct(c['full_decl_gui'], c['gui'])}\n"
    )
    w(f"- On-image interaction (mouse, drawing): {pct(c['interactive'], nl)}\n")
    w(
        f"- OpenCL variant: {pct(c['opencl'], nl)}; tiling allowed: {pct(c['allow_tiling'], nl)}; "
        f"own memory requirements (`tiling_callback`): {pct(c['tiling_cb'], nl)}\n"
    )
    w(f"- Changes geometry (roi, distortion): {pct(c['geometry'], nl)}\n")
    w(f"- Scale aware (`roi->scale`, `iscale`): {pct(c['scale_aware'], nl)}\n")
    w(f"- Reads or writes a raster mask (implicit link between modules): {pct(c['raster'], nl)}\n")
    w(f"- Converts old parameters (`legacy_params`): {pct(c['legacy'], nl)}\n\n")
    w("## By module\n\n")
    w(
        "| module | v | fields | annotated DEF/MIN/MAX/DESC | widgets param/hand | gtk | OpenCL | tiling cb | geometry | scale | raster | interactive | lines |\n"
    )
    w("|---|---|---|---|---|---|---|---|---|---|---|---|---|\n")
    for m in sorted(mods, key=lambda m: m["module"]):
        p = m["params"] or {"fields": 0, "annotated": {}}
        a = p["annotated"]

        def yes(b):
            return "x" if b else ""

        name = m["module"] + (" (deprecated)" if "DEPRECATED" in m["flags"] else "")
        inter = any(
            f in m["api"]["gui"] for f in ("mouse_moved", "button_pressed", "gui_post_expose")
        )
        w(
            f"| {name} | {m['version']} | {p['fields']} | "
            f"{a.get('DEFAULT', 0)}/{a.get('MIN', 0)}/{a.get('MAX', 0)}/{a.get('DESCRIPTION', 0)} | "
            f"{m['widgets_from_params']}/{m['widgets_by_hand']} | {m['gtk_calls']} | "
            f"{yes('process_cl' in m['api']['compute'])} | {yes('tiling_callback' in m['api']['compute'])} | "
            f"{yes(m['api']['geometry'])} | {yes(m['scale_aware'])} | {yes(m['raster_mask'])} | {yes(inter)} | "
            f"{m['lines']} |\n"
        )


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--src",
        default=os.path.join(
            os.environ.get("DARKTABLE_SRC", os.path.join(here, "..", "..", "darktable")), "src"
        ),
    )
    ap.add_argument("--json")
    ap.add_argument("--markdown")
    args = ap.parse_args()
    iop = os.path.join(args.src, "iop")
    mods = [
        m
        for m in (scan(os.path.join(iop, f)) for f in sorted(os.listdir(iop)) if f.endswith(".c"))
        if m
    ]
    if args.json:
        with open(args.json, "w") as f:
            json.dump({"modules": mods}, f, indent=1, sort_keys=True)
    if args.markdown:
        with open(args.markdown, "w") as f:
            markdown(mods, f)
    if not args.json and not args.markdown:
        markdown(mods, sys.stdout)


if __name__ == "__main__":
    main()
