kind: added

Agents and scripts can read any photo's undo history with `photo.history`, open or not: its steps from the saved versions, oldest first, the undone ones marked, a page at a time (`from`, `limit`), each step's edit on request (`edits`). It is read-only, so a client the user has not paired can read it too.
