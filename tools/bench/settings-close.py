"""Close the Settings app's own window by every way, in a headless
compositor driven by wbox, and check no window is left behind.

Run by the bench box's `settings-close` job (see docs/bench.md), with the
Python that has the wbox library:

    settings-close.py --gui helios-gui --out DIR [--compositor labwc|mutter]

`helios-gui` starts on a throwaway library with its agent socket on.
Then, for each way (`--ways`, joined by `+`), the Settings app is opened
in its own window on Health › Health check (`help.health`), the check run
when `--check` says so, and the window closed:

- `titlebar`: its title bar's close button, clicked through accesskit;
- `page`: the page's own Close button;
- `escape`: Escape pressed in the window;
- `command`: `window.close` asked for the window (as the Window menu does);
- `admin`: `admin.close` asked from the main window;
- `compositor`: the compositor closes it (`wlrctl`, labwc only);
- `after-import`: the import window opened and closed first, then the
  title bar's close;
- `with-import`: the import window left open beside, then the title
  bar's close (the import window must stay).

After each close, once the paint guard's delay has passed: the windows
the compositor lists, the windows Helios says are open, the Settings
app's state, and a screenshot. A way passes when the compositor lists the
main window alone (without a window list, on mutter: when the screen
differs little from a shot of the main window alone), Helios lists only `main`, the Settings app is closed
and the screen is not one flat colour.

One JSON line per way in `settings-close.jsonl`, a table in
`settings-close.md`, the screenshots and helios-gui's logs in `--out`;
the exit status is 1 when a way failed.
"""

from __future__ import annotations

import argparse
import contextlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("import_close", HERE / "import-close.py")
ic = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ic)

WAYS = (
    "titlebar",
    "page",
    "escape",
    "command",
    "admin",
    "compositor",
    "after-import",
    "with-import",
)


def build(name: str, compositor: str):
    from wbox.server import build_compositor

    return build_compositor(dict(ic.config(name), compositor=compositor))


def png_is_flat(path: Path) -> bool:
    """Whether a PNG is one flat colour: its pixel bytes, filters and all,
    hold almost no distinct values."""
    data = path.read_bytes()
    pos, idat = 8, b""
    while pos < len(data):
        size = int.from_bytes(data[pos : pos + 4], "big")
        if data[pos + 4 : pos + 8] == b"IDAT":
            idat += data[pos + 8 : pos + 8 + size]
        pos += size + 12
    return len(set(zlib.decompress(idat))) < 16


def listed(comp) -> list[dict]:
    try:
        return [
            {k: w.get(k) for k in ("app_id", "title", "activated")}
            for w in comp.list_windows().get("windows", [])
        ]
    except Exception as exc:  # reported, not fatal
        return [{"error": repr(exc)}]


def app_windows(agent) -> list[str]:
    try:
        return [w.get("id") for w in agent.request("describe", {}).get("windows", [])]
    except (OSError, RuntimeError, ValueError, AttributeError) as exc:
        return [f"error: {exc}"]


def admin_state(agent) -> dict:
    try:
        s = agent.request("snapshot", {"collector": "admin"}) or {}
        s = s.get("admin", s) if isinstance(s, dict) else {}
        return {k: s.get(k) for k in ("open", "scope", "section", "window")}
    except (OSError, RuntimeError, ValueError) as exc:
        return {"error": str(exc)}


def shot(comp, out: Path, name: str) -> Path | None:
    s = comp.screenshot(name)
    if "path" not in s:
        return None
    dest = out / f"{name}.png"
    shutil.copy(s["path"], dest)
    return dest


def closes(agent, window: str) -> list[dict]:
    try:
        return agent.ui(
            {"action": "tree", "window": window, "role": "Button", "label": "Close"}
        ).get("nodes", [])
    except (OSError, RuntimeError, ValueError):
        return []


