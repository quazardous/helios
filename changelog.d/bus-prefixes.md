kind: internal

Helios builds on the published egui-pop 0.15 and egui_hud 0.9 crates, with no git revision left in its graph. Every bus point and listener Helios declares takes the `helios.` prefix (`helios.image.open`, `helios.library.changed`; a health check's listener is `helios.check.<id>`), egui-pop's are `pop.*`; `cargo xtask bus --check` and a test hold Helios to the rule. The journal says Helios's passes under the new point names.
