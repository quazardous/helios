kind: added

Compute units: Helios lists the CPU and every GPU of this machine
(software ones off), with per-device settings kept for this machine only
(enabled, role, priority, thread, memory and cache budgets), a short
benchmark, and routing of each render to the device that fits, falling
back to the CPU and saying where it ran. Remote helios-node units can be
declared and probed; `compute.*` commands drive it all.
