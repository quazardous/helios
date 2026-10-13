kind: added

`helios batch` shows on the node it drives as one batch job with an
export job per image under it, and cancelling the batch there (`helios
control job-cancel`) stops it: the exports in flight are cancelled, the
others are not started, and no file is left half-written.
