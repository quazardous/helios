kind: fixed

Every import now ends the same way, whichever way it ran: one end passes through the `import.ended` listeners in a declared order (undo step, backups, Library fill, Log, toast, session, selection). Photos chosen in an online service's page and imported without the background node now take their camera's default look, and their batches end with one undo step and one backup.
