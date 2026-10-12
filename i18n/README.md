# Translations

One directory per locale, one Fluent file per area: `commands.ftl`,
`panels.ftl`, `modules.ftl`, and `interface.ftl` for the rest of what the
desktop declares (command categories, menus, settings). They are built
into the desktop (`helios-i18n`) and read by egui-pop's translation
plugin, which also brings egui-pop's own strings (its menus, palette,
settings window and dialogs) in English and French.

- `en/` is **generated** from Helios's declarations (commands, panels,
  module parameters) by `cargo xtask i18n extract`; never edit it. A test
  fails when it is out of date.
- Message ids are derived from stable ids, never from English labels:
  `command-<id>` (`.title`), `panel-<id>` (`.title`),
  `param-<module>-<name>` (`.label`, `.description`) and
  `param-<module>-<name>-choice-<value>`, every character other than an
  ASCII letter or digit written `-`. Rewording English keeps the key.
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
