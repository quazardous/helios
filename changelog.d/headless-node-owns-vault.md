kind: added

`helios-node --vault` runs what the desktop runs without a window: it watches the machine's disks and cards, backs up to a backup disk of the strategy when one is plugged in, and runs the health check on its schedule. It opens the vault once, and SIGTERM or Ctrl-C now stops it cleanly: running backups are cancelled after their current copy, and the vault is closed and marked clean.
