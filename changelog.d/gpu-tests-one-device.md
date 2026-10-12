kind: internal

The GPU tests share one device per process, or take a lock when they open
their own, which avoids a Vulkan loader crash when they run on parallel
threads.
