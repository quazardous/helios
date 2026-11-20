kind: changed

Quitting with an edit whose save failed now asks first, as a named listener of the quit request (`helios.quit.unsaved`, before the running jobs' question): "Retry saving" tries the save again (asked again if it still fails), "Quit without saving" quits and loses the edit, Cancel keeps Helios open. The closing splash's time limit and the wait for the library's node after the window are now derived from the listeners' declared deadlines plus a margin, under a hard ceiling, instead of fixed constants.
