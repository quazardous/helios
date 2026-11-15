"""The first-user journey, run live in a headless labwc driven by wbox.

Run by the bench box's `journey` job (see docs/bench.md), with the Python
that has the wbox library:

    journey.py --gui helios-gui --runner scenarios --corpus DIR --out DIR

A new user's first steps, on a fresh profile (a HOME and XDG folders of
its own, nothing answered beforehand) and fixtures only:

1. `first-run`: helios-gui starts with no argument; the init wizard is
   finished with its defaults.
2. `import`: a fixture card (two RAW+JPEG pairs, a raw alone and a JPEG
   alone: CC0 raws of the raw corpus, `--corpus`, and the plugin tests'
   JPEG fixtures) is imported through the import window; the Library
   holds one photo per pair.
3. `quit`: helios-gui is closed by its `workbench.quit` command and must
   exit with status 0.
4. `restart`: helios-gui starts again on the same profile; no wizard, the
   Library holds the same photos.

Each step is a scenario of `tools/bench/journey/` replayed over the
agent socket by the `scenarios` example (egui-pop's scenario runner), its
invariants checked after every step (no error toast, no failed command,
no panic). After every step the journey also reads helios-gui's stderr
and log files: a line at error level or a panic fails the step.

In `--out`: the shots, `journey.jsonl` (one line per step), and on a
failure `journey-failure.txt` naming the step, with the screen at that
moment (`<step>-screen.png`) and helios-gui's logs (`logs/`). The exit
status is 1 when a step failed.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

from wbox.server import build_compositor

HERE = Path(__file__).resolve().parent
JPEGS = HERE.parents[1] / "crates/helios-plugin/tests/fixtures/rated-photos"

# CC0 raws of the raw corpus (tests/raw-corpus.toml), small and of three
# makers; the volume names each `<id>.<ext>`.
RAWS = (
    "nikon-d70-nef-12bit-12bit-compressed-lossy-type-1-3-2",
    "pentax-k-r-pef-12bit-compressed-3-2",
    "panasonic-dmc-gf1-rw2-16-9",
)

# A log line at error level, or a panic.
BAD_LINE = re.compile(r"\bERROR\b|\[error\]|\berror:|panicked at", re.IGNORECASE)


def scenarios_module():
    """tools/bench/scenarios.py: its compositor settings and its quit."""
    spec = importlib.util.spec_from_file_location("scenarios", HERE / "scenarios.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fixture_card(corpus: Path, card: Path) -> dict:
    """A card in DCIM layout: two RAW+JPEG pairs, a raw and a JPEG alone."""
    folder = card / "DCIM" / "100HELIO"
    folder.mkdir(parents=True)
    raws = []
    for raw_id in RAWS:
        found = sorted(corpus.glob(f"{raw_id}.*"))
        if not found:
            raise SystemExit(f"no raw {raw_id}.* in {corpus}")
        raws.append(found[0])
    jpegs = sorted(JPEGS.glob("*.jpg"))
    if len(jpegs) < 3:
        raise SystemExit(f"three JPEG fixtures expected in {JPEGS}")
    names = []
    # The pairs: a raw and a JPEG of one name, as a camera writes them.
    for n, (raw, jpeg) in enumerate(zip(raws[:2], jpegs[:2], strict=True), start=1):
        stem = f"IMG_{n:04d}"
        shutil.copy(raw, folder / f"{stem}{raw.suffix.upper()}")
        shutil.copy(jpeg, folder / f"{stem}.JPG")
        names += [f"{stem}{raw.suffix.upper()}", f"{stem}.JPG"]
    shutil.copy(raws[2], folder / f"IMG_0003{raws[2].suffix.upper()}")
    shutil.copy(jpegs[2], folder / "IMG_0004.JPG")
    names += [f"IMG_0003{raws[2].suffix.upper()}", "IMG_0004.JPG"]
    return {"files": names, "photos": 4}


def profile(work: Path) -> dict[str, str]:
    """A fresh HOME and XDG folders: nothing answered, no library yet."""
    shutil.rmtree(work, ignore_errors=True)
    dirs = {}
    for name in ("home", "config", "data", "state", "cache", "agent"):
        (work / name).mkdir(parents=True)
        dirs[name] = work / name
    (dirs["home"] / "Pictures").mkdir()
    os.chmod(dirs["agent"], 0o700)
    return {
        "HOME": str(dirs["home"]),
        "XDG_CONFIG_HOME": str(dirs["config"]),
        "XDG_DATA_HOME": str(dirs["data"]),
        "XDG_STATE_HOME": str(dirs["state"]),
        "XDG_CACHE_HOME": str(dirs["cache"]),
        "XDG_PICTURES_DIR": str(dirs["home"] / "Pictures"),
        "EGUI_POP_AGENT_SOCKET": str(dirs["agent"] / "helios.sock"),
        "HELIOS_AGENT_WRITE": "1",
    }


def bad_lines(files: list[Path], seen: dict[Path, int]) -> list[str]:
    """Error-level and panic lines written to `files` since `seen`."""
    found = []
    for f in files:
        try:
            lines = f.read_text(errors="replace").splitlines()
        except OSError:
            continue
        start = seen.get(f, 0)
        seen[f] = len(lines)
        found += [f"{f.name}: {line}" for line in lines[start:] if BAD_LINE.search(line)]
    return found


class Journey:
    def __init__(self, args):
        self.args = args
        self.mod = scenarios_module()
        self.work = args.work
        self.env = profile(self.work / "profile")
        self.card = fixture_card(args.corpus, self.work / "card")
        self.stderr = args.out / "logs"
        self.stderr.mkdir(parents=True, exist_ok=True)
        self.seen: dict[Path, int] = {}
        self.comp = None
        self.launches = 0
        self.steps: list[dict] = []

    def log_files(self) -> list[Path]:
        state = Path(self.env["XDG_STATE_HOME"])
        files = sorted(self.stderr.glob("stderr-*.txt"))
        return files + sorted(
            p for p in state.rglob("*") if p.is_file() and p.suffix in (".log", ".txt")
        )

    def launch(self) -> dict:
        self.launches += 1
        self.comp = build_compositor(
            self.mod.config(f"helios-journey-{self.launches}", self.args.screen)
        )
        self.comp.state.screenshot_dir = self.work
        sock = Path(self.env["EGUI_POP_AGENT_SOCKET"])
        sock.unlink(missing_ok=True)
        cmd = ["sh", "-c", 'exec "$0" "$@" 2>"$HELIOS_JOURNEY_STDERR"', str(self.args.gui)]
        env = {
            **self.env,
            "HELIOS_JOURNEY_STDERR": str(self.stderr / f"stderr-{self.launches}.txt"),
        }
        return self.comp.launch(cmd, env)

    def stop(self) -> None:
        if self.comp is not None:
            self.comp.stop()
            self.comp = None

    def screen(self, step: str) -> None:
        if self.comp is None:
            return
        shot = self.comp.screenshot(f"journey-{step}-screen")
        if "path" in shot:
            shutil.copy(shot["path"], self.args.out / f"{step}-screen.png")

    def scenario(self, step: str, file: str) -> dict:
        runner = [
            str(self.args.runner),
            "--socket",
            self.env["EGUI_POP_AGENT_SOCKET"],
            "--out",
            str(self.args.out),
            "--var",
            f"card={self.work / 'card'}",
            "--var",
            f"photos={self.card['photos']}",
            "--shot",
            "gui.snapshot",
            json.dumps({"target": "window", "path": "$path"}),
            "--deadline-ms",
            str(self.args.deadline_ms),
            "--connect-ms",
            "90000",
            str(HERE / "journey" / file),
        ]
        entry: dict = {"step": step}
        try:
            done = subprocess.run(
                runner, capture_output=True, text=True, timeout=self.args.timeout, check=False
            )
            entry["rc"] = done.returncode
            if done.returncode != 0:
                entry["why"] = done.stderr.strip() or done.stdout.strip() or "the runner failed"
        except subprocess.TimeoutExpired:
            entry["rc"] = None
            entry["why"] = f"the scenario did not end within {self.args.timeout} s"
        proc = self.comp.state.app_proc
        if proc is None or proc.poll() is not None:
            entry.setdefault("why", "helios-gui is gone")
            entry["exit"] = None if proc is None else proc.returncode
        return entry

    def quit(self) -> dict:
        proc = self.comp.state.app_proc
        how = self.mod.quit_app(Path(self.env["EGUI_POP_AGENT_SOCKET"]), proc, within=20.0)
        entry: dict = {"step": "quit", "how": how}
        code = None if proc is None else proc.poll()
        entry["exit"] = code
        if how != "clean":
            entry["why"] = f"helios-gui did not quit by itself: {how}"
        elif code != 0:
            entry["why"] = f"helios-gui exited with status {code}"
        return entry

    def check(self, entry: dict) -> bool:
        """The step's logs read; true when the step passed."""
        bad = bad_lines(self.log_files(), self.seen)
        if bad:
            entry["log_lines"] = bad[:40]
            entry.setdefault("why", "error or panic lines in helios-gui's logs")
        entry["passed"] = "why" not in entry
        self.steps.append(entry)
        print(json.dumps(entry, default=str), flush=True)
        return entry["passed"]

    def fail(self, entry: dict) -> None:
        self.screen(entry["step"])
        lines = [
            f"journey failed at step `{entry['step']}`",
            f"why: {entry.get('why')}",
        ]
        for line in entry.get("log_lines", []):
            lines.append(f"  {line}")
        lines.append(f"screen: {entry['step']}-screen.png; logs: logs/")
        (self.args.out / "journey-failure.txt").write_text("\n".join(lines) + "\n")
        print("\n".join(lines), file=sys.stderr)

    def run(self) -> bool:
        plan = [
            ("launch", None),
            ("first-run", "1-first-run.ron"),
            ("import", "2-import.ron"),
            ("quit", None),
            ("relaunch", None),
            ("restart", "3-restart.ron"),
            ("quit-again", None),
        ]
        for step, file in plan:
            started = time.monotonic()
            if step in ("launch", "relaunch"):
                launched = self.launch()
                entry = {"step": step}
                if "error" in launched:
                    entry["why"] = f"launch: {launched['error']}"
            elif file is not None:
                entry = self.scenario(step, file)
            else:
                entry = self.quit()
                entry["step"] = step
            entry["seconds"] = round(time.monotonic() - started, 1)
            if step.startswith("quit"):
                # The screen before the compositor goes, kept on success too.
                self.stop()
            if not self.check(entry):
                self.fail(entry)
                return False
        return True


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui", type=Path, required=True)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--work", type=Path, default=Path("/tmp/helios-journey"))
    parser.add_argument("--screen", default="1280x800")
    parser.add_argument("--deadline-ms", type=int, default=60000)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("XDG_RUNTIME_DIR", "/tmp/xdg")
    started = time.monotonic()
    journey = Journey(args)
    try:
        passed = journey.run()
    finally:
        journey.stop()
        state = Path(journey.env["XDG_STATE_HOME"])
        if state.exists():
            shutil.copytree(state, args.out / "logs" / "state", dirs_exist_ok=True)
        with open(args.out / "journey.jsonl", "w") as f:
            for entry in journey.steps:
                f.write(json.dumps(entry, default=str) + "\n")
    print(f"journey {'passed' if passed else 'failed'} in {time.monotonic() - started:.0f} s")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
