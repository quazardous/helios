kind: added

Profiles. Your theme, your own shortcuts and your panel layout can be
saved as a profile (`profile.export`), read back on another machine
(`profile.import` shows what would change, `profile.import.apply`
applies it), and chosen by name (`profile.use`); each workspace remembers
its profile. Profile files are readable text and never carry secrets: an
export that would is refused. A panel you closed now stays closed when a
layout is restored.
