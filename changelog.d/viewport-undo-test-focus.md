kind: internal

The viewport undo test focuses the image view before Ctrl+Z, as egui-pop now routes Ctrl+Z only to a focused (or last focused) view and never to the newest step of another history; the history module docs say so.
