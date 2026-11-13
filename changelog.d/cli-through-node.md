kind: changed

`helios import`, `import-darktable` and `stacks` no longer open the vault: they run on the node holding it, found beside the vault (`.helios/NAME.node.json`) or named by `--node` / `HELIOS_NODE`. With none running, the command starts `helios-node --idle-exit`, shared by commands run at once, which exits when idle.
