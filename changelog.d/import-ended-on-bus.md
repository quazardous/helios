kind: fixed

Every import now ends the same way, whichever way it ran (a folder or a card, photos chosen in an online service's page, the background node): its end passes once through the `import.ended` (or `import.failed`) listeners, in a declared order, on Helios's one bus (undo step, backups, Library fill, Log, toast, session, selection). Photos chosen in an online service's page now take their camera's default look, and their batches end with one undo step and one backup.
