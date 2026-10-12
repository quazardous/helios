kind: fixed

A failing GPU no longer fails a render. A step whose GPU code fails or runs out of GPU memory is computed on the CPU instead, with the same result, and the event log says which step and why. GPU memory now has a budget per device: when it is spent, older results kept on the GPU are dropped first. A GPU that stops answering or is lost (a driver reset) is restarted once and the render replayed; if that fails, the rest of the session renders on the CPU, and a message says so. No wait for the GPU can hang the application any more.
