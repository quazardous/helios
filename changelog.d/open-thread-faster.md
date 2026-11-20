kind: changed

Opening a raw is faster before its first frame: building its graph no longer hashes the sensor data 4 bytes at a time (about 430 ms on a 24 MP raw, now a few tens), the decode converts the mosaic row by row and hashes the file beside it, and the embedded preview is drawn from its own thread beside the decode, scaled to 2560 pixels before its colours are converted, its smaller previews no longer decoded too. Drawing the embedded JPEG while the raw develops follows darktable and Lightroom. The `open-timing` bench now times the first pixels apart from the first developed frame.
