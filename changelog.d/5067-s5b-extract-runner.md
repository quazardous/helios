kind: added

Extractor runs over library photos are worked by the node on a thread of their own, as a job of kind `extract`: the Activity focus and `promises.list` show the run with its progress (rows concluded of photos × extractors, failures counted), and cancelling it stops the run after the photo at hand, keeping what concluded. Each run writes its document beside the export runs' (`workflow/runs`, `kind: "extract"`), which the export runner leaves alone.
