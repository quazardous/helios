kind: fixed

Opening the Library no longer adds a "Workspace" undo step: the grid turning its default zoom into a thumbnail size is not a change of yours, so Ctrl+Z undoes your last change in the Library and not that. An undo or redo of the Library whose writer fails unexpectedly is refused and put back instead of leaving the undo thread stuck, which kept Helios from closing.
