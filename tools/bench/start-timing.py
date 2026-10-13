"""Time how long the desktop takes to start, in a headless labwc driven by wbox.

Run by the bench box's `start-timing` job (see docs/bench.md), with the
Python that has the wbox library:

    start-timing.py --gui helios-gui --helios helios --corpus DIR --out DIR
                    [--runs N] [--kinds launcher,library,image]
                    [--budget-ms FIRST] [--interactive-ms INTERACTIVE]

It imports the raw corpus into a vault, then, for each run, starts a
compositor, launches `helios-gui` inside it and times, from the process's
start:

- the first frame: the desktop's own mark (`helios_gui::start`) of its
  first frame presented, with when the compositor first listed its window;
- interactive: the first frame that answered an input. From the launch on,
  the script moves the pointer over the window's place every few
  milliseconds; the desktop marks the first frame that saw it.

The runs go through the kinds in turn: `launcher` (a workspace and nothing
else, as the desktop's launcher starts it), `library` (the corpus's vault)
and `image` (the vault and the corpus's first raw open). The first run
starts with no settings; the others find the ones the run before saved
(each run quits by Ctrl+Q).

The desktop marks each step of its start on standard error when
`HELIOS_START_TRACE` is set: the marks are lined up with the process's
start. The results go to `start-timing.jsonl` (one line per run) and
`start-timing.md` (a table) in the output directory. With `--budget-ms`,
a run slower to its first frame fails the job; with `--interactive-ms`,
a run slower to interactive (it defaults to twice the first's budget).
The kinds are separated by `,` or `+` (the box takes no commas).
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
FIRST_FRAME = "first frame presented"
INTERACTIVE = "interactive (an input answered)"
SCREEN = (1280, 800)


def config(name: str) -> dict:
    return {
        "name": name,
        "compositor": "labwc",
        "screen": f"{SCREEN[0]}x{SCREEN[1]}",
        "input_backend": "hybrid",
        "headless": True,
        "keyboard_layout": "us",
        "clipboard_bridge": False,
        # launch() returns as soon as the application is started: the
        # script times the rest.
        "timeouts": {"app_render": 0, "stop": 20},
    }


def helios_windows(comp) -> list[dict] | None:
    listed = comp.list_windows()
    if "error" in listed:
        return None
    return [w for w in listed["windows"] if w.get("app_id") == APP_ID]


def marks(trace: Path) -> list[tuple[float, str]]:
    """The start's marks: (wall clock in seconds, step)."""
    found = []
    try:
        text = trace.read_text(errors="replace")
    except OSError:
        return found
    for line in text.splitlines():
        if not line.startswith("helios-start "):
            continue
        parts = line.split(" ", 4)
        if len(parts) == 5:
            found.append((int(parts[1]) / 1e6, parts[4]))
    return found


