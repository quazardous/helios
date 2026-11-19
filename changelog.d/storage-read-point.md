kind: internal

Storage reaches the Library and the health check through the bus instead of direct calls: what it read (volumes, each photo's backups) passes through the new `helios.storage.read` point, which the Library listens to, and the health check registers its own listener of `helios.card.safe`.
