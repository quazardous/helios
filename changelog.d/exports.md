kind: added

A network can write files: an export renders a node, converts it to sRGB
and saves it as JPEG, PNG (8 or 16-bit) or TIFF (16-bit), with the format
taken from the file name. A file is never seen half written, two exports to
the same file wait for each other, and a node reports each export and says
why one failed.
