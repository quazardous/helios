kind: added

The node verb `workflow.extract` submits extractor runs: extractor uses (`id@version`, inputs, route overrides, the nodes each waits for) over library photos (`image:<hash>`). Extract runs are listed by `workflow.runs` beside the export runs (`kind: "extract"`, counts per extractor node), `workflow.items` pages their photos at each node, and `workflow.cancel` cancels them through their job, keeping what concluded, so the Queue panel shows them as it shows export runs. The node process opens no AI yet: until it does, the verb is refused.
