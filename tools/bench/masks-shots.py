"""Screenshots of the Masks panel at 1920x1080, in a headless labwc driven by wbox.

Run by the bench box's `masks-shots` job (see docs/bench.md), with the
Python that has the wbox library:

    masks-shots.py --gui helios-gui --corpus DIR --out DIR
                   [--themes helios-dark,helios-light] [--screen 1920x1080]
    masks-shots.py --doc --gui helios-gui --out DIR [--screen 1280x800]

For each theme it starts a compositor with `helios-gui --image` on the
corpus's first raw (its network: the exposure through an empty mask),
maximizes the window, then drives it through the command palette
(Ctrl+Shift+P, a query, Enter), as a person would: the theme, the Masks
panel focused, then two shapes and two parametric members added. It
shoots the screen at each stage: `<theme>-1-empty.png` (the empty mask's
message), `<theme>-2-mask.png` (the mask with two parametric members
and two shapes, the last one selected with its handles on the image),
then the mask display on that selected shape: `<theme>-3-overlay.png`
(`M` with the image focused: the theme's colour over the image turned
grey) and `<theme>-4-grey.png` (the palette's next step: the mask alone
in grey).
What each step answered goes to `masks-shots.jsonl`.

With `--doc`, it draws the two masks pictures of docs/gui.md instead, at
1280x800 in the default theme, on a synthetic picture it generates (a
sky, a sun, hills and water) rather than the corpus's raw, each in a run
of its own: `masks-wbox.png` (a circle at the view's centre, then a path
of five clicks closed from the palette: the path selected, the circle
faint with its feather dashed) and `mask-tree-wbox.png` (a circle, an
ellipse and a gradient, the circle hidden, the ellipse inverted and
locked, the gradient selected, the Masks panel focused).

Tips never come by themselves in these runs (the `tips.show` setting is
off), so no card covers the panels.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

from wbox.server import build_compositor

APP_ID = "helios-gui"
RAW = (".dng", ".nef", ".cr2", ".cr3", ".arw", ".raf", ".orf", ".rw2", ".pef", ".srw")


def config(name: str, screen: str) -> dict:
    return {
        "name": name,
        "compositor": "labwc",
        "screen": screen,
        "input_backend": "hybrid",
        "headless": True,
        "keyboard_layout": "us",
        "clipboard_bridge": False,
        "timeouts": {"app_render": 1, "stop": 20},
    }


def helios_windows(comp) -> list[dict]:
    listed = comp.list_windows()
    if "error" in listed:
        return []
    return [w for w in listed["windows"] if w.get("app_id") == APP_ID]


def wait_window(comp, alive, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not alive():
            return False
        if helios_windows(comp):
            return True
        time.sleep(0.2)
    return False


def toplevel(comp, verb: str) -> dict:
    done = subprocess.run(
        ["wlrctl", "toplevel", verb, f"app_id:{APP_ID}"],
        env=comp._wl_env(),
        check=False,
        capture_output=True,
        text=True,
        timeout=5,
    )
    return {"toplevel": verb, "rc": done.returncode, "stderr": done.stderr.strip()}


def palette(comp, query: str, log: list) -> None:
    """Runs a command as a person would: the palette, a query, Enter."""
    steps = [comp.key("ctrl+shift+p")]
    time.sleep(0.8)
    steps.append(comp.type_text(query))
    time.sleep(0.8)
    steps.append(comp.key("Return"))
    time.sleep(1.5)
    log.append({"palette": query, "answers": steps})


def shoot(comp, out: Path, name: str, log: list) -> None:
    shot = comp.screenshot(name)
    log.append({"shot": name, "answer": shot})
    if "path" in shot:
        shutil.copy(shot["path"], out / f"{name}.png")


def synthetic_picture(path: Path, width: int = 1500, height: int = 1000) -> None:
    """Writes a PNG of a sky with a sun, green hills and water, in pure
    Python (the box's wbox Python has no imaging library)."""
    sun_x, sun_y, sun_r = 0.72 * width, 0.25 * height, 0.075 * height
    water = 0.78 * height
    rows = []
    for y in range(height):
        row = bytearray([0])
        t = y / height
        sky = (int(40 + 60 * t), int(80 + 70 * t), int(170 + 50 * t))
        for x in range(width):
            hill = (
                0.58 * height
                - 0.06 * height * math.sin(x / width * 7.0 + 0.6)
                - (0.03 * height * math.sin(x / width * 17.0))
            )
            if y >= water:
                band = 0.5 + 0.5 * math.sin(x / 9.0)
                px = (int(30 + 25 * band), int(90 + 30 * band), int(120 + 30 * band))
            elif y >= hill:
                depth = min(1.0, (y - hill) / (water - hill + 1))
                px = (int(50 - 20 * depth), int(150 - 60 * depth), int(45 - 15 * depth))
            else:
                d = math.hypot(x - sun_x, y - sun_y)
                glow = max(0.0, 1.0 - max(0.0, d - sun_r) / (2.2 * sun_r))
                if d <= sun_r:
                    px = (255, 255, 250)
                else:
                    px = tuple(int(c + (255 - c) * glow * glow * 0.8) for c in sky)
            row.extend(px)
        rows.append(bytes(row))

    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(b"".join(rows), 6))
        + chunk(b"IEND", b"")
    )


