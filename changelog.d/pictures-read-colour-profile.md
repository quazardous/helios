kind: added

JPEG, PNG and TIFF pictures now open in the colour space they are tagged
with (an embedded ICC profile, PNG's colour chunks, or a camera's Adobe RGB
setting), so their colours are no longer shifted. Untagged pictures are
still read as sRGB, and the load report says so; a broken profile does not
stop the load, the report gives the reason.
