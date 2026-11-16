kind: internal

helios-gui's snapshots are cut to what they test: the import window, the Activity and the Library window to their own window (and what floats over it), Compare to the workbench between the bars. The shell's chrome (top bars, status bar, the whole main window once) has its own suite, `chrome_snapshots`, so a change to a bar moves those pictures and no others.
