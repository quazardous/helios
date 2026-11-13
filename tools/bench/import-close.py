"""Time how long the main window takes to draw again once the import window
closes, in a headless labwc driven by wbox.

Run by the bench box's `import-close` job (see docs/bench.md), with the
Python that has the wbox library:

    import-close.py --gui helios-gui --out DIR [--folder DIR]
                    [--ways cancel,escape,compositor,import] [--covers shown,covered]
                    [--runs N]
                    [--budget-ms MS]

`helios-gui` starts in its own compositor on a throwaway library, with its
agent socket on (`EGUI_POP_AGENT_SOCKET`, `HELIOS_AGENT_WRITE=1`) and its
frame trace (`HELIOS_FRAME_TRACE`: a line on standard error per frame
begun, with the viewport, and when the import window closes). For each
way of closing, in turn: the import window is opened on `--folder` (a
folder of photos, the raw corpus on the box), left to read it for
`--settle` seconds, then closed:

- `cancel`: its Cancel button, clicked over the agent socket;
- `escape`: the Escape key, pressed in it over the agent socket;
- `import`: its Import button (the folder imported, the window closed);
- `compositor`: the compositor's close (a window manager's close button).

Each way runs twice (`--covers`): with the main window `shown` beside the
import window, then `covered` (the import window full screen over it, as
a browser opened for a sign-in or a picking covers it): a compositor
sends a hidden window no frame callbacks, so its next frame waits until
it shows again. The agent socket is served by the main window's frames:
covered, only the compositor's close runs. Each close runs in a
compositor and a helios-gui of its own.

From the close on, it times the first frame the main window begins, the
first one it builds, and the first answer of the agent socket (served by
the interface's thread); while the main window draws nothing, the main
thread's kernel wait channel (`/proc/<pid>/task/<pid>/wchan`) is sampled,
to tell a wait on the GPU from a lock. One JSON line per close in
`import-close.jsonl`, a table in `import-close.md`; with `--budget-ms`, a
close after which the main window takes longer to draw fails the job.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

from wbox.server import build_compositor

APP_ID = "helios-gui"
HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parents[1] / "crates/helios-plugin/tests/fixtures/rated-photos"
MAIN_BEGINS = "main window begins"
WAYS = ("cancel", "escape", "compositor", "import")
DEFAULT_WAYS = ("cancel", "compositor")


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


def world(work: Path) -> dict:
    shutil.rmtree(work, ignore_errors=True)
    for d in ("config", "workspace", "state", "agent"):
        (work / d).mkdir(parents=True)
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
    return {
        "vault": work / "library.db",
        "workspace": work / "workspace",
        "state": work / "state",
        "socket": work / "agent" / "helios.sock",
    }


class Agent:
    """helios-gui's agent socket: one JSON-RPC request per connection."""

    def __init__(self, path: Path):
        self.path = path
        self.next_id = 0

    def request(self, method: str, params: dict, within: float = 30.0) -> dict:
        self.next_id += 1
        line = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(within)
            s.connect(str(self.path))
            s.sendall((json.dumps(line) + "\n").encode())
            data = b""
            while not data.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                data += chunk
        answer = json.loads(data.decode() or "{}")
        if "error" in answer:
            raise RuntimeError(f"{method}: {answer['error']}")
        return answer.get("result", {})

    def command(self, cid: str, args: dict) -> dict:
        return self.request(
            "call",
            {"collector": "commands", "action": "run", "args": {"id": cid, "args": args}},
        )

    def ui(self, params: dict) -> dict:
        return self.request("ui", params)