def close(comp, agent, way: str, match: str | None, compositor: str) -> str | None:
    """Closes the Settings window by `way`; an error, or `None`."""
    if way == "titlebar":
        agent.ui(
            {"action": "click", "window": "settings", "role": "Button", "label": "Close", "nth": 0}
        )
    elif way == "page":
        n = len(closes(agent, "settings"))
        if n < 2:
            return f"{n} Close button(s) in the Settings window, 2 expected"
        agent.ui(
            {
                "action": "click",
                "window": "settings",
                "role": "Button",
                "label": "Close",
                "nth": n - 1,
            }
        )
    elif way == "escape":
        agent.ui({"action": "key", "window": "settings", "key": "Escape"})
    elif way == "command":
        agent.command("window.close", {"window": "settings"})
    elif way == "admin":
        agent.command("admin.close", {})
    elif way == "compositor":
        if compositor != "labwc" or match is None:
            return f"no compositor close here ({compositor}, no window list)"
        subprocess.run(
            ["wlrctl", "toplevel", "close", match],
            env=comp._wl_env(),
            check=False,
            capture_output=True,
            timeout=5,
        )
    return None


def pixels(path: Path) -> tuple[int, int, int, bytes]:
    """A PNG's size, bytes per pixel and unfiltered pixel rows (8-bit RGB
    or RGBA, not interlaced)."""
    data = path.read_bytes()
    pos, idat, head = 8, b"", b""
    while pos < len(data):
        size = int.from_bytes(data[pos : pos + 4], "big")
        kind = data[pos + 4 : pos + 8]
        if kind == b"IHDR":
            head = data[pos + 8 : pos + 8 + size]
        elif kind == b"IDAT":
            idat += data[pos + 8 : pos + 8 + size]
        pos += size + 12
    width, height = int.from_bytes(head[0:4], "big"), int.from_bytes(head[4:8], "big")
    bpp = {2: 3, 6: 4}[head[9]]
    raw = zlib.decompress(idat)
    stride = width * bpp
    out = bytearray()
    prev = bytearray(stride)
    i = 0
    for _ in range(height):
        f = raw[i]
        line = bytearray(raw[i + 1 : i + 1 + stride])
        i += 1 + stride
        for x in range(stride if f else 0):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if pa <= pb and pa <= pc else b if pb <= pc else c
                line[x] = (line[x] + pr) & 255
        out += line
        prev = line
    return width, height, bpp, bytes(out)


def changed_share(a: Path, b: Path) -> float | None:
    """The share of pixels that differ between two shots of one size,
    sampled every 4th pixel of every 4th row."""
    try:
        wa, ha, pa, da = pixels(a)
        wb, hb, pb, db = pixels(b)
    except (OSError, ValueError, KeyError, zlib.error):
        return None
    if (wa, ha) != (wb, hb):
        return None
    seen = differ = 0
    for y in range(0, ha, 4):
        for x in range(0, wa, 4):
            ia, ib = (y * wa + x) * pa, (y * wb + x) * pb
            seen += 1
            if any(abs(da[ia + k] - db[ib + k]) > 8 for k in range(3)):
                differ += 1
    return differ / seen if seen else None


def open_settings(args, agent) -> str | None:
    """Opens the Settings app in its own window on Health › Health check,
    as Help › Health check does; an error, or `None`."""
    try:
        agent.command("help.health", {})
    except (OSError, RuntimeError, ValueError) as exc:
        return f"help.health: {exc}"
    until = time.monotonic() + 30
    while time.monotonic() < until:
        if "settings" in app_windows(agent):
            return None
        time.sleep(0.2)
    return f"no Settings window within 30 s (Helios lists {app_windows(agent)})"


def import_window(args, comp, agent, keep: bool) -> str | None:
    """The import window opened on the fixtures (as an import does before
    the Settings are opened), then closed unless `keep`; an error, or
    `None`."""
    try:
        agent.command("library.importWindow", {"source": str(ic.FIXTURES), "copy": False})
    except (OSError, RuntimeError, ValueError) as exc:
        return f"library.importWindow: {exc}"
    until = time.monotonic() + 30
    while "import" not in app_windows(agent):
        if time.monotonic() > until:
            return f"no import window within 30 s (Helios lists {app_windows(agent)})"
        time.sleep(0.2)
    time.sleep(args.settle)
    if not keep:
        agent.command("window.close", {"window": "import"})
        time.sleep(args.settle)
    return None


