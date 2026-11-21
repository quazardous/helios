kind: internal

Helios builds on egui-pop 0.21.0 (egui-pop-agent 0.14.0, egui-pop-i18n 0.19.0, egui-pop-model 0.14.0, egui-pop-test 0.20.0) and on egui_hud 0.14.0, which depends on the same egui-pop: the lock holds one copy of each egui-pop crate and of egui_stack (0.31.1). A plugin page's brick from a newer format is now left out by egui-pop's own manifest check (`admin-unknown-node`) instead of a Helios filter; the plugin's section still says what was left out.
