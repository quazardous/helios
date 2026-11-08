kind: added

Importer plugins speak a version 2 of their protocol: a scan streams its photos in pages, with progress and a cancel between pages, and an image may carry its edits as a darktable history. The darktable library importer uses it: each photo's edit is rebuilt from library.db, so edits with no .xmp (write_sidecar_files off) are no longer lost; when both exist, the newer one is taken, and the import's log says which and why.
