kind: added

An agent's command batch is one step of the library's history: the window commands it runs (rotations, removals, album changes) are listed as "Agent: 3 commands", one undo takes them all back and one redo puts them again. A batch stopped by a failed step keeps one step for what ran; a batch that changed nothing undoable leaves none.
