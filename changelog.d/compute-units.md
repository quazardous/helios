kind: added

Helios now knows the machines and devices it can compute on. The local
machine is always there: its CPU and every GPU it finds, each with its
name, API, kind and limits (software GPUs such as llvmpipe are listed but
off). Each device can be enabled or disabled, given a role (interactive,
batch or both), a priority and budgets (CPU threads, GPU memory, the
render cache), kept in `~/.config/helios/machine/compute.toml`, a file for
this machine only; a device that goes away keeps its settings and shows
as missing. A short benchmark measures each device on reference modules,
and renders go to the device that fits the job, falling back to the CPU
when a GPU is off, missing or fails, saying where they ran and why.
Remote helios-node instances can be declared and probed. The commands
`compute.list`, `compute.enable`, `compute.disable`, `compute.role`,
`compute.budget`, `compute.benchmark`, `compute.route`, `compute.remote`
and `compute.probe` drive it all.
