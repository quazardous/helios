kind: internal

Each health check now lives in the crate of the module that owns it (storage and backups, the library, the GPU, the plugin host, the catalogue), over that module's own state, gathered once per run. The health crate keeps only the report point, the findings, the ignored list and the runner, and depends on no module.
