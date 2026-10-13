kind: added

An export reports as a job (`job.*` events on its bus): started when
queued, its render, encode and write steps, a log line with the file
written, and its end; `helios_export::submit_reported` takes the jobs
whose `cancel` stops it, queued or running, writing nothing.
