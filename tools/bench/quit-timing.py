"""Time how long the desktop takes to quit, in a headless labwc driven by wbox.

Run by the bench box's `quit-timing` job (see docs/bench.md), with the
Python that has the wbox library:

    quit-timing.py --gui helios-gui --helios helios --corpus DIR --out DIR
                   [--runs N] [--settle S] [--budget-ms MS]

It imports the raw corpus into a vault, then, for each run, starts a
compositor with `helios-gui` inside (the vault, the corpus's first raw
open), waits for its window, asks it to close, and times from that request
to the window going away and to the process exiting. The runs alternate
the two ways a user closes: Ctrl+Q (egui-pop's Quit) and the compositor's
close (a window manager's close button, Alt+F4).

The desktop marks each step of its quit on standard error when
`HELIOS_QUIT_TRACE` is set (`helios_gui::quit`): the marks are lined up
with the request on the wall clock. The results go to `quit-timing.jsonl`
(one line per run) and `quit-timing.md` (a table) in the output directory.
With `--budget-ms`, a run whose window outlives the request by more than
that fails the job.
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


def config(name: str) -> dict:
    return {
        "name": name,
        "compositor": "labwc",
        "screen": "1280x800",
        "input_backend": "hybrid",
        "headless": True,
        "keyboard_layout": "us",
        "clipboard_bridge": False,
        "timeouts": {"app_render": 1, "stop": 20},
    }


def helios_windows(comp) -> list[dict] | None:
    listed = comp.list_windows()
    if "error" in listed:
        return None
    return [w for w in listed["windows"] if w.get("app_id") == APP_ID]


def wait_window(comp, proc_alive, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not proc_alive():
            return False
        if helios_windows(comp):
            return True
        time.sleep(0.1)
    return False


def close(comp, how: str) -> None:
    if how == "ctrl+q":
        comp.key("ctrl+q")
    else:
        subprocess.run(
            ["wlrctl", "toplevel", "close", f"app_id:{APP_ID}"],
            env=comp._wl_env(),
            check=False,
            capture_output=True,
            timeout=5,
        )


def marks(trace: Path) -> list[tuple[float, str]]:
    """The quit's marks: (wall clock in seconds, step)."""
    found = []
    try:
        text = trace.read_text(errors="replace")
    except OSError:
        return found
    for line in text.splitlines():
        if not line.startswith("helios-quit "):
            continue
        parts = line.split(" ", 4)
        if len(parts) == 5:
            found.append((int(parts[1]) / 1e6, parts[4]))
    return found


def run_once(args, index: int, how: str, raw: Path) -> dict:
    trace = args.out / f"run-{index}.stderr"
    comp = build_compositor(config(f"helios-quit-{index}"))
    log_dir = args.out / "wbox-log"
    if hasattr(comp, "set_log_dir"):
        comp.set_log_dir(log_dir)
    cmd = [
        "sh",
        "-c",
        'exec "$0" "$@" 2>"$HELIOS_TRACE_FILE"',
        str(args.gui),
        "--vault",
        str(args.work / "vault.db"),
        "--image",
        str(raw),
        str(args.work / "workspace"),
    ]
    env = {"HELIOS_QUIT_TRACE": "1", "HELIOS_TRACE_FILE": str(trace)}
    result: dict = {"run": index, "close": how}
    try:
        started = comp.launch(cmd, env)
        if "error" in started:
            return {**result, "error": f"launch: {started['error']}"}
        proc = comp.state.app_proc

        def alive() -> bool:
            return proc is not None and proc.poll() is None

        if not wait_window(comp, alive, 60):
            return {**result, "error": "no window"}
        time.sleep(args.settle)
        # How long one look at the windows takes: the resolution of
        # "window gone".
        t = time.time()
        helios_windows(comp)
        result["poll_ms"] = round((time.time() - t) * 1e3, 1)
        asked = time.time()
        close(comp, how)
        gone = exited = None
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline and (gone is None or exited is None):
            now = time.time()
            if exited is None and not alive():
                exited = now
            if gone is None:
                windows = helios_windows(comp)
                if windows is not None and not windows:
                    gone = time.time()
                elif exited is not None:
                    gone = exited
        if exited is None:
            proc.kill()
            result["error"] = f"still running {args.timeout} s after the close"
        result["window_gone_ms"] = None if gone is None else round((gone - asked) * 1e3, 1)
        result["exit_ms"] = None if exited is None else round((exited - asked) * 1e3, 1)
        result["exit_code"] = proc.poll()
        result["steps"] = [
            {"step": step, "ms": round((at - asked) * 1e3, 1)} for at, step in marks(trace)
        ]
        return result
    finally:
        comp.stop()


def table(results: list[dict]) -> str:
    steps: list[str] = []
    for r in results:
        for s in r.get("steps", []):
            if s["step"] not in steps:
                steps.append(s["step"])
    head = ["step"] + [f"run {r['run']} ({r['close']})" for r in results]
    rows = [head, ["---"] * len(head)]
    for step in steps:
        row = [step]
        for r in results:
            ms = next((s["ms"] for s in r.get("steps", []) if s["step"] == step), None)
            row.append("" if ms is None else f"{ms:.1f}")
        rows.append(row)
    for key, label in (("window_gone_ms", "**window gone**"), ("exit_ms", "**process exited**")):
        rows.append([label] + ["" if r.get(key) is None else f"{r[key]:.1f}" for r in results])
    rows.append(["poll resolution"] + [str(r.get("poll_ms", "")) for r in results])
    rows.append(["error"] + [r.get("error", "") for r in results])
    lines = ["| " + " | ".join(row) + " |" for row in rows]
    return (
        "Milliseconds since the close request (the script's), per step the "
        "desktop marked.\n\n" + "\n".join(lines) + "\n"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--helios", type=Path, required=True)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--work", type=Path, default=Path("/tmp/quit-timing"))
    parser.add_argument("--runs", type=int, default=6)
    parser.add_argument("--settle", type=float, default=3.0)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--budget-ms", type=float, default=0.0)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(args.work, ignore_errors=True)
    (args.work / "workspace").mkdir(parents=True)
    raws = sorted(p for p in args.corpus.iterdir() if p.suffix.lower() in RAW)
    if not raws:
        print(f"no raw in {args.corpus}", file=sys.stderr)
        return 2
    # The library: the whole corpus, referenced where it is.
    subprocess.run(
        [
            str(args.helios),
            "import",
            "--vault",
            str(args.work / "vault.db"),
            "--reference",
            str(args.corpus),
        ],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    results = []
    for i in range(args.runs):
        how = "ctrl+q" if i % 2 == 0 else "compositor"
        try:
            r = run_once(args, i + 1, how, raws[0])
        except Exception as exc:  # report and go on with the next run
            r = {"run": i + 1, "close": how, "error": repr(exc)}
        results.append(r)
        print(json.dumps(r), flush=True)
    with open(args.out / "quit-timing.jsonl", "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    report = table(results)
    (args.out / "quit-timing.md").write_text(report)
    print(report)
    failed = [r for r in results if "error" in r]
    if args.budget_ms > 0:
        failed += [
            r
            for r in results
            if r.get("window_gone_ms") is None or r["window_gone_ms"] > args.budget_ms
        ]
        print(f"budget: the window goes within {args.budget_ms:.0f} ms of the request")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
