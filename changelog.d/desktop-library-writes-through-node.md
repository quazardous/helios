kind: changed

The desktop's library writes ratings, flags, colour labels, tags and stack expansion through its own node, off the window's thread: a busy library no longer stalls a frame while a mark is saved. `library.fields.set` takes an `op` (`add`, `remove`, `set_if_absent`), and `library.fields.get` reads photos' fields back.
