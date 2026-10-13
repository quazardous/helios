kind: fixed

A batch whose `helios batch` process dies no longer shows as running on
its node for ever: the node ends it as abandoned when the process's
connection closes, or after ten minutes with nothing asked for it.
