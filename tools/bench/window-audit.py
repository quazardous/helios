"""Check what the import window opens lands in the import window, in a
headless labwc driven by wbox: the multi-window checklist's probes
(docs/design/windows.md, "Multi-window checklist").

Run by the bench box's `window-audit` job (see docs/bench.md), with the
Python that has the wbox library:

    window-audit.py --gui helios-gui --out DIR [--folder DIR]

`helios-gui` starts in its own compositor on a throwaway library, with its
agent socket on, and the import window is opened on `--folder`. Then, in
turn, each probe acts in the import window over the agent socket and reads
both windows' accessibility trees (each OS window has its own):

- `picker`: "Choose a folder…" clicked; the in-app picker's modal (its
  heading, the request's title) must be in the import window's tree, not
  the main window's;
- `escape`: Escape pressed in the import window: the picker closes, the
  import window stays;
- `palette`: the command palette's key pressed in the import window: it
  opens there;
- `close-with-picker`: the picker opened again, the import window closed
  by the compositor: the window goes, no picker comes up over the main
  window, and the main window still answers.

One JSON line per probe in `window-audit.jsonl`, a table in
`window-audit.md`, a screenshot after each probe; a probe that fails
fails the job.
"""

from __future__ import annotations

import argparse
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

CHOOSE = "Choose a folder…"
PALETTE_KEY = "Ctrl+Shift+P"


def labelled(agent, window: str, label: str) -> int:
    """How many nodes of `window`'s tree are named `label` (0 when the
    window is not open)."""
    try:
        return agent.ui({"action": "tree", "window": window, "label": label}).get("count", 0)
    except (OSError, RuntimeError, ValueError):
        return 0


def text_inputs(agent, window: str) -> int:
    try:
        return agent.ui({"action": "tree", "window": window, "role": "TextInput"}).get("count", 0)
    except (OSError, RuntimeError, ValueError):
        return 0


def shot(comp, args, name: str) -> None:
    s = comp.screenshot(name)
    if "path" in s:
        shutil.copy(s["path"], args.out / f"{name}.png")


def settle() -> None:
    time.sleep(1.0)


