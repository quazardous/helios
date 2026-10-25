#!/usr/bin/env python3
"""Pointer, wheel and keyboard input for the rgbcurve scroll scenario.

Runs under wbox's own interpreter (the box's $WBOX_PYTHON), like the fork's
tools/helios/wbox/wbox_ctl.py, and drives the same wbox library (pinned
wbox-mcp a2f3eab: scroll, virtual pointer and virtual keyboard).

    drive.py up      CONFIG
    drive.py down    CONFIG
    drive.py running CONFIG
    drive.py shot    CONFIG NAME
    drive.py locate  CONFIG OUTDIR          finds the rgb curve graph
    drive.py caseB   CONFIG OUTDIR          wheel away from every node
    drive.py caseC   CONFIG OUTDIR          wheel on a node with `a` held

`locate` writes OUTDIR/graph.json ({"x0","y0","x1","y1"}, screen pixels of
the curve's filled area); the cases read it. RGBC_GRAPH="x0,y0,x1,y1"
overrides the detection. Every step prints one JSON line.
"""

import json
import os
import struct
import sys
import time
import zlib
from pathlib import Path

from wbox.config import apply_overrides, load_config, resolve_dir
from wbox.pointer import PRESSED, RELEASED, _parse_shortcut
from wbox.server import _build_app_cmd, _build_app_env, build_compositor


def say(**kw):
    print(json.dumps(kw, default=str), flush=True)


def compositor(config_path):
    config_path = Path(config_path).resolve()
    cfg = load_config(config_path)
    cfg["headless"] = os.environ.get("WBOX_VISIBLE", "").lower() not in ("1", "true", "yes", "on")
    apply_overrides(cfg, [])
    cfg["_config_dir"] = str(config_path.parent)
    os.chdir(config_path.parent)
    comp = build_compositor(cfg)
    comp.state.screenshot_dir = resolve_dir(cfg, "screenshot_dir", "./screenshots")
    return cfg, comp


def warm_up(comp):
    # a fresh virtual device swallows its first event (see wbox_ctl.py)
    comp._vptr_op("warm_up")
    time.sleep(0.3)


def shot(comp, name):
    r = comp.screenshot(name)
    if "error" in r:
        raise SystemExit("screenshot failed: %s" % r["error"])
    return r["path"]


# --------------------------------------------------------------------------
# PNG decode (grim writes 8-bit RGB or RGBA, not interlaced), pure Python