def wait_socket(agent: Agent, alive, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline and alive():
        try:
            agent.request("describe", {}, within=5)
            return True
        except (OSError, RuntimeError, ValueError):
            time.sleep(0.2)
    return False


def frames(trace: Path) -> list[tuple[float, str]]:
    found = []
    try:
        text = trace.read_text(errors="replace")
    except OSError:
        return found
    for line in text.splitlines():
        if not line.startswith("helios-frame "):
            continue
        parts = line.split(" ", 2)
        if len(parts) == 3:
            found.append((int(parts[1]) / 1e6, parts[2]))
    return found


def helios_pid(state: Path) -> int | None:
    for marker in sorted((state / "helios" / "crashes").glob("session-*.json")):
        try:
            return int(marker.stem.removeprefix("session-"))
        except ValueError:
            continue
    return None


def wchan(pid: int | None) -> str:
    if pid is None:
        return "?"
    try:
        chan = Path(f"/proc/{pid}/task/{pid}/wchan").read_text().strip() or "0"
        stat = Path(f"/proc/{pid}/task/{pid}/stat").read_text()
        return f"{stat.rsplit(')', 1)[1].split()[0]}:{chan}"
    except OSError:
        return "?"


def stack(pid: int | None) -> str | None:
    """The main thread's stack, by gdb or eu-stack when the box has one."""
    if pid is None:
        return None
    for cmd in (
        ["eu-stack", "-1", "-p", str(pid)],
        ["gdb", "-batch", "-p", str(pid), "-ex", "bt 40"],
    ):
        if shutil.which(cmd[0]):
            try:
                done = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            except subprocess.TimeoutExpired:
                continue
            return (done.stdout + done.stderr)[-6000:]
    return None


def import_title(comp, before: set) -> str | None:
    """The window listed now that was not before the import window opened."""
    listed = comp.list_windows()
    for w in listed.get("windows", []):
        if (w.get("app_id"), w.get("title")) not in before:
            return w.get("title")
    return None


def listed_windows(comp) -> set:
    return {(w.get("app_id"), w.get("title")) for w in comp.list_windows().get("windows", [])}


def close(comp, agent: Agent, way: str, title: str) -> None:
    if way == "cancel":
        agent.ui({"action": "click", "window": "import", "role": "Button", "label": "Cancel"})
    elif way == "escape":
        agent.ui({"action": "key", "window": "import", "key": "Escape"})
    elif way == "import":
        tree = agent.ui({"action": "tree", "window": "import", "role": "Button"})
        label = next(
            (
                n["label"]
                for n in tree.get("nodes", [])
                if str(n.get("label", "")).startswith("Import ") and not n.get("disabled")
            ),
            None,
        )
        if label is None:
            raise RuntimeError("no Import button enabled")
        agent.ui({"action": "click", "window": "import", "role": "Button", "label": label})
    else:
        subprocess.run(
            ["wlrctl", "toplevel", "close", f"title:{title}"],
            env=comp._wl_env(),
            check=False,
            capture_output=True,
            timeout=5,
        )


def one_close(
    args, comp, agent: Agent, trace: Path, pid, main_title: str, way: str, cover: str
) -> dict:
    result: dict = {"close": way, "cover": cover}
    before = listed_windows(comp)
    agent.command("library.importWindow", {"source": str(args.folder), "copy": False})
    deadline = time.monotonic() + 30
    title = None
    while time.monotonic() < deadline and title is None:
        title = import_title(comp, before)
        time.sleep(0.1)
    if title is None:
        listed = comp.list_windows()
        return {**result, "error": f"no import window listed: {listed}"}
    # Left to read the folder (scans, glances, thumbnails).
    time.sleep(args.settle)
    if cover == "covered":
        # The import window over the whole screen: the main window is
        # hidden, as behind a browser opened for a sign-in or a picking.
        subprocess.run(
            ["wlrctl", "toplevel", "fullscreen", f"title:{title}"],
            env=comp._wl_env(),
            check=False,
            capture_output=True,
            timeout=5,
        )
        time.sleep(1.5)
    asked = time.time()
    try:
        close(comp, agent, way, title)
    except (OSError, RuntimeError, ValueError) as exc:
        return {**result, "error": f"close: {exc}"}
    # The agent socket's first answer after the close, on a thread: it
    # waits on the interface's thread.
    answered: dict = {}

    def ping() -> None:
        try:
            agent.request("describe", {}, within=args.timeout)
            answered["at"] = time.time()
        except (OSError, RuntimeError, ValueError) as exc:
            answered["error"] = str(exc)

    pinger = threading.Thread(target=ping, daemon=True)
    pinger.start()
    began = built = gone = None
    waits: list[str] = []
    stacked = False
    end = time.monotonic() + args.timeout
    while time.monotonic() < end:
        marks = [(at, what) for at, what in frames(trace) if at >= asked]
        if began is None:
            began = next((at for at, w in marks if w == MAIN_BEGINS), None)
        closed = next((at for at, w in marks if w == "import window closed"), None)
        if built is None and closed is not None:
            # The first main window frame begun after the one that closed it.
            built = next(
                (at for at, w in marks if w == MAIN_BEGINS and at > closed + 1e-4),
                None,
            )
        if gone is None and title not in {t for _, t in listed_windows(comp)}:
            gone = time.time()
        if built is not None and gone is not None and "at" in answered:
            break
        if built is None:
            w = wchan(pid)
            if not waits or waits[-1].split(" ", 1)[1] != w:
                waits.append(f"{round((time.time() - asked) * 1e3)}ms {w}")
            if not stacked and time.time() - asked > 3:
                stacked = True
                result["stack"] = stack(pid)
        time.sleep(0.05)
    pinger.join(timeout=1)

    def since(at):
        return None if at is None else round((at - asked) * 1e3, 1)

    marks = [(at, what) for at, what in frames(trace) if at >= asked]
    closed = next((at for at, w in marks if w == "import window closed"), None)
    result["closed_ms"] = since(closed)
    result["main_frame_after_close_ms"] = since(built)
    result["window_gone_ms"] = since(gone)
    result["answered_ms"] = since(answered.get("at"))
    result["main_thread_waits"] = waits
    # The longest gap between two main window frames after the close.
    roots = [at for at, w in marks if w == MAIN_BEGINS]
    gaps = [b - a for a, b in zip(roots, roots[1:])]
    result["longest_main_gap_ms"] = round(max(gaps) * 1e3, 1) if gaps else None
    if built is None:
        result["error"] = f"the main window drew nothing within {args.timeout:.0f} s of the close"
    shot = comp.screenshot(f"after-{way}-{cover}")
    if "path" in shot:
        shutil.copy(shot["path"], args.out / f"after-{way}-{cover}.png")
    # A way that did not close it (Escape where it does not): closed by
    # Cancel, so the next way opens it afresh.
    if title in {t for _, t in listed_windows(comp)}:
        try:
            close(comp, agent, "cancel", title)
        except (OSError, RuntimeError, ValueError):
            pass
        until = time.monotonic() + 10
        while time.monotonic() < until and title in {t for _, t in listed_windows(comp)}:
            time.sleep(0.1)
    return result


def run(args, index: int, way: str, cover: str) -> dict:
    """One close, in a compositor and a helios-gui of its own."""
    work = args.work / f"run-{index}"
    w = world(work)
    trace = args.out / f"run-{index}.stderr"
    comp = build_compositor(config(f"helios-import-close-{index}"))
    if hasattr(comp, "set_log_dir"):
        comp.set_log_dir(args.out / "wbox-log")
    try:
        cmd = [
            "sh",
            "-c",
            'exec "$0" "$@" 2>"$HELIOS_TRACE_FILE"',
            str(args.gui),
            "--vault",
            str(w["vault"]),
            str(w["workspace"]),
        ]
        env = {
            "XDG_CONFIG_HOME": str(work / "config"),
            "XDG_STATE_HOME": str(w["state"]),
            "EGUI_POP_AGENT_SOCKET": str(w["socket"]),
            "HELIOS_AGENT_WRITE": "1",
            "HELIOS_FRAME_TRACE": "1",
            "HELIOS_TRACE_FILE": str(trace),
        }
        launched = comp.launch(cmd, env)
        if "error" in launched:
            return {"run": index, "close": way, "cover": cover, "error": "launch failed"}
        proc = comp.state.app_proc

        def alive() -> bool:
            return proc is not None and proc.poll() is None

        agent = Agent(w["socket"])
        if not wait_socket(agent, alive, 90):
            return {"run": index, "close": way, "cover": cover, "error": "no agent socket"}
        pid = helios_pid(w["state"])
        listed = comp.list_windows().get("windows", [])
        main_title = next((x["title"] for x in listed if x.get("app_id") == APP_ID), "")
        try:
            r = one_close(args, comp, agent, trace, pid, main_title, way, cover)
        except Exception as exc:  # reported, the next close runs
            r = {"close": way, "cover": cover, "error": repr(exc)}
        r["run"] = index
        return r
    finally:
        comp.stop()
        state = w["state"]
        if state.exists():
            shutil.copytree(state, args.out / f"run-{index}-state", dirs_exist_ok=True)


def table(results: list[dict]) -> str:
    keys = (
        ("closed_ms", "import window closed"),
        ("main_frame_after_close_ms", "**main window's next frame**"),
        ("window_gone_ms", "import window gone (compositor)"),
        ("answered_ms", "agent socket answered"),
        ("longest_main_gap_ms", "longest gap between main frames"),
    )
    head = ["", *[f"{r.get('run')}: {r.get('close')}, {r.get('cover')}" for r in results]]
    rows = [head, ["---"] * len(head)]
    for key, label in keys:
        rows.append([label, *["" if r.get(key) is None else f"{r[key]:.0f}" for r in results]])
    rows.append(["error", *[r.get("error", "") for r in results]])
    return (
        "Milliseconds since the close was asked.\n\n"
        + "\n".join("| " + " | ".join(row) + " |" for row in rows)
        + "\n"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--folder", type=Path, default=FIXTURES)
    parser.add_argument("--ways", default=",".join(DEFAULT_WAYS))
    parser.add_argument("--covers", default="shown,covered")
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--settle", type=float, default=4.0)
    parser.add_argument("--timeout", type=float, default=40.0)
    parser.add_argument("--budget-ms", type=float, default=0.0)
    parser.add_argument("--work", type=Path, default=Path("/tmp/import-close"))
    args = parser.parse_args()
    args.ways = [w for w in args.ways.replace("+", ",").split(",") if w]
    args.covers = [c for c in args.covers.replace("+", ",").split(",") if c]
    if set(args.covers) - {"shown", "covered"}:
        print(f"unknown covers: {args.covers}", file=sys.stderr)
        return 2
    unknown = set(args.ways) - set(WAYS)
    if unknown or not args.ways:
        print(f"unknown ways: {sorted(unknown)}", file=sys.stderr)
        return 2
    args.out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    results: list[dict] = []
    # The agent socket is served by the main window's frames, which a
    # covered main window does not get: covered, the import window is
    # closed by the compositor alone.
    combos = [(w, c) for c in args.covers for w in args.ways if c == "shown" or w == "compositor"]
    index = 0
    for _ in range(args.runs):
        for way, cover in combos:
            index += 1
            r = run(args, index, way, cover)
            results.append(r)
            print(json.dumps(r), flush=True)
    with open(args.out / "import-close.jsonl", "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    report = table(results)
    (args.out / "import-close.md").write_text(report)
    print(report)
    failed = [r for r in results if "error" in r]
    if args.budget_ms > 0:
        failed += [
            r
            for r in results
            if (r.get("main_frame_after_close_ms") or float("inf")) > args.budget_ms
        ]
        print(f"budget: the main window draws within {args.budget_ms:.0f} ms of the close")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
