kind: fixed

Closing the Import window no longer waits on work it left: a connection in progress is given up, and a source's choices are saved to the library off the interface's thread, so a busy library cannot hold a frame. If the interface is ever held more than 2 seconds, Helios now logs what it was doing and leaves a `stall-*.txt` note in its crash folder.
