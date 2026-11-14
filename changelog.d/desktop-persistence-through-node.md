kind: changed

The desktop's Library filter history, its Folders counts and its workshop sessions' records are read and written through its node, off the frame: a vault busy with another writer no longer stalls the window. The node gains `library.documents.get`/`.set`, `library.folders` and `library.workshop.sessions.list`/`.set` for any client.
