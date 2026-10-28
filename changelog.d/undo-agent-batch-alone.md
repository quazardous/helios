kind: added

"Undo an agent's last changes alone" in the command palette takes back
what an agent last did even after you went on working, unless you have
changed the same values since. Agents undo, redo and jump only through
their own steps, and can undo one of their batches alone with
`helios_history_undo` and its `group`.
