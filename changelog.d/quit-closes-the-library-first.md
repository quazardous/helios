kind: fixed

Quitting stops each background job as a named step on the closing screen, within its own deadline, then closes the library. When the system ends Helios (logout, shutdown, `kill`), the quit now has time to close the library; if it still ends with the library open, the next start reports an unclean end instead of a clean one.