def process_start(pid: int) -> float | None:
    """When a process started, on the wall clock (to the kernel's tick)."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        boot = next(
            int(line.split()[1])
            for line in Path("/proc/stat").read_text().splitlines()
            if line.startswith("btime ")
        )
    except (OSError, StopIteration, ValueError):
        return None
    # The fields after the command's closing parenthesis; starttime is
    # the 22nd field of the line, the 20th after it.
    fields = stat.rsplit(")", 1)[1].split()
    ticks = int(fields[19])
    return boot + ticks / os.sysconf("SC_CLK_TCK")


def launch_args(args, kind: str, raw: Path) -> list[str]:
    cmd = [str(args.gui)]
    if kind in ("library", "image"):
        cmd += ["--vault", str(args.work / "vault.db")]
    if kind == "image":
        cmd += ["--image", str(raw)]
    return cmd + [str(args.work / "workspace")]


def run_once(args, index: int, kind: str, raw: Path) -> dict:
    trace = args.out / f"run-{index}.stderr"
    comp = build_compositor(config(f"helios-start-{index}"))
    if hasattr(comp, "set_log_dir"):
        comp.set_log_dir(args.out / "wbox-log")
    gui = launch_args(args, kind, raw)
    cmd = ["sh", "-c", 'exec "$0" "$@" 2>"$HELIOS_TRACE_FILE"', *gui]
    env = {"HELIOS_START_TRACE": "1", "HELIOS_TRACE_FILE": str(trace)}
    result: dict = {"run": index, "kind": kind}
    try:
        launched = comp.launch(cmd, env)
        if "error" in launched:
            return {**result, "error": f"launch: {launched['error']}"}
        proc = comp.state.app_proc
        started = process_start(proc.pid) if proc is not None else None

        def alive() -> bool:
            return proc is not None and proc.poll() is None

        listed = None
        answered = False
        deadline = time.monotonic() + args.timeout
        centre = (SCREEN[0] // 2, SCREEN[1] // 2)
        step = 0
        # Move the pointer until the desktop says a frame answered it.
        while time.monotonic() < deadline and alive():
            step += 1
            comp.mouse_move(centre[0] + step % 7, centre[1] + step % 5)
            if listed is None and helios_windows(comp):
                listed = time.time()
            if any(s == INTERACTIVE for _, s in marks(trace)):
                answered = True
                break
            time.sleep(0.02)
        found = marks(trace)
        origin = started
        if origin is None:
            origin = next((at for at, s in found if s == "main"), None)
        if origin is None:
            return {**result, "error": "no start time (no process, no mark)"}

        def since(at: float | None) -> float | None:
            return None if at is None else round((at - origin) * 1e3, 1)

        result["steps"] = [{"step": s, "ms": since(at)} for at, s in found]
        result["window_listed_ms"] = since(listed)
        result["first_frame_ms"] = since(next((at for at, s in found if s == FIRST_FRAME), None))
        result["interactive_ms"] = since(next((at for at, s in found if s == INTERACTIVE), None))
        if not answered:
            why = "exited" if not alive() else f"no input answered within {args.timeout:.0f} s"
            result["error"] = why
        if alive():
            # Quit as a user does, so the next run finds the settings saved.
            comp.key("ctrl+q")
            try:
                proc.wait(timeout=args.timeout)
            except subprocess.TimeoutExpired:
                proc.kill()
                result.setdefault("error", f"still running {args.timeout:.0f} s after Ctrl+Q")
        return result
    finally:
        comp.stop()


def table(results: list[dict]) -> str:
    steps: list[str] = []
    for r in results:
        for s in r.get("steps", []):
            if s["step"] not in steps:
                steps.append(s["step"])
    head = ["step"] + [f"run {r['run']} ({r['kind']})" for r in results]
    rows = [head, ["---"] * len(head)]
    for step in steps:
        row = [step]
        for r in results:
            ms = next((s["ms"] for s in r.get("steps", []) if s["step"] == step), None)
            row.append("" if ms is None else f"{ms:.1f}")
        rows.append(row)
    for key, label in (
        ("window_listed_ms", "window listed (compositor)"),
        ("first_frame_ms", "**first frame**"),
        ("interactive_ms", "**interactive**"),
    ):
        rows.append([label] + ["" if r.get(key) is None else f"{r[key]:.1f}" for r in results])
    rows.append(["error"] + [r.get("error", "") for r in results])
    lines = ["| " + " | ".join(row) + " |" for row in rows]
    return (
        "Milliseconds since the process started, per step the desktop marked.\n\n"
        + "\n".join(lines)
        + "\n"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--helios", type=Path, required=True)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--work", type=Path, default=Path("/tmp/start-timing"))
    parser.add_argument("--runs", type=int, default=6)
    parser.add_argument("--kinds", default="launcher,library,image")
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--budget-ms", type=float, default=0.0)
    parser.add_argument("--interactive-ms", type=float, default=0.0)
    args = parser.parse_args()
    budget = None
    if args.budget_ms > 0 or args.interactive_ms > 0:
        first = args.budget_ms or float("inf")
        budget = (first, args.interactive_ms or 2 * first)
    kinds = [k for k in args.kinds.replace("+", ",").split(",") if k]
    unknown = set(kinds) - {"launcher", "library", "image"}
    if unknown or not kinds:
        print(f"unknown kinds: {sorted(unknown)}", file=sys.stderr)
        return 2
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
        timeout=600,
    )
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    results = []
    for i in range(args.runs):
        kind = kinds[i % len(kinds)]
        try:
            r = run_once(args, i + 1, kind, raws[0])
        except Exception as exc:  # report and go on with the next run
            r = {"run": i + 1, "kind": kind, "error": repr(exc)}
        results.append(r)
        print(json.dumps(r), flush=True)
    with open(args.out / "start-timing.jsonl", "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    report = table(results)
    (args.out / "start-timing.md").write_text(report)
    print(report)
    failed = [r for r in results if "error" in r]
    if budget is not None:
        first, interactive = budget
        failed += [
            r
            for r in results
            if r.get("first_frame_ms") is None
            or r["first_frame_ms"] > first
            or r.get("interactive_ms") is None
            or r["interactive_ms"] > interactive
        ]
        print(
            f"budget: the first frame within {first:.0f} ms, "
            f"interactive within {interactive:.0f} ms of the start"
        )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
