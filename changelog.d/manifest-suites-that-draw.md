kind: internal

`cargo xtask affected` lists, as test manifest lines, the desktop suites that draw what a change touches (a change to the Library's sources lists `control_names`, `library_filter`, `library_gallery` and `library_window`), so a pull request's manifest no longer misses a suite that looks at the same screen from another angle.
