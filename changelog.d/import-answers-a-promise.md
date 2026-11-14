kind: changed

`helios import` hands the import to the node and answers at once with a promise: the import goes on in the background and survives a restart, picking up where it stopped. `--wait` waits for its outcome as before, and `--new` imports only what the library does not know. Agents start the same import with `library.import {source, rule}` and wait with `promises.wait`.
