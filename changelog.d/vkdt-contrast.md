kind: added

vkdt's local contrast (`vkdt_contrast`): a guided filter on the
luminance, its blurs computed at lower resolutions, then the detail
boosted against it. It is the first module made of several kernels, each
computed, cached and tiled on its own, those reading one image on the
GPU; the nightly check compares it with vkdt's own render.
