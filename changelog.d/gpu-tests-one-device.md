kind: internal

The headless GUI tests share one GPU device per process, which avoids a
Vulkan loader crash when they run on parallel threads.
