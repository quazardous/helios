kind: added

On the GPU, images now stay in GPU memory from one step to the next: a
chain of GPU modules uploads its source once and reads its result back
once, and a render's result can be handed over in GPU memory, so a viewport
on the same device shows it without a copy back to the CPU.
