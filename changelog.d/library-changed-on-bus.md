kind: fixed

The Library is read again once per change: an import or an undo used to read it twice (the import's end and the vault's own event each asked), and the photos an import brought are now selected in the frame they appear, not a frame later. What changed in the library (photos put in or taken out, where their copies are, capture times, stacks) passes once through the `library.changed` listeners (volumes, Library, albums, stacks), in a declared order, on Helios's one bus; `library.filled` follows once the Library shows it. The albums are no longer compared every frame.
