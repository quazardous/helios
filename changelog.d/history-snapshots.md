kind: added

History snapshots: "Take a snapshot" in the History panel names the
edit as it is, kept with the image once the edit is next saved (shown
"not saved yet" until then). A click restores it as one step that Undo
takes back, with a toast saying so. Agents have
`helios_history_snapshot` and `helios_history_restore`.
