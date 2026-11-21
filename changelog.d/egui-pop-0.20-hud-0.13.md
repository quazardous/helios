kind: internal

Helios builds on egui-pop 0.20.0 (egui-pop-agent 0.13.0, egui-pop-i18n 0.18.0, egui-pop-model 0.13.0, egui-pop-test 0.19.0) and on egui_hud 0.13.0, which depends on the same egui-pop: the lock holds one copy of each egui-pop crate and of egui_stack. Undo keeps its routing (the newest step); focus routing stays off.
