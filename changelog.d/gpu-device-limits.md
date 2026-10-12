kind: fixed

GPUs are now opened with everything they allow instead of wgpu's small default limits, so images over 128 MB, such as a 24 MP photo in full precision, no longer fail on a GPU that was chosen to run them. When a driver refuses its own limits, the device opens with less, says why, and work that no longer fits goes to the CPU.
