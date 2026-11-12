kind: fixed

A workspace whose files change outside Helios reloads only the part that changed: a new network keeps the panels open on screen, a new layout applies without touching the network, and files rewritten with the same content reload nothing. The layout copy saved with the workspace now follows every layout change, so a reload never brings back an older layout.
