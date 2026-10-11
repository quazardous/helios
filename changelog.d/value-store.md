kind: added

Value store: every edited value lives at a stable path (`node/…`,
`settings/…`, `view/…`) with a descriptor from the module's manifest and
hints (ranges, steps, unit). `param.set` and its siblings set a value or
a step, say whether a range held it back, refuse a stale write or one
during someone else's drag, undo by gesture, and announce each change
with who made it.
