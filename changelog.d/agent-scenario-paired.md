kind: changed

Under `HELIOS_AGENT_PRESET=scenario` (or `scenario+private`), Helios pairs the scenario runner at start with a token from `EGUI_POP_AGENT_TOKEN`, or one it writes to the file `EGUI_POP_AGENT_TOKEN_FILE` names; the runner says `hello` with it (`scenarios --token-file PATH`). Only that runner and agents the user pairs get the scenario grant: unpaired clients only read again. `HELIOS_AGENT_WRITE` was removed: it grants nothing and logs a warning.
