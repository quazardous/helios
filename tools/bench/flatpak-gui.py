"""Run the development Flatpak as a user runs it, in a headless labwc
driven by wbox, and check it works with only its manifest's permissions.

Run by the bench box's `flatpak-devel-gui` job (see docs/bench.md), with
the Python that has the wbox library:

    flatpak-gui.py --bundle helios-devel.flatpak --out DIR

The bundle is installed for the user (its runtime already installed by
the `flatpak` jobs), then started with `flatpak run` and no permission
added: its settings, library and agent socket live in its own data folder
(`~/.var/app/<id>`), which the app can write and this script can reach.
The steps, each a JSON line in `flatpak-gui.jsonl`, a table in
`flatpak-gui.md` and a screenshot:

- `start`: the agent socket answers and the main window is listed;
- `library`: the library collector answers (a library is open, empty);
- `import-window`: the import window opens on a folder of `~/Pictures`
  (reached through `--filesystem=xdg-pictures`, nothing granted here)
  and lists its photos by name;
- `import`: its Import button imports them: the library counts them, and
  the jobs collector shows the import job finished, which runs through
  helios-node, started by helios-gui inside the sandbox.

The app is uninstalled at the end, its data folder removed.
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
from pathlib import Path

from wbox.server import build_compositor

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("import_close", HERE / "import-close.py")
ic = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ic)

APP = "io.github.quazardous.Helios.Devel"
PHOTOS = ("a.jpg", "b.jpg", "c.jpg")


def flatpak(*args: str, timeout: float = 300) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["flatpak", *args], capture_output=True, text=True, timeout=timeout, check=False
    )


def shot(comp, out: Path, name: str) -> None:
    s = comp.screenshot(name)
    if "path" in s:
        shutil.copy(s["path"], out / f"{name}.png")


def snapshot(agent, collector: str) -> dict:
    try:
        return agent.request("snapshot", {"collector": collector}) or {}
    except (OSError, RuntimeError, ValueError):
        return {}


def names_in(agent, window: str) -> set[str]:
    """Every label and value of `window`'s tree."""
    try:
        nodes = agent.ui({"action": "tree", "window": window}).get("nodes", [])
    except (OSError, RuntimeError, ValueError):
        return set()
    found = set()
    for n in nodes:
        for key in ("label", "value"):
            if isinstance(n.get(key), str):
                found.add(n[key])
    return found


def until(check, within: float, every: float = 0.25):
    """`check()` once truthy, or its last value after `within` seconds."""
    deadline = time.monotonic() + within
    while True:
        value = check()
        if value or time.monotonic() >= deadline:
            return value
        time.sleep(every)


def world(home: Path) -> dict:
    """The app's own folders, its first-run questions answered, and a
    folder of photos in Pictures, declared as the desktop declares it."""
    data = home / ".var" / "app" / APP
    shutil.rmtree(data, ignore_errors=True)
    ui = data / "config" / "helios" / "ui"
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
    (data / "data" / "workspace").mkdir(parents=True)
    (data / "cache").mkdir(parents=True)
    (home / ".config").mkdir(parents=True, exist_ok=True)
    (home / ".config" / "user-dirs.dirs").write_text('XDG_PICTURES_DIR="$HOME/Pictures"\n')
    photos = home / "Pictures" / "flatpak-gui"
    shutil.rmtree(photos, ignore_errors=True)
    photos.mkdir(parents=True)
    for p in PHOTOS:
        shutil.copy(ic.FIXTURES / p, photos / p)
    return {
        "data": data,
        "vault": data / "data" / "library.db",
        "workspace": data / "data" / "workspace",
        "socket": data / "cache" / "agent.sock",
        "photos": photos,
    }


