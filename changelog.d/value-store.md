kind: added

Value store: every value widgets and agents edit lives at a stable path
(`node/<node>/<param>`, `settings/…`, `view/…`) with a descriptor (type,
hard and soft range, steps, unit, default). `param.list`, `param.get`,
`param.set` (a value, a step, reset, top or bottom; `force` past the soft
range; `if_seq` to write only if nothing changed since reading),
`param.reset`, `param.undo` and `param.redo` answer what was applied and
whether it was clamped. A human's drag holds its values against agents;
each drag or group is one undo step; every change is announced as
`param.changed` with who made it. Modules may give parameter hints (soft
range, steps, unit, display scale).
