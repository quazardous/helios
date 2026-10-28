# Translations

One directory per locale, one Fluent file per area: `commands.ftl`,
`panels.ftl`, `modules.ftl`, `interface.ftl` for the rest of what the
desktop declares (command categories, menus, settings),
`messages.ftl` for the messages the code builds (a GPU that fell back, an
export that failed, a toast), and `tips.ftl` for the tips
(`crates/helios-gui/tips/<id>.md`: `tip-<id>` with `.title` and `.text`,
`{key}` written `{ $key }`). They are built
into the desktop (`helios-i18n`) and read by egui-pop's translation
plugin, which also brings egui-pop's own strings (its menus, palette,
settings window and dialogs) in English and French.

- `en/` is **generated** from Helios's declarations (commands, panels,
  module parameters) and its code's messages by `cargo xtask i18n extract`;
  never edit it. A test fails when it is out of date.
- Message ids are derived from stable ids, never from English labels:
  `command-<id>` (`.title`), `panel-<id>` (`.title`),
  `param-<module>-<name>` (`.label`, `.description`) and
  `param-<module>-<name>-choice-<value>`, every character other than an
  ASCII letter or digit written `-`. Rewording English keeps the key.
- A message the code builds is written `Message::new("key", "English
  { $arg }")` (`helios_core::Message`), with literals: the extractor reads
  its key and English from the sources (`crates/*/src`, `modules/*/src`).
  Its key names its area (`engine-…`, `export-…`, `xmp-…`, `gui-…`); its
  English uses arguments only, translations may use all of Fluent
  (plurals, `NUMBER`). The message carries its English, so whoever cannot
  translate it still reads it.
- A locale may translate only some keys: the rest falls back to its parent
  (`fr-CA` to `fr`), then to English.
- `<locale>/english.hashes` records the English each key was translated
  from. `cargo xtask i18n stamp <locale>` records newly translated keys;
  `cargo xtask i18n check` reports coverage, missing and orphan keys, and
  keys whose English changed since (review them, then `stamp <locale>
  --all`).
- The desktop's Language setting picks the locale; `system` reads
  `HELIOS_LANG`, then the system's language. `x-pseudo`
  (`HELIOS_LANG=x-pseudo`) shows accented, longer English to spot
  untranslated text and tight layouts.

Command categories (`View`, `Vault`) have no stable id of their own:
their key is derived from their English (`category-vault`), so rewording
one needs its translations moved too.
