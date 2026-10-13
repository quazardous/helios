kind: added

`helios_core::jobs`: background work reports how it goes as `job.*`
events on the bus (started, progress with done and total, the current
item and step, log lines, an item failed, finished, failed, cancelled),
the shape egui-pop's jobs centre folds; a job is cancelled by its id
(`Jobs::cancel`). `Scheduler::canceller` cancels jobs without holding the
scheduler.
