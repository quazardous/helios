kind: internal

`cargo xtask engine-diffstat` runs the bench fixtures' pipelines on the CPU and on the GPU and prints, per module, how far the GPU output is from the CPU's: the largest and mean distance, the share of pixels past a threshold and the worst tile, for the module alone and through the chain; `--dump-diff` writes one module's distance image. The idea comes from darktable's `--opencl-diffstat` (darktable pull requests #23072 and #23141).
