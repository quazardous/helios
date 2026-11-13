kind: added
release: minor

Import glances for remote sources: a connector that gives previews (Google Photos does) has its items' glances kept in the glance cache, in memory and on disk, by connector, account, item and its version, so a pick opened again, even after a restart, asks nothing of the service. Reading ahead keeps to the limits the connector declares (requests a minute, a daily hint; a source plugin's manifest says `thumbnails` and `limits`) and to the cache's budget, never pushing out a glance shown.
