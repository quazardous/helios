kind: fixed

The Library's marks bar says what its buttons do: each colour label, pick and reject shows a tooltip with its name and the keys bound to it now ("Red label (6)", "Flag as pick (P)", "Reject (X)"). A test walks the main window, the Library window, the import window and the Activity focus and fails on any clickable control without a name, or named by a symbol alone.
