kind: added

The render cache stays within a memory budget (a machine setting, 2 GiB by
default): what was used least recently is evicted and computed again when
asked for, and content still being computed is never evicted.
