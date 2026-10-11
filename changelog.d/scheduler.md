kind: added

Background work runs as named, cancellable jobs, announced when they are
queued, start and end; `helios inspect wait-idle` returns as soon as nothing
is left to do, and the number of jobs run at once is a machine setting.
