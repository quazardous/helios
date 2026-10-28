kind: internal

The GPU backend opens its Vulkan device to foreign Vulkan code (vkdt's kernels): raw handles, and images exchanged both ways on the device without a copy.
