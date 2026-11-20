kind: internal

Helios builds on egui-pop 0.19.0 (egui_stack 0.31; egui-pop-agent 0.12.0, egui-pop-i18n 0.17.0, egui-pop-test 0.18.0) and on egui_hud 0.12.0, which depends on the same egui-pop: the lock holds one copy of each egui-pop crate and of egui_stack, so egui_hud's renderers read the kit Helios installs. No behaviour change.
