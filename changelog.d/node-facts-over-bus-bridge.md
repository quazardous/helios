kind: changed

On Unix, a node followed with `--node` sends its facts to the desktop over bus-bridge, on a socket next to its inspect socket: a fact reaches the desktop in well under a millisecond, the first link starts from the node's present, and a link back resumes where it left off. Windows keeps the inspect stream.
