kind: internal

The diffstat tool reports, per module, where it ran (GPU, CPU only, or fell back) and fails the run when a module with a GPU kernel fell back to the CPU, naming it: a kernel the driver refuses no longer passes as a perfect match.
