"""Screenshots of the Masks panel at 1920x1080, in a headless labwc driven by wbox.

Run by the bench box's `masks-shots` job (see docs/bench.md), with the
Python that has the wbox library:

    masks-shots.py --gui helios-gui --corpus DIR --out DIR
                   [--themes helios-dark,helios-light] [--screen 1920x1080]

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
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
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


def run_theme(args, theme: str, raw: Path) -> list:
    log: list = [{"theme": theme, "raw": raw.name}]
    work = args.work / theme
    shutil.rmtree(work, ignore_errors=True)
    for d in ("config", "workspace", "shots"):
        (work / d).mkdir(parents=True)
    # A first run: the "Coming from darktable?" notice is answered
    # beforehand, so the shots show the panels alone.
    ui = work / "config" / "helios" / "ui"
    ui.mkdir(parents=True)
    (ui / "settings.json").write_text(
        json.dumps(
            {
                "kind": "settings",
                "version": 1,
                "values": {"settings/helios.pointer_profile_asked": True},
            }
        )
    )
    comp = build_compositor(config(f"helios-masks-{theme}", args.screen))
    comp.state.screenshot_dir = work / "shots"
    if hasattr(comp, "set_log_dir"):
        comp.set_log_dir(args.out / "wbox-log")
    stderr = args.out / f"{theme}.stderr"
    cmd = [
        "sh",
        "-c",
        'exec "$0" "$@" 2>"$HELIOS_SHOTS_STDERR"',
        str(args.gui),
        "--image",
        str(raw),
        str(work / "workspace"),
    ]
    env = {
        "XDG_CONFIG_HOME": str(work / "config"),
        "HELIOS_SHOTS_STDERR": str(stderr),
    }
    try:
        started = comp.launch(cmd, env)
        log.append({"launch": started})
        if "error" in started:
            return log
        proc = comp.state.app_proc

        def alive() -> bool:
            return proc is not None and proc.poll() is None

        if not wait_window(comp, alive, 90):
            log.append({"error": "no window"})
            return log
        time.sleep(args.settle)
        log.append(toplevel(comp, "maximize"))
        time.sleep(2)
        log.append({"windows": helios_windows(comp)})
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
        log.append({"alive": alive()})
        return log
    finally:
        comp.stop()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--work", type=Path, default=Path("/tmp/masks-shots"))
    parser.add_argument("--themes", default="helios-dark,helios-light")
    parser.add_argument("--screen", default="1920x1080")
    parser.add_argument("--settle", type=float, default=3.0)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    raws = sorted(p for p in args.corpus.iterdir() if p.suffix.lower() in RAW)
    if not raws:
        print(f"no raw in {args.corpus}", file=sys.stderr)
        return 2
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
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
                args.out / f"{theme}-{n}.png"
                for n in ("1-empty", "2-mask", "3-overlay", "4-grey")
            ]
            failed |= not all(s.exists() for s in shots)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
