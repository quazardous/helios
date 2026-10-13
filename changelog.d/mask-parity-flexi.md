kind: added

The drawn-mask parity job also builds darktable's flexi masks pull
request at a pinned commit: it renders two flexi fixtures and every
classic one again (which darktable migrates to flexi masks as it reads
them), and keeps darktable's own decoding of the fixtures' masks, which
a test compares with what Helios reads.
