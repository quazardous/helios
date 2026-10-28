kind: changed

`helios batch` now hands its images to the node as one batch the node
works through itself: running the same batch again only exports what is
new, and with `--vault` a batch interrupted or cut short by a crash is
finished by running it again, writing no file twice (`--again` starts
over). A batch that needs a person to answer a decision now ends with the
images left reported, instead of waiting.