def probes(args, comp, agent) -> list[dict]:
    out: list[dict] = []
    before = ic.listed_windows(comp)
    agent.command("library.importWindow", {"source": str(args.folder), "copy": False})
    deadline = time.monotonic() + 30
    title = None
    while time.monotonic() < deadline and title is None:
        title = ic.import_match(comp, before)
        time.sleep(0.1)
    if title is None:
        return [{"probe": "open", "ok": False, "error": "no import window listed"}]
    time.sleep(args.settle)

    # The picker: its heading joins the button in the import window.
    base_main, base_import = labelled(agent, "main", CHOOSE), labelled(agent, "import", CHOOSE)
    agent.ui({"action": "click", "window": "import", "role": "Button", "label": CHOOSE})
    settle()
    main, imp = labelled(agent, "main", CHOOSE), labelled(agent, "import", CHOOSE)
    picker_in = "import" if imp > base_import else "main" if main > base_main else "nowhere"
    out.append(
        {
            "probe": "picker",
            "ok": picker_in == "import",
            "seen": f"in the {picker_in} window",
            "counts": {"main": [base_main, main], "import": [base_import, imp]},
        }
    )
    shot(comp, args, "picker")

    # Escape in the import window: the picker only.
    try:
        agent.ui({"action": "key", "window": "import", "key": "Escape"})
    except (OSError, RuntimeError, ValueError) as exc:
        out.append({"probe": "escape", "ok": False, "error": str(exc)})
    settle()
    still = ic.present(comp, title)
    gone = labelled(agent, "import", CHOOSE) == base_import and (
        labelled(agent, "main", CHOOSE) == base_main
    )
    out.append(
        {
            "probe": "escape",
            "ok": still and gone,
            "seen": f"picker {'closed' if gone else 'still open'}, "
            f"import window {'open' if still else 'closed'}",
        }
    )
    shot(comp, args, "escape")

    # The palette's key in the import window.
    inputs = (text_inputs(agent, "main"), text_inputs(agent, "import"))
    try:
        agent.ui({"action": "key", "window": "import", "key": PALETTE_KEY})
    except (OSError, RuntimeError, ValueError) as exc:
        out.append({"probe": "palette", "ok": False, "error": str(exc)})
    settle()
    after = (text_inputs(agent, "main"), text_inputs(agent, "import"))
    where = "import" if after[1] > inputs[1] else "main" if after[0] > inputs[0] else "nowhere"
    out.append(
        {
            "probe": "palette",
            "ok": where == "import",
            "seen": f"in the {where} window",
            "counts": {"main": [inputs[0], after[0]], "import": [inputs[1], after[1]]},
        }
    )
    shot(comp, args, "palette")
    try:
        agent.ui({"action": "key", "window": where if where != "nowhere" else "import", "key": "Escape"})
    except (OSError, RuntimeError, ValueError):
        pass
    settle()

    # The picker open, the window closed by the compositor.
    try:
        agent.ui({"action": "click", "window": "import", "role": "Button", "label": CHOOSE})
    except (OSError, RuntimeError, ValueError) as exc:
        out.append({"probe": "close-with-picker", "ok": False, "error": str(exc)})
        return out
    settle()
    subprocess.run(
        ["wlrctl", "toplevel", "close", title],
        env=comp._wl_env(),
        check=False,
        capture_output=True,
        timeout=5,
    )
    until = time.monotonic() + 10
    while time.monotonic() < until and ic.present(comp, title):
        time.sleep(0.1)
    closed = not ic.present(comp, title)
    settle()
    try:
        agent.request("describe", {}, within=10)
        answers = True
    except (OSError, RuntimeError, ValueError):
        answers = False
    over_main = labelled(agent, "main", CHOOSE) > base_main
    out.append(
        {
            "probe": "close-with-picker",
            "ok": closed and answers and not over_main,
            "seen": f"window {'gone' if closed else 'still listed'}, "
            f"picker {'over the main window' if over_main else 'gone'}, "
            f"main window {'answers' if answers else 'silent'}",
        }
    )
    shot(comp, args, "close-with-picker")
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--folder", type=Path, default=ic.FIXTURES)
    parser.add_argument("--settle", type=float, default=3.0)
    parser.add_argument("--work", type=Path, default=Path("/tmp/window-audit"))
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    w = ic.world(args.work)
    comp = build_compositor(ic.config("helios-window-audit"))
    if hasattr(comp, "set_log_dir"):
        comp.set_log_dir(args.out / "wbox-log")
    results: list[dict] = []
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
            "XDG_CONFIG_HOME": str(args.work / "config"),
            "XDG_STATE_HOME": str(w["state"]),
            "XDG_CACHE_HOME": str(args.work / "cache"),
            "EGUI_POP_AGENT_SOCKET": str(w["socket"]),
            "HELIOS_AGENT_WRITE": "1",
            "HELIOS_TRACE_FILE": str(args.out / "helios.stderr"),
        }
        if "error" in comp.launch(cmd, env):
            results = [{"probe": "launch", "ok": False, "error": "launch failed"}]
        else:
            proc = comp.state.app_proc
            agent = ic.Agent(w["socket"])
            if not ic.wait_socket(agent, lambda: proc is not None and proc.poll() is None, 90):
                results = [{"probe": "launch", "ok": False, "error": "no agent socket"}]
            else:
                try:
                    results = probes(args, comp, agent)
                except Exception as exc:  # reported in the table
                    results.append({"probe": "run", "ok": False, "error": repr(exc)})
    finally:
        comp.stop()
    with open(args.out / "window-audit.jsonl", "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    lines = ["| probe | ok | seen |", "|---|---|---|"]
    for r in results:
        lines.append(f"| {r['probe']} | {'yes' if r.get('ok') else 'NO'} | {r.get('seen', r.get('error', ''))} |")
    report = "\n".join(lines) + "\n"
    (args.out / "window-audit.md").write_text(report)
    print(report)
    return 0 if results and all(r.get("ok") for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
