kind: internal

Each saved version of a photo's edit now records what kind of step it was (a new step, an undo, a redo, a save during a drag, a cancelled drag), and Helios can rebuild the photo's undo and redo lists from these versions. Loading the rebuilt history when a photo opens after a restart comes in a later change.
