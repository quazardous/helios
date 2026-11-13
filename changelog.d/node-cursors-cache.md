kind: added
release: minor

Clients read the library at scale through the node: cursors page a query's photos window by window (`cursor.open`, `cursor.window`, `cursor.rank`, `cursor.refresh`, `cursor.close`), each photo read once even while another client writes, and a cursor left unread expires. `library.fields.set` writes photos' fields. A client cache keeps repeated reads off the node and drops exactly what the node's events say changed.
