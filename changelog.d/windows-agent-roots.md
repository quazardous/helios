kind: fixed

Windows: an agent confined to its roots can no longer pass a path starting with a backslash (`\x`, the current drive's root) or a drive letter without its folder (`C:x`) in a command's parameters: such paths are checked against the roots, and refused.
