kind: added

The image map scans a volume when it is plugged in: the folders it knows there first, then the rest, a stat per file and no read, in the background, pausable, giving way to imports and other jobs. Which volumes it scans follows system.map.record (every one, those with a role, or none when only imports record); once a scan ends, the cap on files outside the Library drops those seen longest ago. library.map.scanNow scans a volume, or every one allowed, on demand.
