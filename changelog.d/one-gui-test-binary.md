kind: internal

helios-gui's integration tests link into one test executable instead of about a hundred, each carrying the whole GUI and GPU stack: far less disk and link time on the bench box and in CI. The box's `test=<former target>` still runs that module alone.
