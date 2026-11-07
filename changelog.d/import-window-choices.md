kind: added

The import window's Choices column: copy the photos into the Library or add
them where they are (a card's are copied, with a note on why; you can still
switch), where copies go (the Library folder or one you choose) filed by
day, month, year or in one folder, and how they are named (original name,
date and name, date and number, date, time and camera, or a custom pattern),
each with a live example from the first photo ticked
("e.g. 2026/2026-11-02/"). Then duplicates (skip, keep or ask) and "Read
darktable edits". Into and Name hide for "Add where they are". The choices
reach the import and are remembered per library. `library.import`,
`library.importFiles` and `helios import` take a `layout` (`day`, `month`,
`year`, `flat`; `--layout` on the command line), and `library.importFiles`
takes `into`, `rename` and `duplicates` too.
