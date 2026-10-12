kind: added

A vault that was not closed properly (a crash, a kill, a power cut) is
checked before it is used. If the check fails, the vault opens read-only
and the error names the latest verified snapshot to restore from; the
vault keeps its last five snapshots.
