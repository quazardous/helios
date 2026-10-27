kind: added

vkdt's film curve (`vkdt_filmcurv`): a Weibull tone curve that compresses
scene light for display, its colour kept by vkdt's default hue-preserving
step or applied per channel. It runs on the GPU, and the nightly check
compares it with vkdt's own render.
