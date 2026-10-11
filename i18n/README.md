# Translations

One directory per locale, one Fluent file per area: `commands.ftl`,
`panels.ftl`, `modules.ftl`.

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
- `x-pseudo` (`HELIOS_LANG=x-pseudo`) shows accented, longer English to
  spot untranslated text and tight layouts.

Not translated yet: command categories (`View`, `Vault`) have no stable
id of their own, and a key derived from their English would break on
rewording; they stay English until they are declared with ids.
