kind: added

Modules declare the setup steps they need first (the library folder to begin with), with what each fills, when it is done and what it comes after; import requires the library folder. Agents read them on the node with `init.steps` and `init.status`, which says what each module still misses.
