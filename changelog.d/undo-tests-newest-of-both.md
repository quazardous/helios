kind: internal

Two tests that undo a workspace step (a sub-workspace undone while shown, a graph edit undone after a photo switch) now walk the newest step of both history lines, as Ctrl+Z does since the workspace got its own line; they were still calling the image line's undo alone, which left next's CI red.