def one(args, comp, agent, way: str, index: int, baseline: Path | None) -> dict:
    name = f"{index:02d}-{way}{'-checked' if args.check else ''}"
    r: dict = {"way": way, "name": name}
    # Xwayland's windows are not in labwc's list: as without one.
    listing = args.compositor == "labwc" and args.x11 != "yes"
    # `after-import`: the import window opened and closed first;
    # `with-import`: left open beside (closed by its title bar).
    expected = ["main"]
    if way in ("after-import", "with-import"):
        keep = way == "with-import"
        why = import_window(args, comp, agent, keep)
        if why is not None:
            return {**r, "ok": False, "why": why}
        if keep:
            expected = ["main", "import"]
        way = "titlebar"
    before = ic.listed_windows(comp) if listing else set()
    why = open_settings(args, agent)
    if why is not None:
        return {**r, "ok": False, "why": why}
    match = None
    if listing:
        until = time.monotonic() + 30
        while time.monotonic() < until and match is None:
            match = ic.import_match(comp, before)
            time.sleep(0.1)
    r["opened"] = match
    r["admin_open"] = admin_state(agent)
    time.sleep(args.settle)
    if args.check:
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            agent.command("health.checkNow", {})
        time.sleep(args.settle * 2)
    r["close_buttons"] = len(closes(agent, "settings"))
    shot(comp, args.out, f"{name}-open")
    try:
        why = close(comp, agent, way, match, args.compositor)
    except (OSError, RuntimeError, ValueError) as exc:
        why = f"close: {exc}"
    if why is not None:
        r.update(ok=False, why=why)
        # Leave a clean state for the next way.
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            agent.command("admin.close", {})
        time.sleep(args.settle)
        return r
    t0 = time.monotonic()
    gone_after = None
    while time.monotonic() < t0 + 10:
        gone = (
            not ic.present(comp, match)
            if match is not None
            else "settings" not in app_windows(agent)
        )
        if gone:
            gone_after = round(time.monotonic() - t0, 2)
            break
        time.sleep(0.1)
    r["gone_after_s"] = gone_after
    # Past the paint guard's delay (2 s), and a pass or more.
    time.sleep(max(args.settle, 3.0))
    r["listed"] = listed(comp) if listing else "not available"
    r["app_windows"] = app_windows(agent)
    r["admin"] = admin_state(agent)
    screen = shot(comp, args.out, f"{name}-after")
    r["flat"] = screen is not None and png_is_flat(screen)
    if screen is not None and baseline is not None:
        share = changed_share(baseline, screen)
        r["changed_from_baseline"] = None if share is None else round(share, 3)
    problems = []
    if listing:
        ours = [w for w in r["listed"] if w.get("app_id", "").startswith("helios") or "error" in w]
        if len(ours) != len(expected):
            problems.append(f"{len(ours)} windows listed by the compositor")
    elif (r.get("changed_from_baseline") or 0) > args.changed:
        problems.append(
            f"{r['changed_from_baseline']:.0%} of the screen differs from the main window alone"
        )
    if r["app_windows"] != expected:
        problems.append(f"Helios lists {r['app_windows']}")
    if r["admin"].get("open"):
        problems.append("the Settings app is still open")
    if r["flat"]:
        problems.append("the screen is one flat colour")
    r["ok"] = not problems
    if problems:
        r["why"] = "; ".join(problems)
    if "import" in r["app_windows"]:
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            agent.command("window.close", {"window": "import"})
        time.sleep(args.settle)
    # The Settings app closed whatever happened, for the next way.
    if r["admin"].get("open"):
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            agent.command("admin.close", {})
        time.sleep(args.settle)
    return r


