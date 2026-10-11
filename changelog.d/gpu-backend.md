kind: added

Modules can run on the GPU (Vulkan, Metal, through wgpu): a module may
carry a GPU version of its computation next to its CPU one, and the GPU
result is checked against the CPU reference before it is trusted.
Exposure is the first.
