kind: added

The GPU calibration reports as a job once the compute manager is given
the jobs (`Manager::report_to`): each device's reference modules as items,
its speeds as log lines, a device that failed as a failed item, and a
cancel that stops it between two modules.
