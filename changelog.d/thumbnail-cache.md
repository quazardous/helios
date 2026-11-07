kind: changed

Library thumbnails come from a cache beside the catalogue (.helios/thumbs, 2 GB by default, setting "Thumbnail cache"): they show at once on the next start, and offline photos keep theirs, with an "offline" badge. A miss reads the file's smallest embedded preview, scaled while decoded, on a few threads, the photos in view first. A file with no readable preview shows its type and a "no preview" mark instead of an empty cell.
