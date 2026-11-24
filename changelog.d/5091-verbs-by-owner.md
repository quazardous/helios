kind: internal

Node verbs declared by their owners: the workflow, the decisions, the settings layers and the caches each declare their own verbs as a set, with the new `helios-command` contract (what a verb declares, moved out of helios-node-core so the lowest crates can use it); the node gathers the sets and keeps only its network's and scheduler's verbs. Ids, labels, schemas and grants are unchanged; a test compares every command with the list before the move.
