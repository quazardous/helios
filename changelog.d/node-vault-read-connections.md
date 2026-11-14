kind: fixed

A vault on disk reads through connections of its own beside its single writer: with many clients on one node, reading an image, its edit, its fields or the stacks no longer waits behind imports and saves.
