kind: added

Helios's exposure is now checked against darktable's own render: the same
image goes through darktable-cli and through Helios, and the results must
agree within darktable's colour difference thresholds (they agree to
within a ten-thousandth of a Delta E). The check runs nightly.