def finish_wizard(args, comp, agent) -> None:
    """The first-run wizard, a focus screen on a library without its
    folder: finished on its defaults."""
    for _ in range(3):
        if not admin_state(agent).get("open"):
            return
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            agent.ui({"action": "click", "window": "main", "role": "Button", "label": "Finish"})
        time.sleep(args.settle)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--compositor", default="labwc")
    parser.add_argument("--ways", default="+".join(WAYS))
    parser.add_argument("--check", default="no", help="yes: run the health check first")
    parser.add_argument("--settle", type=float, default=2.0)
    parser.add_argument("--x11", default="no", help="yes: helios-gui on X11 (Xwayland)")
    parser.add_argument(
        "--changed",
        type=float,
        default=0.25,
        help="without a window list (mutter): the share of the screen that may differ "
        "from the main window alone after a close",
    )
    parser.add_argument("--work", type=Path, default=Path("/tmp/settings-close"))
    args = parser.parse_args()
    args.check = args.check == "yes"
    args.out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    w = ic.world(args.work)
    comp = build(f"helios-settings-close-{args.compositor}", args.compositor)
    if hasattr(comp, "set_log_dir"):
        comp.set_log_dir(args.out / "wbox-log")
    results: list[dict] = []
    try:
        cmd = [
            "sh",
            "-c",
            'exec "$0" "$@" 2>"$HELIOS_STDERR"',
            str(args.gui),
            "--vault",
            str(w["vault"]),
            str(w["workspace"]),
        ]
        env = {
            "XDG_CONFIG_HOME": str(args.work / "config"),
            "XDG_STATE_HOME": str(w["state"]),
            "XDG_CACHE_HOME": str(args.work / "cache"),
            "EGUI_POP_AGENT_SOCKET": str(w["socket"]),
            "HELIOS_AGENT_WRITE": "1",
            "HELIOS_STDERR": str(args.out / "helios.stderr"),
            "RUST_LOG": "info,egui_pop=debug",
        }
        if args.x11 == "yes":
            # winit picks X11 when no Wayland display is named.
            env["WAYLAND_DISPLAY"] = ""
            env["WINIT_UNIX_BACKEND"] = "x11"
        if "error" in comp.launch(cmd, env):
            results = [{"way": "launch", "ok": False, "why": "launch failed"}]
        else:
            proc = comp.state.app_proc
            agent = ic.Agent(w["socket"])
            if not ic.wait_socket(agent, lambda: proc is not None and proc.poll() is None, 90):
                results = [{"way": "launch", "ok": False, "why": "no agent socket"}]
            elif args.compositor == "labwc" and args.x11 != "yes" and ic.wait_main(comp) is None:
                results = [{"way": "launch", "ok": False, "why": "no main window listed"}]
            else:
                time.sleep(args.settle * 2)
                finish_wizard(args, comp, agent)
                time.sleep(args.settle)
                baseline = shot(comp, args.out, "baseline")
                for i, way in enumerate(args.ways.split("+")):
                    try:
                        r = one(args, comp, agent, way, i, baseline)
                    except Exception as exc:  # reported in the table
                        r = {"way": way, "ok": False, "why": repr(exc)}
                    print(json.dumps(r), flush=True)
                    results.append(r)
                    if proc.poll() is not None:
                        results.append(
                            {"way": "exit", "ok": False, "why": f"exited {proc.returncode}"}
                        )
                        break
    finally:
        comp.stop()
        if w["state"].exists():
            shutil.copytree(w["state"], args.out / "state", dirs_exist_ok=True)
    with open(args.out / "settings-close.jsonl", "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    lines = [f"compositor: {args.compositor}", "", "| way | ok | why |", "|---|---|---|"]
    for r in results:
        lines.append(
            f"| {r.get('name', r['way'])} | {'yes' if r.get('ok') else 'NO'} | {r.get('why', '')} |"
        )
    report = "\n".join(lines) + "\n"
    (args.out / "settings-close.md").write_text(report)
    print(report)
    return 0 if results and all(r.get("ok") for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
