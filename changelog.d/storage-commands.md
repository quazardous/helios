kind: added

Read-only commands for the storage: `library.volumes` lists every volume known (state, roles, space, photos, whether the backup strategy copies to it) and `library.backupStrategy` the library's strategy with its disks plugged in; `library.volumeRole` gives a volume a role or takes it back. Commands now declare whether they only read, which the Settings app and read-only agents rely on.
