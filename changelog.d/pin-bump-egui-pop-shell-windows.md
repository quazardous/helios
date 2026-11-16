kind: internal

Helios `next` moves to egui-pop `next` with the window invariants checked against what is on screen and the shell's windows on the bus (ADR 0029): the main window's close, `quit` and a window's content moving into the main window are bus requests. Quitting from outside the main window's pass (the system's end of session, a catalogue restore) now runs Quit, which ends in egui-pop's `quit`, instead of a raw viewport close; a restore with another window open now quits instead of moving that window into the main one.
