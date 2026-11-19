kind: changed

Quitting is one request on the bus (`helios.app.quit`), asked whenever closing the main window would end Helios. Its listeners run in a declared order, each within its deadline, and one may keep Helios open with its reason (jobs running ask first, as before). A listener that fails or does not answer in time is logged and the quit goes on. The waits left on the quit path are woken by what they wait for instead of polling a clock.
