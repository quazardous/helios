kind: changed

The edit's marks (the Modules panel's dots, the status bar's "edited"), the workspace's state after an undo or a redo, and the workshop session's record now follow the values on the application's bus (`values.changed`) instead of checking them every frame: a change made while the panels draw shows in the same frame, and one made from a window drawn apart or another thread at the next. A change made before the drawing reaches the photo before it is drawn again.
