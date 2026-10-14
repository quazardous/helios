kind: fixed

An agent editing a workspace that is open on the desktop now goes through
the desktop: the change shows at once, is one step of Edit > Undo, and the
desktop's next save no longer overwrites it. A workspace's files changed
outside Helios are reloaded as one undo step, keeping what you changed
meanwhile unless the same part changed on both sides, which a warning names.
