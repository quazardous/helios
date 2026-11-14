kind: changed

Undo and redo of library steps no longer freeze the window while the library is busy: the step moves at once, the status bar says "Undoing…" until the library answers, a second Ctrl+Z queues behind the first, and a refused step comes back with a toast saying why. The Calibration window likewise waits off the frame, saying it is waiting for the library, and shows each answer as it lands.
