kind: fixed

Closing Helios no longer leaves a black window on screen for seconds. The quit's own work takes a few dozen milliseconds, but the window toolkit only left its event loop at the next window event; Helios now finishes the quit without waiting for it, so the window and the process go at once.
