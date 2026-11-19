kind: internal

The Library and calibration windows are woken by listeners registered for them, as the import window's are: thumbnails and head renders say `helios.library.window.news` and each calibration answer landed says `helios.calibrate.window.news` on the bus, and egui-pop wakes that window only. The last hand `wake_window` calls are gone.
