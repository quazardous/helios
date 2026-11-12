kind: added

`helios-node --vault` imports through its control endpoints (`helios control import FOLDERS-OR-IMAGES...`, the `library.import` request, also on the batch endpoint): each import is announced on the node's bus as `library.imported`, and the node then runs the backup strategy's after-import backup, as the desktop does.
