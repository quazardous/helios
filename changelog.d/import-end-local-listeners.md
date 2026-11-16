kind: internal

Helios `next` moves to egui-pop `next` with local bus listeners (bus-contract 0.2.0, from crates.io): listeners holding the interface's state, bound to its thread, run in the declared order with the others. The interface's listeners of an import's end (cards waiting for backups, Duplicates, Log, toast, import session, the import window's History, selection) and of `library.filled` now each do their own part inside the pass, instead of marking their place for the shell to do it after.
