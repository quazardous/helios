kind: fixed

View state is no longer an undo step. Each Library view's zoom, sort, auto-advance, folder and album, and the workspace's import and export choices, are still kept in the workspace's files, but Edit › Undo (Ctrl+Z) never takes them back and never stops on them: after opening panels, zooming or sorting, it undoes your last real edit. An undo or redo of a workspace edit leaves the view as it is.
