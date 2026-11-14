kind: added

New read-only node commands `library.version.get`, `export.plan`, `import.known` and `import.recorded`. An export is now planned off the frame, so starting one no longer freezes the window while another writer holds the catalogue; exports, the Compare panel's head renders and the import window's "in your Library" prefilter and offline listings read through the node instead of the catalogue directly.