def read_png(path):
    """Return (width, height, channels, rows); rows are bytes, unfiltered."""
    data = Path(path).read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("%s: not a PNG" % path)
    pos, idat, ihdr = 8, [], None
    while pos < len(data):
        length, kind = struct.unpack_from(">I4s", data, pos)
        body = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    w, h, depth, ctype, _, _, interlace = ihdr
    if depth != 8 or ctype not in (2, 6) or interlace:
        raise ValueError("%s: unsupported PNG (depth %d, type %d, interlace %d)"
                         % (path, depth, ctype, interlace))
    bpp = 3 if ctype == 2 else 4
    raw = zlib.decompress(b"".join(idat))
    stride = w * bpp
    rows, prev = [], bytearray(stride)
    for y in range(h):
        off = y * (stride + 1)
        ftype = raw[off]
        cur = bytearray(raw[off + 1:off + 1 + stride])
        if ftype == 1:
            for i in range(bpp, stride):
                cur[i] = (cur[i] + cur[i - bpp]) & 0xFF
        elif ftype == 2:
            for i in range(stride):
                cur[i] = (cur[i] + prev[i]) & 0xFF
        elif ftype == 3:
            for i in range(stride):
                left = cur[i - bpp] if i >= bpp else 0
                cur[i] = (cur[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:
            for i in range(stride):
                a = cur[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                cur[i] = (cur[i] + pred) & 0xFF
        elif ftype != 0:
            raise ValueError("%s: bad filter %d on row %d" % (path, ftype, y))
        rows.append(bytes(cur))
        prev = cur
    return w, h, bpp, rows


# --------------------------------------------------------------------------
# Locating the rgb curve graph
#
# src/iop/rgbcurve.c (_area_draw_callback) paints the widget .2 grey, then
# fills the graph, inset by 1 px, with a flat .3 grey (77,77,77) whatever the
# theme; grid lines, the identity line, nodes and the histogram are drawn on
# top. The drawing area is square (dtgtk_drawing_area_new_with_height(0):
# height = width), so the fill is square too. The graph's columns are fill
# nearly top to bottom, then its rows nearly side to side; the identity line
# and the grid only thin them.

FILL = 77          # .3 * 255
FILL_TOL = 8       # per channel
RUN_GAP = 4        # columns or rows short of fill tolerated inside the graph (grid lines)
EDGE_TOL = 2       # px short of square still counted as the full height
MIN_WIDTH = 100    # px


def _is_fill(row, i):
    lo, hi = FILL - FILL_TOL, FILL + FILL_TOL
    return lo <= row[i] <= hi and lo <= row[i + 1] <= hi and lo <= row[i + 2] <= hi


def _longest_span(counts, start, threshold):
    """The longest stretch of counts at or over threshold, gaps of up to
    RUN_GAP entries (grid lines) allowed inside; (first, last) or None."""
    best, run_start, last = None, None, None
    for i, c in enumerate(counts):
        if c < threshold:
            continue
        if run_start is None or i - last - 1 > RUN_GAP:
            run_start = i
        last = i
        if best is None or last - run_start > best[1] - best[0]:
            best = (run_start, last)
    return None if best is None else (start + best[0], start + best[1])


def detect_graph(png_path):
    w, h, bpp, rows = read_png(png_path)
    x_from = w * 2 // 5  # the right panel; the image itself is left alone
    # columns: the graph's are fill from top to bottom, but for the grid,
    # curve and nodes drawn over it
    cols = [sum(_is_fill(rows[y], x * bpp) for y in range(h)) for x in range(x_from, w)]
    xs = _longest_span(cols, x_from, MIN_WIDTH)
    if xs is None:
        return None, {"png_size": [w, h], "max_column": max(cols)}
    x0, x1 = xs
    width = x1 - x0 + 1
    lines = [sum(_is_fill(rows[y], x * bpp) for x in range(x0, x1 + 1)) for y in range(h)]
    ys = _longest_span(lines, 0, width // 2)
    if ys is None or width < MIN_WIDTH:
        return None, {"png_size": [w, h], "x": [x0, x1]}
    y0, y_last = ys
    # the histogram can hide the lower rows; the fill is square
    measured = y_last - y0 + 1
    if measured >= width - 2 * EDGE_TOL:
        y1, y1_from = y_last, "measured"
    else:
        y1, y1_from = y0 + width - 1, "square"
    info = {"png_size": [w, h], "measured_height": measured, "y1_from": y1_from}
    return {"x0": x0, "y0": y0, "x1": x1, "y1": y1}, info


# --------------------------------------------------------------------------
# Steps

def _graph(outdir):
    return json.loads((Path(outdir) / "graph.json").read_text())


def _settle(seconds):
    time.sleep(seconds)


def _check(result, what):
    if isinstance(result, dict) and result.get("error"):
        raise RuntimeError("%s: %s" % (what, result["error"]))
    return result


def _to_screen(comp, png_size, graph):
    """Screenshot pixels to virtual pointer units (the wl_output mode size)."""
    client = comp._vptr_client()
    sw, sh = client.screen_w, client.screen_h
    pw, ph = png_size
    if not sw or not sh or (sw, sh) == (pw, ph):
        return graph, [sw, sh]
    fx, fy = sw / pw, sh / ph
    return ({"x0": round(graph["x0"] * fx), "y0": round(graph["y0"] * fy),
             "x1": round(graph["x1"] * fx), "y1": round(graph["y1"] * fy)}, [sw, sh])


def step_locate(comp, outdir):
    env = os.environ.get("RGBC_GRAPH", "").strip()
    if env:
        x0, y0, x1, y1 = (int(v) for v in env.split(","))
        graph, info = {"x0": x0, "y0": y0, "x1": x1, "y1": y1}, {"source": "RGBC_GRAPH"}
    else:
        warm_up(comp)
        # park the pointer off the right panel so no hover text covers the graph
        _check(comp.mouse_move(10, 10), "mouse_move")
        _settle(0.5)
        png = shot(comp, "locate")
        found, info = detect_graph(png)
        info.update(source="detected", png=png)
        if found is None:
            raise RuntimeError("rgb curve graph not found in %s (%s)" % (png, info))
        graph, info["pointer_extent"] = _to_screen(comp, info["png_size"], found)
        if graph != found:
            info["png_graph"] = found
    if graph["x1"] - graph["x0"] < 20 or graph["y1"] - graph["y0"] < 20:
        raise RuntimeError("graph too small: %s" % graph)
    (Path(outdir) / "graph.json").write_text(json.dumps(graph) + "\n")
    return dict(graph, **info)


def step_case_b(comp, outdir):
    """Default mode, R tab: wheel up on an empty point, no node selected."""
    g = _graph(outdir)
    notches = int(os.environ.get("RGBC_B_NOTCHES", "3"))
    # 50 % across, 20 % down from the top: curve point (.5, .8), far from the
    # two default nodes (0,0) and (1,1) and outside their .04 pick radius
    x = g["x0"] + (g["x1"] - g["x0"]) // 2
    y = g["y0"] + (g["y1"] - g["y0"]) // 5
    warm_up(comp)
    # two motions inside the graph: the enter, then a motion at the target
    _check(comp.mouse_move(x + 6, y + 6), "mouse_move")
    _settle(0.3)
    _check(comp.mouse_move(x, y), "mouse_move")
    _settle(0.5)
    _check(comp.scroll(x, y, -notches), "scroll")  # negative = up
    _settle(1.0)
    png = shot(comp, "caseB-after")
    return {"point": [x, y], "notches_up": notches, "png": png}


def step_case_c(comp, outdir):
    """Wheel up on the selected origin node with `a` held."""
    g = _graph(outdir)
    notches = int(os.environ.get("RGBC_C_NOTCHES", "3"))
    inside = int(os.environ.get("RGBC_C_INSIDE", "3"))
    # node 0 sits at curve (0,0): the bottom-left corner of the fill
    x, y = g["x0"] + inside, g["y1"] - inside
    warm_up(comp)
    # approach from inside the graph so the pointer never leaves the widget
    # (leaving clears the selection), then stop on the node to select it
    _check(comp.mouse_move(x + 20, y - 20), "mouse_move")
    _settle(0.3)
    _check(comp.mouse_move(x, y), "mouse_move")
    _settle(0.5)
    png_selected = shot(comp, "caseC-selected")

    # raw key down/up on the virtual keyboard: wbox's hold() only takes
    # modifiers, and key()/keys() press and release at once. The same
    # persistent client carries the scroll, so the key stays down meanwhile.
    client = comp._vptr_client()
    client._ensure_vkbd()
    code, _, _ = _parse_shortcut("a")
    client._kbd_key(code, PRESSED)
    client.roundtrip()
    try:
        _settle(0.2)
        _check(comp.scroll(x, y, -notches), "scroll")  # negative = up
        _settle(0.2)
    finally:
        client = comp._vptr_client()
        client._ensure_vkbd()
        client._kbd_key(code, RELEASED)
        client.roundtrip()
    _settle(1.0)
    png = shot(comp, "caseC-after")
    return {"point": [x, y], "notches_up": notches, "held": "a",
            "png_selected": png_selected, "png": png}


def main():
    argv = sys.argv[1:]
    usage = __doc__
    if len(argv) < 2:
        sys.exit(usage)
    command, config_path, rest = argv[0], argv[1], argv[2:]
    cfg, comp = compositor(config_path)
    try:
        if command == "up":
            result = comp.launch(_build_app_cmd(cfg), _build_app_env(cfg))
        elif command == "down":
            result = comp.kill()
        elif command == "running":
            result = {"running": comp.is_running()}
        elif command == "shot":
            if len(rest) != 1:
                sys.exit(usage)
            result = {"path": shot(comp, rest[0])}
        elif command in ("locate", "caseB", "caseC"):
            if len(rest) != 1:
                sys.exit(usage)
            outdir = Path(rest[0]).resolve()
            outdir.mkdir(parents=True, exist_ok=True)
            if not comp.is_running():
                raise RuntimeError("compositor is not running")
            # screenshots land in the case's output folder
            comp.state.screenshot_dir = outdir
            step = {"locate": step_locate, "caseB": step_case_b,
                    "caseC": step_case_c}[command]
            result = dict(step(comp, outdir), ok=True)
        else:
            sys.exit(usage)
    except (RuntimeError, ValueError, OSError, KeyError, SystemExit) as exc:
        say(step=command, error=str(exc))
        return 1
    say(step=command, **(result if isinstance(result, dict) else {"result": result}))
    return 1 if isinstance(result, dict) and result.get("error") else 0


if __name__ == "__main__":
    sys.exit(main())