def steps(args, comp, agent, w) -> list[dict]:
    out: list[dict] = []

    main = ic.wait_main(comp, 60)
    out.append(
        {
            "step": "start",
            "ok": main is not None,
            "seen": f"main window {main.get('title')!r}" if main else "no main window listed",
        }
    )
    shot(comp, args.out, "start")
    if main is None:
        return out

    lib = until(lambda: snapshot(agent, "helios.library"), 30)
    out.append(
        {
            "step": "library",
            "ok": bool(lib) and lib.get("count") == 0,
            "seen": json.dumps({k: lib.get(k) for k in ("count", "selected")})
            if lib
            else "no answer",
        }
    )

    before = ic.listed_windows(comp)
    agent.command("library.importWindow", {"source": str(w["photos"]), "copy": False})
    title = until(lambda: ic.import_match(comp, before), 30, 0.1)
    if title is None:
        out.append({"step": "import-window", "ok": False, "seen": "no import window listed"})
        return out
    names = until(
        lambda: (lambda s: s if all(any(p in n for n in s) for p in PHOTOS) else None)(
            names_in(agent, "import")
        ),
        30,
    )
    shot(comp, args.out, "import-window")
    listed = sorted(p for p in PHOTOS if names and any(p in n for n in names))
    out.append(
        {
            "step": "import-window",
            "ok": len(listed) == len(PHOTOS),
            "seen": f"{title}; lists {listed}",
        }
    )
    if not names:
        with open(args.out / "import-tree.json", "w") as f:
            json.dump(sorted(names_in(agent, "import")), f, indent=1)
        return out

    try:
        ic.close(comp, agent, "import", title)
    except (OSError, RuntimeError, ValueError) as exc:
        out.append({"step": "import", "ok": False, "seen": f"Import button: {exc}"})
        return out
    lib = until(
        lambda: (lambda s: s if s.get("count") == len(PHOTOS) else None)(
            snapshot(agent, "helios.library")
        ),
        90,
    ) or snapshot(agent, "helios.library")
    jobs = snapshot(agent, "helios.jobs")
    shot(comp, args.out, "imported")
    out.append(
        {
            "step": "import",
            "ok": lib.get("count") == len(PHOTOS) and not jobs.get("failed"),
            "seen": f"library count {lib.get('count')}; jobs "
            + json.dumps({k: jobs.get(k) for k in ("total", "finished", "failed", "running")}),
        }
    )
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    home = Path(os.environ["HOME"])
    results: list[dict] = []

    got = flatpak(
        # the box is offline: its runtime is installed by the flatpak jobs
        "install",
        "--user",
        "-y",
        "--noninteractive",
        "--reinstall",
        "--no-deps",
        str(args.bundle),
    )
    (args.out / "install.log").write_text(got.stdout + got.stderr)
    if got.returncode != 0:
        results.append({"step": "install", "ok": False, "seen": got.stderr.strip()[-300:]})
    else:
        info = flatpak("info", "--user", "--show-permissions", APP)
        (args.out / "permissions.txt").write_text(info.stdout + info.stderr)
        w = world(home)
        comp = build_compositor(ic.config("helios-flatpak-gui"))
        if hasattr(comp, "set_log_dir"):
            comp.set_log_dir(args.out / "wbox-log")
        try:
            # No permission added: the agent socket and the library are in
            # the app's own data folder.
            cmd = [
                "sh",
                "-c",
                'exec flatpak run --user --env=EGUI_POP_AGENT_SOCKET="$1" --env=HELIOS_AGENT_PRESET=scenario+private '
                '"$0" --vault "$2" "$3" 2>"$4"',
                APP,
                str(w["socket"]),
                str(w["vault"]),
                str(w["workspace"]),
                str(args.out / "helios.stderr"),
            ]
            if "error" in comp.launch(cmd, {}):
                results.append({"step": "start", "ok": False, "seen": "launch failed"})
            else:
                proc = comp.state.app_proc
                agent = ic.Agent(w["socket"])
                if not ic.wait_socket(agent, lambda: proc is not None and proc.poll() is None, 120):
                    results.append({"step": "start", "ok": False, "seen": "no agent socket"})
                    shot(comp, args.out, "start")
                else:
                    try:
                        results = steps(args, comp, agent, w)
                    except Exception as exc:  # reported in the table
                        results.append({"step": "run", "ok": False, "seen": repr(exc)})
        finally:
            comp.stop()
            ps = flatpak("ps")
            (args.out / "flatpak-ps.txt").write_text(ps.stdout + ps.stderr)
            state = w["data"] / "state"
            if state.exists():
                shutil.copytree(state, args.out / "state", dirs_exist_ok=True)
            with contextlib.suppress(OSError, subprocess.TimeoutExpired):
                flatpak("kill", APP, timeout=30)
                flatpak("uninstall", "--user", "-y", "--noninteractive", APP, timeout=120)
            shutil.rmtree(w["data"], ignore_errors=True)

    with open(args.out / "flatpak-gui.jsonl", "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    lines = ["| step | ok | seen |", "|---|---|---|"]
    for r in results:
        lines.append(f"| {r['step']} | {'yes' if r.get('ok') else 'NO'} | {r.get('seen', '')} |")
    report = "\n".join(lines) + "\n"
    (args.out / "flatpak-gui.md").write_text(report)
    print(report)
    return 0 if results and all(r.get("ok") for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
