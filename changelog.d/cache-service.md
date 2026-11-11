kind: added

Settings › Storage › Caches shows what each cache holds (thumbnails and large thumbnails, against their budget) and clears them: one kind with its "Clear", or all with "Clear caches"; photos, edits and the catalogue are never touched. The thumbnail folder is tagged as a cache (`CACHEDIR.TAG`), so backup tools skip it, and existing thumbnails stay valid. `helios-node` answers `cache.stats` and `cache.clear` too.
