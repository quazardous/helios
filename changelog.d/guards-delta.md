kind: internal

On a pull request, the raw-egui and bus guards now check the merge base too and fail only on what the change adds, printing what went down as progress; the idea comes from darktable's dt-lockcheck diff mode (darktable-org/darktable#23056).