class Run:
    """One `helios-gui --image` in its own compositor, configuration and
    workspace, maximized once its window shows; stopped on leaving."""

    def __init__(self, args, name: str, image: Path, log: list):
        self.args, self.name, self.image, self.log = args, name, image, log
        self.comp = None
        self.ready = False

    def __enter__(self) -> Run:
        args, log = self.args, self.log
        work = args.work / self.name
        shutil.rmtree(work, ignore_errors=True)
        for d in ("config", "workspace", "shots"):
            (work / d).mkdir(parents=True)
        # A first run: the "Coming from darktable?" notice is answered
        # beforehand and tips are off, so the shots show the panels alone.
        ui = work / "config" / "helios" / "ui"
        ui.mkdir(parents=True)
        (ui / "settings.json").write_text(
            json.dumps(
                {
                    "kind": "settings",
                    "version": 1,
                    "values": {
                        "settings/helios.pointer_profile_asked": True,
                        "settings/tips.show": False,
                    },
                }
            )
        )
        comp = build_compositor(config(f"helios-masks-{self.name}", args.screen))
        self.comp = comp
        comp.state.screenshot_dir = work / "shots"
        if hasattr(comp, "set_log_dir"):
            comp.set_log_dir(args.out / "wbox-log")
        stderr = args.out / f"{self.name}.stderr"
        cmd = [
            "sh",
            "-c",
            'exec "$0" "$@" 2>"$HELIOS_SHOTS_STDERR"',
            str(args.gui),
            "--image",
            str(self.image),
            str(work / "workspace"),
        ]
        env = {
            "XDG_CONFIG_HOME": str(work / "config"),
            "HELIOS_SHOTS_STDERR": str(stderr),
        }
        started = comp.launch(cmd, env)
        log.append({"launch": started})
        if "error" in started:
            return self
        if not wait_window(comp, self.alive, 90):
            log.append({"error": "no window"})
            return self
        time.sleep(args.settle)
        log.append(toplevel(comp, "maximize"))
        time.sleep(2)
        log.append({"windows": helios_windows(comp)})
        self.ready = True
        return self

    def alive(self) -> bool:
        proc = self.comp.state.app_proc if self.comp else None
        return proc is not None and proc.poll() is None

    def __exit__(self, *exc) -> None:
        if self.comp is not None:
            self.log.append({"alive": self.alive()})
            self.comp.stop()


def run_theme(args, theme: str, raw: Path) -> list:
    log: list = [{"theme": theme, "raw": raw.name}]
    with Run(args, theme, raw, log) as run:
        if not run.ready:
            return log
        comp = run.comp
        title = theme.removeprefix("helios-").replace("-", " ").title()
        palette(comp, f"Theme: {title}", log)
        palette(comp, "Focus Masks", log)
        time.sleep(args.settle)
        shoot(comp, args.out, f"{theme}-1-empty", log)
        palette(comp, "add a parametric member", log)
        palette(comp, "add a parametric member", log)
        palette(comp, "add a circle at the view", log)
        # The ellipse last: selected, its handles on the image.
        palette(comp, "add an ellipse at the view", log)
        palette(comp, "Focus Masks", log)
        time.sleep(args.settle)
        shoot(comp, args.out, f"{theme}-2-mask", log)
        # The mask display, as a person turns it on: M with the image
        # focused, then the palette's cycle to grey.
        palette(comp, "Focus Image", log)
        log.append({"key": "m", "answer": comp.key("m")})
        time.sleep(args.settle)
        shoot(comp, args.out, f"{theme}-3-overlay", log)
        palette(comp, "show the mask (cycles", log)
        time.sleep(args.settle)
        shoot(comp, args.out, f"{theme}-4-grey", log)
    return log


