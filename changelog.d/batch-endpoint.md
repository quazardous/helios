kind: added

`helios-node --batch-control PATH` serves, in every build, the few control
commands a batch export needs (give a source a file, export, cancel), on a
socket only its user can open; the other control commands stay in debug
builds.
