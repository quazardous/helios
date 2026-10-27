kind: added

vkdt's colour module (`vkdt_colour`): the camera's matrix, white balance
towards the camera's white (as shot, or set by hand), exposure, saturation
and highlight clipping. A chain using it leaves out Helios's own white
balance, since vkdt balances here. It runs on the GPU, and the nightly
check compares it with vkdt's own render.
