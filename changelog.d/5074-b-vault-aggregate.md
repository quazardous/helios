kind: added

The node answers `library.aggregate`, a read-only request: one value per group of a set of photos (a search or a list), from a measure the library already stores (the culling focus and clipping, the rating, the ISO…): its count, minimum, maximum, mean, the photos ranked, or the one proposed on top of a burst. The set is taken whole, split by stack, or split into bursts as the stack suggestions find them; a photo without the measure is skipped and listed as missing, and ties all hold the value.
