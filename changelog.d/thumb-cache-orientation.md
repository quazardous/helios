kind: fixed

A picture shot in portrait whose thumbnail was cached before pictures were turned no longer stays sideways: the thumbnail cache marks the entries it stores with its reader's version, and an older entry of a picture its EXIF orientation turns is read again, while those of untagged pictures and raws are kept, with no rebuild.
