kind: internal

One module catalogue: the new helios-modules crate lists each processing module once (its id, its node entry, whether templates may name it, its control patches); the node, the desktop, templates and calibration read it instead of naming the modules themselves. A test fails when a crate under modules/ has no entry.
