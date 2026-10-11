kind: added

A headless node renders a network described in JSON, and the `helios`
command observes it: its state, its events, and waiting for an event, so
scripts and agents never have to sleep or parse logs. A wait that times
out says what happened instead.
