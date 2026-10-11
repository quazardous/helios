kind: added

External plugins (proof of concept): a plugin is any program speaking
JSON-RPC over its standard input and output, described by a manifest that
is checked before it runs. Helios starts it when first needed, restarts it
after a crash without affecting anything else, and holds it to the
capabilities the user grants (`plugin.list`, `plugin.grant`,
`metadata.read`, `device.capture`).
