kind: internal

The desktop's own node is no longer copied to its listeners by a relay thread: its facts are the application bus's journaled facts, and the Storage reads, the import window's cards and the import sources follow them as named listeners of the bus's points (`helios.reads`, `import.cards`, `helios.import-sources`), each fact heard once.
