kind: added

A node can also be reached over loopback TCP (`tcp://127.0.0.1:PORT`), for
systems without Unix sockets: only local connections are served, and each
must present a session token the node writes to a file only its user can
read.
