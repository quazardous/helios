kind: fixed

The desktop applies Settings › Storage › Caches › Render cache (`system.cache.budget`, or `system.cache.share` of memory): only `helios-node` did, the desktop kept a quarter of memory whatever was set. The render cache is listed on the Caches page and cleared with the others, and evicting many entries at once no longer scans the whole cache for each one.
