kind: added

Queue panel: photos dragged out of the library and dropped on the Queue panel join the export run queued at the selected node, which exports them with the run's settings. A drop where no run queues yet is refused, saying why. Agents do the same with `helios.workflow.submit`, and a node's `workflow.submit` now takes library photos (`image:` and their content hash) as items, exporting the copy at hand.
