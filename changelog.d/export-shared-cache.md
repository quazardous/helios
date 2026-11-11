kind: fixed

Exports render on the desktop's own engine: they share the render cache and its budget, so exporting the image open in the darkroom does not compute again what it already holds, and a large export no longer grows memory without bound (the exporter's own cache had no limit).
