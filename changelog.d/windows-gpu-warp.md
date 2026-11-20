kind: internal

Windows GPU tests: sigmoid's GPU kernel compiles on DX12 again (FXC refused its write into a vector at a run-time index, so on Windows the step always ran on the CPU), the WGSL test now refuses that write in every shader, and the nightly `windows-tests` run uses nextest, one process per test as on Linux, so the GUI's tests no longer share one WARP device and time out.
