kind: fixed

`helios import` waits for the import and prints its report again (its progress on the terminal), exiting non-zero when it fails; `--no-wait` returns the promise at once. `helios run` waits likewise for a command answering with a promise. A closed output (a pipe whose reader is gone) no longer makes `helios` or `helios-gui --version` panic.
