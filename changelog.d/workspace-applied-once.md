kind: fixed

The graph editor and the Workshops panel now follow a workspace change (an edit, an undo, a redo) at once, even while hidden, and agents can wait for `workspace.applied` on the journal, which says what changed.
