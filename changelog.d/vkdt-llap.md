kind: added

vkdt's local contrast, shadows and highlights (`vkdt_llap`): a local
Laplacian pyramid as deep as the image allows, with vkdt's ten tone curves
(sigma, shadows, highlights, clarity). Its defaults leave the image as it
is. The tone curves and the pyramid's reductions run on the GPU, the rest
on the CPU; the nightly check compares it with vkdt's own render.
