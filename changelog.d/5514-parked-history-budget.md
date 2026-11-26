kind: changed

The undo history each photo keeps while you work on another is now held within a RAM budget of its own (64 MiB), each history counted by its size, instead of a fixed 20 photos: past the budget, the photo left longest ago loses its parked history first. Opening that photo again rebuilds its history from its saved versions, as after a restart. Settings > Storage > Caches lists the parked histories as "Undo histories" and can clear them.
