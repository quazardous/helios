kind: internal

Helios `next` moves to egui-pop `next` with the Settings app's window and the job lifecycle on the bus (ADR 0029) and undo keeping values changed outside their history. The log's "Settings app open" line for its own window now comes from a listener on `window.open`, as the close's does on `window.close`, and the `settings-close` bench holds `admin.close` to that listener too.
