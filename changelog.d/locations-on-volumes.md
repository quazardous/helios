kind: changed

A photo on a disk that is unplugged is now offline, not missing: Helios
knows which volume each copy is on and its path there, keeps the photo,
its marks and its edits, and finds it again when the disk comes back,
even at another mount point or drive letter. The volumes are scanned in
the background at start, when a disk is plugged in or out, and every
five minutes. Repair never marks an offline photo missing.
