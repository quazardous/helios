kind: added

"Remove from Library" takes the selected photos out of the Library (Delete in the grid, as Lightroom and darktable, or the grid's context menu): their flag only, files, marks and edits kept, and Ctrl+Z puts them back. Through the node's new `library.images.remove`, which answers the step; `vault.library_changed` is announced, so every reader of the library refreshes. Command `library.remove`.
