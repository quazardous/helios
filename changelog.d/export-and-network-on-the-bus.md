kind: added

An export asked for now starts in the same frame and says so on the journal (`export.started`), or says why it did not (`export.refused`, with the reason: no library open); agents can wait for either. A workspace change to the network is followed by named listeners of `workspace.applied` (`library.heads`, `darkroom.network`), listed in the journal's `ran`.
