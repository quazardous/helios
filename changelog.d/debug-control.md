kind: added

In debug builds, a running node can be driven from the command line with
`helios control`: render a node, change a parameter, cancel a job, pause
and resume work, and make a computation or a source read fail once to test
what happens on errors. A failure injected this way is never cached.
