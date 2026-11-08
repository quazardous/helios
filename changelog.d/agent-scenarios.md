kind: added

Scenarios run live: `HELIOS_AGENT_WRITE=1` lets an agent on the socket change the image and the library, quick open goes to `file:<path>`, and the box's `scenarios` job replays `crates/helios-gui/tests/scenarios` (import a folder, exposure in the darkroom, undo and redo) on a throwaway library in a headless compositor, with invariants after every step, shots and the journal (docs/gui.md, "Agents").
