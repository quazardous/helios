kind: internal

A bench box `soak` job runs a moderate mix of clients on one headless node for an hour (`minutes=`), sampling its memory, threads, descriptors, vault files and write-ahead log, caches and bus every 30 s; no measure may grow, no call pass its deadline, and the vault must hold exactly what was acknowledged. The series goes to `out/soak.json`; a six-second variant runs as a normal test.
