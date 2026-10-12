kind: fixed

Helios now opens one GPU device per process and shares it between the window and the engine, instead of letting the window open a second one. On Linux with a Vulkan loader older than 1.4.350, two devices could crash the application when one was being opened while the other was in use.
