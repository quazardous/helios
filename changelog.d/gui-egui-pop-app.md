kind: changed

The desktop is now built on egui-pop's application framework: a menu bar, a command palette that ranks commands by a fuzzy match with the recent ones first, a settings window (Ctrl+,) with a shortcuts editor where a key is changed by pressing it, and the window's size and place kept between runs. Dialogs, toasts and menus are egui-pop's; for now a path field browses with the in-app picker only, and a picker is a list of buttons without a search.
