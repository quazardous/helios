"""Helios's scenarios run live, in a headless labwc driven by wbox.

Run by the bench box's `scenarios` job (see docs/bench.md), with the
Python that has the wbox library:

    scenarios.py --gui helios-gui --runner scenarios --out DIR
                 [--scenarios crates/helios-gui/tests/scenarios] [--only NAME]
                 [--screen 1280x800]

For each scenario file (`*.ron`), a world of its own: a throwaway
library (`--vault`), a workspace, a configuration with the first-run
questions answered, and a folder of known photos (the plugin tests'
JPEG fixtures and a synthetic picture). `helios-gui` starts in its own
compositor with its agent socket on and writable (`EGUI_POP_AGENT_SOCKET`,
`HELIOS_AGENT_WRITE=1`); the runner (`--runner`, the `scenarios`
example: egui-pop's scenario command line) replays the scenario over the
socket, with `$folder` and `$image` set and `gui.snapshot` as the shot
capability, checking the invariants after every step.

In `--out`, per scenario: its shots (`<scenario>-<shot>.png`, and every
collector's snapshot as `.json`), its journal (`<scenario>-journal.json`),
a failure's report (`<scenario>-failure.txt`), the compositor's picture
at the end (`<scenario>-screen.png`) and helios-gui's stderr and log
files (`<scenario>/`). One JSON line per scenario in `scenarios.jsonl`.
The exit status is 1 when a scenario failed or helios-gui did not
survive it (`NoPanic`, from outside). After the scenario, helios-gui is
closed by its `workbench.quit` command over the socket, before the
compositor stops (`quit`: `clean`, `forced` or why not).
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

from wbox.server import build_compositor

APP_ID = "helios-gui"
HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parents[1] / "crates/helios-plugin/tests/fixtures/rated-photos"


def synthetic_picture(path: Path) -> None:
    """masks-shots.py's synthetic landscape, so the darkroom shows a picture."""
    spec = importlib.util.spec_from_file_location("masks_shots", HERE / "masks-shots.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.synthetic_picture(path, 900, 600)


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


def world(work: Path) -> dict:
    """A throwaway library, workspace, configuration and photos folder."""
    shutil.rmtree(work, ignore_errors=True)
    for d in ("config", "workspace", "photos", "state", "agent"):
        (work / d).mkdir(parents=True)
    # The agent socket's folder must be private.
    os.chmod(work / "agent", 0o700)
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
    photos = work / "photos"
    for f in sorted(FIXTURES.glob("*.jpg")):
        shutil.copy(f, photos / f.name)
    image = photos / "landscape.png"
    synthetic_picture(image)
    return {
        "vault": work / "library.db",
        "workspace": work / "workspace",
        "photos": photos,
        "image": image,
        "socket": work / "agent" / "helios.sock",
    }


def quit_app(sock: Path, proc, within: float = 10.0) -> str:
    """Closes helios-gui through `workbench.quit` on its agent socket, before
    the compositor stops: `clean` when it exits within `within` seconds,
    else why not (the compositor's stop then ends it)."""
    if proc is None or proc.poll() is not None:
        return "gone"
    request = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "call",
        "params": {
            "collector": "commands",
            "action": "run",
            "args": {"id": "workbench.quit", "args": {}},
        },
    }
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(within)
            s.connect(str(sock))
            s.sendall((json.dumps(request) + "\n").encode())
            # The answer, or the socket closing with the application.
            s.recv(65536)
    except OSError as exc:
        return f"no quit: {exc}"
    try:
        proc.wait(timeout=within)
    except subprocess.TimeoutExpired:
        return "forced"
    return "clean"


def run_one(args, scenario: Path) -> dict:
    name = scenario.stem
    work = args.work / name
    w = world(work)
    keep = args.out / name
    keep.mkdir(parents=True, exist_ok=True)
    entry: dict = {"scenario": name}
    comp = build_compositor(config(f"helios-scenario-{name}", args.screen))
    comp.state.screenshot_dir = work
    if hasattr(comp, "set_log_dir"):
        comp.set_log_dir(args.out / "wbox-log")
    try:
        cmd = [
            "sh",
            "-c",
            'exec "$0" "$@" 2>"$HELIOS_SCENARIO_STDERR"',
            str(args.gui),
            "--vault",
            str(w["vault"]),
            str(w["workspace"]),
        ]
        env = {
            "XDG_CONFIG_HOME": str(work / "config"),
            "XDG_STATE_HOME": str(work / "state"),
            "EGUI_POP_AGENT_SOCKET": str(w["socket"]),
            "HELIOS_AGENT_WRITE": "1",
            "HELIOS_SCENARIO_STDERR": str(keep / "stderr.txt"),
        }
        entry["launch"] = comp.launch(cmd, env)
        if "error" in entry["launch"]:
            entry["passed"] = False
            return entry
        runner = [
            str(args.runner),
            "--socket",
            str(w["socket"]),
            "--out",
            str(args.out),
            "--var",
            f"folder={w['photos']}",
            "--var",
            f"image={w['image']}",
            "--shot",
            "gui.snapshot",
            json.dumps({"target": "window", "path": "$path"}),
            "--deadline-ms",
            str(args.deadline_ms),
            "--connect-ms",
            "90000",
            str(scenario),
        ]
        try:
            done = subprocess.run(
                runner, capture_output=True, text=True, timeout=args.timeout, check=False
            )
            entry["rc"] = done.returncode
            lines = [json.loads(line) for line in done.stdout.splitlines() if line.startswith("{")]
            entry["result"] = lines[0] if lines else None
            if done.stderr.strip():
                entry["report"] = done.stderr.strip()
        except subprocess.TimeoutExpired:
            entry["rc"] = None
            entry["report"] = f"the runner did not end within {args.timeout} s"
        proc = comp.state.app_proc
        entry["alive"] = proc is not None and proc.poll() is None
        shot = comp.screenshot(f"{name}-screen")
        if "path" in shot:
            shutil.copy(shot["path"], args.out / f"{name}-screen.png")
        entry["passed"] = entry.get("rc") == 0 and entry["alive"]
        # Closed by its own quit command, not by the compositor going away
        # (a broken pipe and winit's event loop error at teardown).
        entry["quit"] = quit_app(w["socket"], proc)
        return entry
    finally:
        comp.stop()
        # helios-gui's log files and crash reports, whatever happened.
        state = work / "state"
        if state.exists():
            shutil.copytree(state, keep / "state", dirs_exist_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--scenarios", type=Path, default=HERE.parents[1] / "crates/helios-gui/tests/scenarios"
    )
    parser.add_argument("--only", default="")
    parser.add_argument("--work", type=Path, default=Path("/tmp/helios-scenarios"))
    parser.add_argument("--screen", default="1280x800")
    parser.add_argument("--deadline-ms", type=int, default=30000)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    files = sorted(args.scenarios.glob("*.ron"))
    if args.only:
        files = [f for f in files if f.stem in args.only.split(",")]
    if not files:
        print(f"no scenario in {args.scenarios} ({args.only or 'all'})", file=sys.stderr)
        return 2
    failed = False
    with open(args.out / "scenarios.jsonl", "w") as f:
        for scenario in files:
            started = time.monotonic()
            try:
                entry = run_one(args, scenario)
            except Exception as exc:  # report and go on with the next one
                entry = {"scenario": scenario.stem, "passed": False, "error": repr(exc)}
            entry["seconds"] = round(time.monotonic() - started, 1)
            failed |= not entry.get("passed")
            line = json.dumps(entry, default=str)
            f.write(line + "\n")
            print(line, flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
