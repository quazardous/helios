kind: fixed

A health finding's fix that takes a while (it finishes on a later frame) no longer has its re-check run before it ends: the checks run again once the fix's command is done, and the Health page shows the new answer by itself. Each module's health checks now answer one collect point, `health.report`.
