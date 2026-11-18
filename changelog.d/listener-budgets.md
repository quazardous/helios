kind: added

The library, workspace and darkroom events now have a time budget: when what follows one of them takes longer than 4 ms on the interface thread, the log names the slowest part, so a lag is found by its name instead of by hand. Nothing is stopped or failed by it.
