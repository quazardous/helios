kind: internal

Settings declared by their owners: thumbnails, storage, the library, decisions, the node and the desktop shell each declare their own settings; helios-node-core gathers the sets into the one schema the node and the desktop read. Only the render cache's size stays in helios-config. Keys, defaults and help texts are unchanged; a test checks every key is still declared exactly once.
