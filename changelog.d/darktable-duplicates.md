kind: added

darktable's duplicates of a photo (`IMG_0001_01.CR2.xmp` and so on) now
import as versions of that photo, each kept apart, next to the base edit,
which stays the one a batch exports. The new `helios import-darktable`
keeps a whole folder's edits in a vault, listing sidecars whose image is
missing and importing photos that have no sidecar. A camera file name that
merely ends in digits is not taken for a duplicate.