# The path's five points, on the screen: up and left of the view's
# centre, crossing the circle there. At 1280x800 the image fills about
# x 300-980, y 100-560.
PATH = [(420, 200), (560, 160), (700, 230), (660, 360), (470, 340)]


def doc_viewport(args, image: Path) -> list:
    """docs/gui/masks-wbox.png: a selected path beside a faint circle."""
    log: list = [{"doc": "masks-wbox", "image": image.name}]
    with Run(args, "doc-viewport", image, log) as run:
        if not run.ready:
            return log
        comp = run.comp
        palette(comp, "add a circle at the view", log)
        palette(comp, "draw a path (click its points", log)
        for x, y in PATH:
            log.append({"click": [x, y], "answer": comp.click(x, y)})
            time.sleep(0.5)
        palette(comp, "close the path being drawn", log)
        # The pointer off the image, so no point is hovered.
        log.append({"move": comp.mouse_move(640, 700)})
        time.sleep(args.settle)
        shoot(comp, args.out, "masks-wbox", log)
    return log


def doc_tree(args, image: Path) -> list:
    """docs/gui/mask-tree-wbox.png: the Masks panel's rows, each with its
    icon, eye and lock."""
    log: list = [{"doc": "mask-tree-wbox", "image": image.name}]
    with Run(args, "doc-tree", image, log) as run:
        if not run.ready:
            return log
        comp = run.comp
        # Each shape is selected once added: the member commands act on it.
        palette(comp, "add a circle at the view", log)
        palette(comp, "set a member", log)  # no opacity: off (the eye)
        palette(comp, "add an ellipse at the view", log)
        palette(comp, "invert a member", log)
        palette(comp, "lock a member", log)
        palette(comp, "add a gradient across the view", log)
        palette(comp, "Focus Masks", log)
        log.append({"move": comp.mouse_move(640, 700)})
        time.sleep(args.settle)
        shoot(comp, args.out, "mask-tree-wbox", log)
    return log


def run_doc(args) -> bool:
    image = args.work / "synthetic.png"
    image.parent.mkdir(parents=True, exist_ok=True)
    synthetic_picture(image)
    failed = False
    with open(args.out / "masks-shots.jsonl", "w") as f:
        for scene, name in ((doc_viewport, "masks-wbox"), (doc_tree, "mask-tree-wbox")):
            try:
                log = scene(args, image)
            except Exception as exc:  # report and go on with the next scene
                log = [{"doc": name, "error": repr(exc)}]
            for entry in log:
                f.write(json.dumps(entry, default=str) + "\n")
                print(json.dumps(entry, default=str), flush=True)
            failed |= not (args.out / f"{name}.png").exists()
    return not failed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--corpus", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--work", type=Path, default=Path("/tmp/masks-shots"))
    parser.add_argument("--themes", default="helios-dark,helios-light")
    parser.add_argument("--screen")
    parser.add_argument(
        "--doc", action="store_true", help="the two masks pictures of docs/gui.md, at 1280x800"
    )
    parser.add_argument("--settle", type=float, default=3.0)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    if args.doc:
        args.screen = args.screen or "1280x800"
        return 0 if run_doc(args) else 1
    args.screen = args.screen or "1920x1080"
    if args.corpus is None:
        parser.error("--corpus is needed without --doc")
    raws = sorted(p for p in args.corpus.iterdir() if p.suffix.lower() in RAW)
    if not raws:
        print(f"no raw in {args.corpus}", file=sys.stderr)
        return 2
    failed = False
    with open(args.out / "masks-shots.jsonl", "w") as f:
        for theme in args.themes.split(","):
            try:
                log = run_theme(args, theme, raws[0])
            except Exception as exc:  # report and go on with the next theme
                log = [{"theme": theme, "error": repr(exc)}]
            for entry in log:
                f.write(json.dumps(entry, default=str) + "\n")
                print(json.dumps(entry, default=str), flush=True)
            shots = [
                args.out / f"{theme}-{n}.png" for n in ("1-empty", "2-mask", "3-overlay", "4-grey")
            ]
            failed |= not all(s.exists() for s in shots)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
