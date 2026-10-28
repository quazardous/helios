kind: added

A node can now take a batch as a run of the workflow level: each image is
exported by the node itself, retried when a second try may help, and
followed per step, and submitting the same batch again redoes nothing that
is done. Output file names are decided once, when the batch is submitted.
