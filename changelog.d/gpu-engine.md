kind: added

The engine can compute on the GPU: given a GPU device, every step whose
module has a GPU variant (exposure, blur, sigmoid so far) runs there, the
rest stays on the CPU in the same network, and the result matches the CPU
render. A GPU kernel that fails now reports an error instead of crashing.
