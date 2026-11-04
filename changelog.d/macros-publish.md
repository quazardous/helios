kind: added

Macros: Publish… in the Modules panel copies a macro's version into
your library (`<config>/composites/`), and every workspace's graph
offers it in its Add menu, under "Macros ›". A dialog asks first and
says where the file goes: publishing is not undone (remove the file to
take it back), and a later change is published as a new version.
Agents use `helios_macro_publish` (with `dry_run`); the file is written
only once you accept.
