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
3. `imports-list`: the Library's Imports section opened on that import.
4. `import-session`: a click on the import's line (read from the tree)
   scopes the Library to its photos, without an error.
5. `health`: the health check opened and run now; its findings read.
6. `health-fix-<finding>`: each fix offered applied (none offered on a
   healthy new library), then the main window must still draw, and the
   screen must not be black.
6b. `health-close`: Settings › Health check opened in its own window,
   then closed by its title bar's close button: the compositor and
   Helios list the windows open before (no window left behind), and the
   screen is not one flat colour.
7. `storage`: Settings › Storage, then Backup › Strategy; the library's
   disk, when listed, must be online with the Library role, and the
   strategy must have its line.
8. `quit`: helios-gui is closed by its `workbench.quit` command and must
   exit with status 0.
9. `restart`: helios-gui starts again on the same profile; no wizard, the
   Library holds the same photos.
10. `workshop`: the first RAW+JPEG pair opened in the darkroom; its
    render ends (the viewport has an image size) and the image region is
    not one flat colour.
11. `export`: that photo exported with the default settings to a folder
    of the profile; the file is written, decodes, and has the raw's size
    (its crop in the raw corpus's manifest).
12. `quit-again`: closed as before, exit status 0.

Each step is a scenario of `tools/bench/journey/` replayed over the
agent socket by the `scenarios` example (egui-pop's scenario runner), its
invariants checked after every step (no error toast, no failed command,
no panic). After every step the journey also reads helios-gui's stderr
and log files: a line at error level or a panic fails the step.

In `--out`: the shots, `journey.jsonl` (one line per step), and on a
failure `journey-failure.txt` naming the step, with the screen at that
moment (`<step>-screen.png`), each window's accessibility tree
(`<step>-tree-<window>.json`) and helios-gui's logs (`logs/`). The exit
status is 1 when a step failed.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import zlib
from pathlib import Path

from wbox.server import build_compositor

HERE = Path(__file__).resolve().parent
JPEGS = HERE.parents[1] / "crates/helios-plugin/tests/fixtures/rated-photos"
CORPUS_MANIFEST = HERE.parents[1] / "tests/raw-corpus.toml"

# CC0 raws of the raw corpus (tests/raw-corpus.toml), small and of three
# makers; the volume names each `<id>.<ext>`.
RAWS = (
    "nikon-d70-nef-12bit-12bit-compressed-lossy-type-1-3-2",
    "pentax-k-r-pef-12bit-compressed-3-2",
    "panasonic-dmc-gf1-rw2-16-9",
)

# The import's line in the Library's Imports section: "<when> · <source> · <count>".
SESSION_LINE = re.compile(r"^\d{4}-\d\d-\d\d \d\d:\d\d · (.+) · (\d+)$")

# Fixes that leave Helios (a browser, the file manager): not applied.
OUTSIDE = ("links.open", "library.openFolder")

# A log line at error level, or a panic: the level as loggers write it
# (`ERROR`, `[error]`, a leading `error:`), never the word in a message (an
# INFO line quoting a refusal's `"error"`).
BAD_LINE = re.compile(r"\bERROR\b|\[(?i:error)\]|^(?i:error):|panicked at")


def raw_crop(raw_id: str) -> tuple[int, int]:
    """A corpus raw's developed size: its crop (`3008x2000+15+7`) in the
    raw corpus's manifest."""
    # Read by its lines: the box's Python may predate `tomllib`.
    entries = CORPUS_MANIFEST.read_text().split("[[file]]")
    for entry in entries:
        if f'id = "{raw_id}"' in entry:
            crop = re.search(r'^crop = "(\d+)x(\d+)', entry, re.MULTILINE)
            if crop:
                return int(crop.group(1)), int(crop.group(2))
    raise SystemExit(f"no crop for {raw_id} in {CORPUS_MANIFEST}")


def image_size(path: Path) -> tuple[int, int]:
    """An exported image's size, its file read to the end (a JPEG's frame
    and its end marker, a PNG's rows, a TIFF's first page); raises
    `ValueError` when it does not decode."""
    data = path.read_bytes()
    if data[:2] == b"\xff\xd8":
        return jpeg_size(data)
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        return png_size(data)
    if data[:4] in (b"II*\x00", b"MM\x00*"):
        return tiff_size(data)
    raise ValueError(f"not a JPEG, PNG or TIFF: starts with {data[:8]!r}")


def jpeg_size(data: bytes) -> tuple[int, int]:
    pos, size = 2, None
    while pos + 4 <= len(data):
        if data[pos] != 0xFF:
            raise ValueError(f"no marker at {pos}")
        marker = data[pos + 1]
        length = int.from_bytes(data[pos + 2 : pos + 4], "big")
        if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
            h = int.from_bytes(data[pos + 5 : pos + 7], "big")
            w = int.from_bytes(data[pos + 7 : pos + 9], "big")
            size = (w, h)
        if marker == 0xDA:
            break
        pos += 2 + length
    if size is None:
        raise ValueError("a JPEG with no frame header")
    if not data.rstrip(b"\x00").endswith(b"\xff\xd9"):
        raise ValueError("a JPEG cut short (no end marker)")
    return size


def png_size(data: bytes) -> tuple[int, int]:
    w = int.from_bytes(data[16:20], "big")
    h = int.from_bytes(data[20:24], "big")
    bits, colour = data[24], data[25]
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[colour]
    pos, idat = 8, b""
    while pos < len(data):
        size = int.from_bytes(data[pos : pos + 4], "big")
        if data[pos + 4 : pos + 8] == b"IDAT":
            idat += data[pos + 8 : pos + 8 + size]
        pos += size + 12
    rows = len(zlib.decompress(idat))
    if rows != h * (1 + (w * channels * bits + 7) // 8):
        raise ValueError(f"a PNG of {w}x{h} with {rows} bytes of rows")
    return w, h


def tiff_size(data: bytes) -> tuple[int, int]:
    order = "little" if data[:2] == b"II" else "big"
    ifd = int.from_bytes(data[4:8], order)
    count = int.from_bytes(data[ifd : ifd + 2], order)
    tags = {}
    for i in range(count):
        at = ifd + 2 + 12 * i
        tag = int.from_bytes(data[at : at + 2], order)
        kind = int.from_bytes(data[at + 2 : at + 4], order)
        value = data[at + 8 : at + 12]
        tags[tag] = int.from_bytes(value[:2] if kind == 3 else value, order)
    if 256 not in tags or 257 not in tags:
        raise ValueError("a TIFF with no width or height")
    return tags[256], tags[257]


def same_size(got: tuple[int, int], want: tuple[int, int]) -> bool:
    """The raw's size within 1 % a side (a demosaic's border), either way
    up (the camera's orientation applied)."""
    a, b = sorted(got), sorted(want)
    return all(abs(x - y) <= max(2, y // 100) for x, y in zip(a, b, strict=True))


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


def labels(tree) -> list[str]:
    """Every node's label in a `ui` tree answer, whatever its nesting."""
    found = []
    if isinstance(tree, dict):
        if isinstance(tree.get("label"), str):
            found.append(tree["label"])
        for v in tree.values():
            if isinstance(v, (dict, list)):
                found += labels(v)
    elif isinstance(tree, list):
        for v in tree:
            found += labels(v)
    return found


def png_is_flat(path: Path) -> bool:
    """Whether a PNG is one flat colour (a black window): its pixel bytes,
    filters and all, hold almost no distinct values, where any drawn
    interface (text, edges) holds many."""
    data = path.read_bytes()
    pos, idat = 8, b""
    while pos < len(data):
        size = int.from_bytes(data[pos : pos + 4], "big")
        kind = data[pos + 4 : pos + 8]
        if kind == b"IDAT":
            idat += data[pos + 8 : pos + 8 + size]
        pos += size + 12
    return len(set(zlib.decompress(idat))) < 16


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
        self.session = ""
        self.photo = ""
        self.image = ""
        self.export = self.work / "profile" / "export"

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

    def rpc(self, method: str, params: dict, within: float = 30.0):
        """One request on the agent socket: its result, or raises."""
        request = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(within)
            s.connect(self.env["EGUI_POP_AGENT_SOCKET"])
            s.sendall((json.dumps(request) + "\n").encode())
            buf = b""
            while not buf.endswith(b"\n"):
                chunk = s.recv(1 << 16)
                if not chunk:
                    break
                buf += chunk
        answer = json.loads(buf)
        if "error" in answer:
            raise RuntimeError(f"{method}: {answer['error']}")
        return answer.get("result")

    def journal(self, action: str, args: dict):
        result = self.rpc("call", {"collector": "journal", "action": action, "args": args})
        return result.get("data") if isinstance(result, dict) else result

    def command(self, name: str, args: dict | None = None, within: float = 30.0):
        """A command's answer, run as the agent: `commands.run` only queues
        it, its answer is the journal's `command.done` (or `command.failed`)."""
        snapshot = self.rpc("snapshot", {"collector": "journal"})
        snapshot = snapshot.get("journal", snapshot) if isinstance(snapshot, dict) else {}
        seq = snapshot.get("last", 0) if isinstance(snapshot, dict) else 0
        result = self.rpc(
            "call",
            {"collector": "commands", "action": "run", "args": {"id": name, "args": args or {}}},
        )
        if isinstance(result, dict) and result.get("ok") is False:
            raise RuntimeError(f"{name}: {result.get('message')}")
        end = time.monotonic() + within
        while time.monotonic() < end:
            for event in self.journal("since", {"seq": seq}) or []:
                seq = max(seq, event.get("seq", seq))
                data = event.get("data") or {}
                kind = event.get("name") or event.get("event")
                if data.get("command") != name:
                    continue
                if kind == "command.done":
                    return data.get("result")
                if kind == "command.failed":
                    raise RuntimeError(f"{name} failed: {data}")
            time.sleep(0.3)
        raise RuntimeError(f"{name}: no answer within {within} s")

    def windows(self) -> list[str]:
        try:
            described = self.rpc("describe", {})
            return [w["id"] for w in described.get("windows", [])] or ["main"]
        except (OSError, RuntimeError, ValueError, KeyError, TypeError):
            return ["main"]

    def trees(self, step: str) -> None:
        """Each window's accessibility tree, for the failure report."""
        if self.comp is None:
            return
        for window in self.windows():
            try:
                tree = self.rpc("ui", {"action": "tree", "window": window})
            except (OSError, RuntimeError, ValueError) as exc:
                tree = {"error": str(exc)}
            (self.args.out / f"{step}-tree-{window}.json").write_text(json.dumps(tree, indent=1))

    def scenario(self, step: str, file: str, extra: dict | None = None) -> dict:
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
            *[a for k, v in (extra or {}).items() for a in ("--var", f"{k}={v}")],
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
        self.trees(entry["step"])
        lines = [
            f"journey failed at step `{entry['step']}`",
            f"why: {entry.get('why')}",
        ]
        for line in entry.get("log_lines", []):
            lines.append(f"  {line}")
        lines.append(f"screen: {entry['step']}-screen.png; logs: logs/")
        (self.args.out / "journey-failure.txt").write_text("\n".join(lines) + "\n")
        print("\n".join(lines), file=sys.stderr)

    def find_session(self, entry: dict) -> None:
        """The import's line in the Imports section, for `import-session`."""
        try:
            tree = self.rpc("ui", {"action": "tree", "window": "main"})
        except (OSError, RuntimeError, ValueError) as exc:
            entry["why"] = f"the tree could not be read: {exc}"
            return
        lines = [x for x in labels(tree) if SESSION_LINE.match(x)]
        entry["sessions"] = lines
        if len(lines) != 1:
            entry["why"] = f"one import line expected in the Imports section, found {lines}"
            return
        # It counts photos as the Library does (a pair is one) and names
        # the card as the user picked it, not its first file.
        source, count = SESSION_LINE.match(lines[0]).groups()
        entry["session_count"] = int(count)
        self.session = lines[0]
        card = (self.work / "card").name
        if int(count) != self.card["photos"]:
            entry["why"] = (
                f"the import's line counts {count} photos, not {self.card['photos']}: {lines[0]}"
            )
        elif source != card:
            entry["why"] = f"the import's line names {source!r}, not the card {card!r}: {lines[0]}"

    def health_fixes(self) -> bool:
        """Each fix the health check offers applied, a step each."""
        started = time.monotonic()
        entry: dict = {"step": "health-findings"}
        rows = []
        try:
            # The run "Check now" started: its findings once it has ended.
            end = time.monotonic() + 90
            while True:
                answer = self.command("health.findings")
                if answer.get("ran") and not answer.get("running"):
                    break
                if time.monotonic() > end:
                    entry["why"] = f"the health check did not end within 90 s: {answer}"
                    break
                time.sleep(1)
            rows = answer.get("findings", [])
            entry["summary"] = (answer.get("summary") or {}).get("text")
        except (OSError, RuntimeError, ValueError, AttributeError) as exc:
            entry["why"] = f"health.findings: {exc}"
        entry["findings"] = [
            {k: r.get(k) for k in ("id", "level", "title", "fixCommand", "ignored")} for r in rows
        ]
        offered = [r for r in rows if r.get("fix") and not r.get("ignored")]
        entry["offered"] = [r["id"] for r in offered]
        entry["seconds"] = round(time.monotonic() - started, 1)
        if not self.check(entry):
            self.fail(entry)
            return False
        for row in offered:
            if row.get("fixCommand") in OUTSIDE:
                print(f"journey: fix of {row['id']} leaves Helios, not applied", flush=True)
                continue
            step = f"health-fix-{row['id']}"
            started = time.monotonic()
            entry = self.scenario(step, "6-health-fix.ron", {"fix": row["id"]})
            entry["fix"] = row.get("fixCommand")
            if "why" not in entry:
                self.painting(entry)
            entry["seconds"] = round(time.monotonic() - started, 1)
            if not self.check(entry):
                self.fail(entry)
                return False
        return True

    def painting(self, entry: dict) -> None:
        """The screen is not one flat colour (a black window)."""
        shot = self.comp.screenshot(f"journey-{entry['step']}-check")
        path = shot.get("path")
        if path is None:
            entry["why"] = f"no screenshot: {shot.get('error')}"
        elif png_is_flat(Path(path)):
            shutil.copy(path, self.args.out / f"{entry['step']}-flat.png")
            entry["why"] = "the screen is one flat colour after this step (a black window?)"

    def listed(self) -> list[str]:
        """The Helios windows the compositor lists, by title."""
        return sorted(
            w.get("title", "")
            for w in self.comp.list_windows().get("windows", [])
            if str(w.get("app_id", "")).startswith("helios")
        )

    def windows_left(self, entry: dict, before: tuple[list[str], list[str]]) -> None:
        """After the Settings window's close: the windows open before it
        opened (the main window, and the import window the journey left
        open), as the compositor and Helios list them, and a screen that is
        not one flat colour."""
        listed, apps = self.listed(), self.windows()
        entry["listed"], entry["app_windows"] = listed, apps
        if listed != before[0]:
            entry["why"] = f"the compositor lists {listed}, {before[0]} expected"
        elif sorted(apps) != sorted(before[1]):
            entry["why"] = f"Helios lists {apps}, {before[1]} expected"
        else:
            self.painting(entry)

    def storage_probe(self, entry: dict) -> None:
        """The volumes hold the library's disk, online; the strategy a line."""
        try:
            # The volumes are read off the frame: given a moment.
            end = time.monotonic() + 30
            while True:
                volumes = self.command("library.volumes").get("volumes", [])
                if volumes or time.monotonic() > end:
                    break
                time.sleep(1)
            strategy = self.command("library.backupStrategy")
        except (OSError, RuntimeError, ValueError, AttributeError) as exc:
            entry["why"] = f"storage commands: {exc}"
            return
        entry["volumes"] = [
            {k: v.get(k) for k in ("name", "mount", "roleText", "state")} for v in volumes
        ]
        entry["strategy"] = (strategy.get("summary") or {}).get("text")
        # The library folder is in the profile's home: the volume mounted
        # deepest above it holds it. In a container none may be listed (an
        # overlay root): then there is no disk to check.
        home = self.env["HOME"]
        holders = [
            v
            for v in volumes
            if v.get("mount") and (home + "/").startswith(v["mount"].rstrip("/") + "/")
        ]
        holder = max(holders, key=lambda v: len(v["mount"]), default=None)
        entry["library_volume"] = holder and holder.get("name")
        if not volumes:
            entry["why"] = "Storage lists no volume"
        elif holder is not None and not (holder.get("role_library") and holder.get("online")):
            entry["why"] = (
                f"the library's disk {holder.get('name')} is not online with the Library role"
            )
        elif not entry["strategy"]:
            entry["why"] = "the backup strategy has no line"

    def collector(self, name: str) -> dict:
        snapshot = self.rpc("snapshot", {"collector": name})
        if isinstance(snapshot, dict) and isinstance(snapshot.get(name), dict):
            return snapshot[name]
        return snapshot if isinstance(snapshot, dict) else {}

    def find_photo(self, entry: dict) -> None:
        """The first pair's cell in the grid, for `workshop`."""
        try:
            tree = self.rpc("ui", {"action": "tree", "window": "main"})
        except (OSError, RuntimeError, ValueError) as exc:
            entry["why"] = f"the tree could not be read: {exc}"
            return
        cells = sorted({x for x in labels(tree) if x.startswith("IMG_0001.")})
        entry["cells"] = cells
        if not cells:
            entry["why"] = "the first RAW+JPEG pair (IMG_0001) is not in the grid"
            return
        self.photo = cells[0]

    def darkroom(self, entry: dict) -> None:
        """The photo opened: its render ends, the image region is drawn."""
        try:
            end = time.monotonic() + 120
            while True:
                library = self.collector("helios.library")
                if library.get("open_size") or time.monotonic() > end:
                    break
                time.sleep(1)
        except (OSError, RuntimeError, ValueError) as exc:
            entry["why"] = f"helios.library: {exc}"
            return
        current = library.get("current") or {}
        entry["open_size"] = library.get("open_size")
        entry["image"] = {k: current.get(k) for k in ("id", "name")}
        if not library.get("open_size"):
            entry["why"] = "no render in the viewport within 120 s (no image size)"
            return
        if not str(current.get("name", "")).startswith("IMG_0001."):
            entry["why"] = f"the image open is not the pair clicked: {current}"
            return
        self.image = current["id"]
        picture = self.args.out / "workshop-viewport.png"
        # Taken from the first frame that draws the image region (the
        # darkroom may be entered a frame after the render ends).
        try:
            self.command("gui.snapshot", {"target": "viewport.display", "path": str(picture)})
        except (OSError, RuntimeError, ValueError) as exc:
            entry["why"] = f"gui.snapshot: {exc}"
            return
        end = time.monotonic() + 8
        while not picture.exists() and time.monotonic() < end:
            time.sleep(0.5)
        if not picture.exists():
            entry["why"] = "the viewport could not be pictured"
        elif png_is_flat(picture):
            entry["why"] = "the viewport's image is one flat colour"

    def exported(self, entry: dict) -> None:
        """The export's file: written, decoded, at the raw's size."""
        end = time.monotonic() + 180
        files: list[Path] = []
        sizes: dict[Path, int] = {}
        while time.monotonic() < end:
            files = sorted(p for p in self.export.iterdir() if p.is_file())
            now = {p: p.stat().st_size for p in files}
            # Written once its size holds between two looks.
            if files and now == sizes and all(now.values()):
                break
            sizes = now
            time.sleep(1)
        entry["files"] = [p.name for p in files]
        if len(files) != 1:
            entry["why"] = f"one file expected in the export folder, found {entry['files']}"
            return
        try:
            got = image_size(files[0])
        except (ValueError, KeyError, IndexError, zlib.error) as exc:
            entry["why"] = f"{files[0].name} does not decode: {exc}"
            return
        want = raw_crop(RAWS[0])
        entry["size"] = list(got)
        entry["expected"] = list(want)
        if not same_size(got, want):
            entry["why"] = f"{files[0].name} is {got[0]}x{got[1]}, the raw {want[0]}x{want[1]}"

    def run(self) -> bool:
        plan = [
            ("launch", None),
            ("first-run", "1-first-run.ron"),
            ("import", "2-import.ron"),
            ("imports-list", "3-imports-list.ron"),
            ("import-session", "4-import-session.ron"),
            ("health", "5-health.ron"),
            ("health-fixes", None),
            ("health-close", "6b-health-close.ron"),
            ("storage", "7-storage.ron"),
            ("quit", None),
            ("relaunch", None),
            ("restart", "8-restart.ron"),
            ("workshop", "9-workshop.ron"),
            ("export", "10-export.ron"),
            ("quit-again", None),
        ]
        for step, file in plan:
            if step == "health-fixes":
                if not self.health_fixes():
                    return False
                continue
            started = time.monotonic()
            if step in ("launch", "relaunch"):
                launched = self.launch()
                entry = {"step": step}
                if "error" in launched:
                    entry["why"] = f"launch: {launched['error']}"
            elif file is not None:
                entry = {"step": step}
                if step == "workshop":
                    self.find_photo(entry)
                if step == "export":
                    self.export.mkdir(parents=True, exist_ok=True)
                # After `find_photo`: the cell's label is read from the tree.
                extra = {
                    "import-session": {"session": self.session},
                    "workshop": {"photo": self.photo},
                    "export": {"image": self.image, "export": self.export},
                }.get(step)
                # The windows open before, the Settings aside (an earlier
                # step may have left them open): what must be left after.
                before = (
                    (
                        [t for t in self.listed() if t != "Settings"],
                        [w for w in self.windows() if w != "settings"],
                    )
                    if step == "health-close"
                    else None
                )
                if "why" not in entry:
                    entry = {**entry, **self.scenario(step, file, extra)}
                if "why" not in entry and step == "health-close":
                    # Past the paint guard's delay (2 s).
                    time.sleep(3)
                    self.windows_left(entry, before)
                if "why" not in entry and step == "imports-list":
                    self.find_session(entry)
                if "why" not in entry and step == "storage":
                    self.storage_probe(entry)
                if "why" not in entry and step == "workshop":
                    self.darkroom(entry)
                if "why" not in entry and step == "export":
                    self.exported(entry)
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
