kind: fixed

Editing the workspace's network with an image open (adding or removing
a node, a reset, an import) no longer clears the image's undo history:
the image stays open through the new network with its values, and the
edit can be undone like any other step.
