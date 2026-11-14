kind: added

`helios run` chains commands: steps between `--` or a script (`--file`, `-` for the standard input), run in order as one batch, one line per step (`--json`: one JSON object each). The first failed step stops the rest unless `--keep-going`; exit 1 when a step failed, 4 when the outcome is unknown. Each step is keyed after `--request-id`, so the same chain run again applies a step once.
