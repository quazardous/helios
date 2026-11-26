kind: internal

`cargo xtask affected` lists the guard unit tests a change can break as manifest lines (helios-gui's `plugins::tests` for a desktop source, helios-node's `commands::tests` for a node verb), and `cargo xtask gate` leaves out a pull request whose `pr-lint` is red or still running, naming the job.
