kind: changed

Helios moves to egui-pop 0.18 (egui_stack 0.30, bus-contract 0.5 with its journal, defer and crossing contract crates, bus-kernel 0.3.1) and egui_hud 0.10 on it. A floating panel with a single tab group no longer shows a second "Close all tabs" ×. The jobs centre follows Helios's jobs through typed events: the desktop says each `helios.job.*` fact, its own or a node's, to egui-pop on `pop.job.event`, instead of renaming its facts `job.*` in egui-pop's journal. Bus point ids are checked when made, and listener ids when they register.
