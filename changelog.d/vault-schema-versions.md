kind: added

The vault now records its schema version and upgrades older vaults step by
step, after taking a verified copy of the database next to it. A vault
written by a newer Helios is refused with a clear message instead of being
silently changed, and an upgrade gets past leftovers from old versions
(stale views, rows pointing to deleted images), reporting what it found.
