kind: changed

Large libraries scroll within a graphics memory budget (Thumbnail textures setting, 512 MiB): thumbnails out of view are freed, oldest first, and read back from the thumbnail cache when they return. The cache is no longer listed while the window opens. Compare shows the cached 1024 px picture at once while the full render comes.
