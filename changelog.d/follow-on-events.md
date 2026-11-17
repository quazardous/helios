kind: fixed

An undo that takes away the sub-workspace shown now leaves it in the same frame, and the Library's import sessions are read again when an import is recorded (any client's) instead of every 10 seconds, so an idle desktop no longer asks the node for them in the interface's thread.
