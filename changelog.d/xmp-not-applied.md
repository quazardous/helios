kind: changed

A darktable edit that uses modules Helios does not have yet (or a version,
a blend or a mask it cannot apply) still imports: those steps are kept
untouched, listed as not applied with their name and version, and one
warning per sidecar sums them up, instead of being left out. A masked or
blended step is no longer applied to the whole image, and a damaged history
entry no longer stops the import. The import report now lists these steps
under `not_applied`, which replaces `unknown_modules`,
`unsupported_versions` and `undecodable`.
