//! helios-gui's integration tests: one module per area, all linked into
//! this one executable. Each test target used to be an executable of its
//! own carrying the whole GUI and GPU stack; a hundred of them filled the
//! box's target and relinked that stack once each.
//!
//! The suite runs under nextest, one process per test, as CI and the box
//! run it. Some modules hold process-wide state that relies on it: a `log`
//! logger (`accounts`, `settings_plugins`); others start this executable
//! again as a child, naming a test by its full path (`dialog_details`,
//! `english_on_a_french_system`, `platform`, `survival`).
//!
//! One module alone: `--test it -E 'test(/^library_filter::/)'`; the box's
//! `test=library_filter` maps to it (tools/bench/Benchfile).

mod accounts;
mod activity_focus;
mod background_jobs;
mod batch_undo;
mod caches;
mod calibrate;
mod calibrate_focus;
mod chrome_snapshots;
mod clipboard;
mod compare_focus;
mod compare_heads;
mod contexts;
mod control_names;
mod culling;
mod darktable_library;
mod decisions;
mod develop_raw;
mod dialog_details;
mod dialogs;
mod display_route;
mod duplicates;
mod edit_marks;
mod edit_snapshots;
mod edited_badge;
mod engine_notices;
mod engine_source;
mod english_on_a_french_system;
mod exit;
mod export;
mod fonts;
mod fullscreen;
mod graph_panel;
mod gui;
mod health;
mod history_panel;
mod history_preview;
mod image_open;
mod image_tools;
mod import;
mod import_albums;
mod import_auto;
mod import_card_identity;
mod import_close;
mod import_end;
mod import_handoff;
mod import_profiles;
mod import_snapshots;
mod import_source_switch;
mod importer_plugins;
mod init_wizard;
mod input;
mod jobs;
mod keep_image_open;
mod keymap;
mod language;
mod library_changed;
mod library_filter;
mod library_gallery;
mod library_imports;
mod library_scale;
mod library_sort;
mod library_window;
mod local_node_jobs;
mod macros_desk;
mod mask_display;
mod mask_handles;
mod mask_tree;
mod masks;
mod modules;
mod node_jobs;
mod node_volumes;
mod normalized;
mod palettes;
mod params;
mod persist;
mod photo_panels;
mod picture_orientation;
mod platform;
mod plugin_consent;
mod queue;
mod quick_open;
mod quick_settings;
mod quit;
mod raw_front;
mod scale;
mod scenario_files;
mod scopes_size;
mod scopes_snapshots;
mod screens;
mod settings_app;
mod settings_plugins;
mod settings_transfer;
mod settings_window;
mod shell_followers;
mod shortcuts;
mod shot;
mod splash_snapshots;
mod stacks;
mod storage;
mod subworkspaces;
mod survival;
mod system_menu;
mod templates;
mod theme_editor;
mod themes;
mod thumb_budget;
mod thumbnails;
mod tips;
mod viewport;
mod window_close;
mod window_commands;
mod workshop_design;
mod workshop_run;
mod workshops;
mod workspace_applied;
mod workspace_edit;
mod workspace_keys;
mod workspace_live;
mod workspaces;
mod zero_copy;
