kind: added

Each health check runs when its declaration says: every few minutes or daily, and at once when a disk comes or goes for those that ask. A run of some checks keeps the others' findings. Help > Health check groups the findings by the module or plugin whose check found them. Plugins declare `every_minutes` and `on` in `[[health]]`.
