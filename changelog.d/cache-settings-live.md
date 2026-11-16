kind: fixed

The cache sizes changed in Settings > Storage > Caches (the render cache's budget or share of memory, the thumbnails' cache, the thumbnails' textures) apply at once; before, they applied only at the next start. A plugin's settings changed in Settings reach the running plugin in the same way, through the one "setting changed" event every follower listens to.
