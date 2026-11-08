kind: fixed

The darktable library importer now opens libraries on Windows: it built a `file:/D:/…` address that SQLite refuses, now `file:///D:/…`. A library from a darktable older than 4.0 is refused with a message naming its schema and asking to open it once in darktable 4.0 or later, which upgrades it.
