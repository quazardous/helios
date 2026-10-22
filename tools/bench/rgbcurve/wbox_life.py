#!/usr/bin/env python3
"""Start or stop the scenario's wbox compositor (and darktable in it).

    wbox_life.py up   CONFIG
    wbox_life.py down CONFIG

Lifecycle only, the same calls as the fork's tools/helios/wbox/wbox_ctl.py
(pinned wbox-mcp a2f3eab). Runs under wbox's own interpreter. Headless
unless WBOX_VISIBLE=1. Prints the wbox result as JSON.
"""

import json
import os
import sys
from pathlib import Path

from wbox.config import apply_overrides, load_config
from wbox.server import _build_app_cmd, _build_app_env, build_compositor


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in ("up", "down"):
        sys.exit(__doc__)
    command, config_path = sys.argv[1], Path(sys.argv[2]).resolve()
    cfg = load_config(config_path)
    cfg["headless"] = os.environ.get("WBOX_VISIBLE", "").lower() not in ("1", "true", "yes", "on")
    apply_overrides(cfg, [])
    cfg["_config_dir"] = str(config_path.parent)
    os.chdir(config_path.parent)
    comp = build_compositor(cfg)
    if command == "up":
        result = comp.launch(_build_app_cmd(cfg), _build_app_env(cfg))
    else:
        result = comp.kill()
    print(json.dumps(result, default=str))
    return 1 if isinstance(result, dict) and result.get("error") else 0


if __name__ == "__main__":
    sys.exit(main())
