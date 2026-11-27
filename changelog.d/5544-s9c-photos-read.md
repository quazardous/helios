kind: added

Agents: `helios.library.photos` reads the photos the views show, in their order with the current filter applied, a page at a time (`from`, `limit`): each photo's id, stars, flag, labels, dates, whether it is edited and whether its file is at hand. It is read-only, so an agent allowed only to read can follow the library; names, paths, titles, tags and people are given only to agents allowed private data. Its arguments are checked before it runs. `helios.workflow.submit`, which names the folder written to, now needs private data too.
