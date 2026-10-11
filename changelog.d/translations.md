kind: added

Translation catalogs: Helios's commands, panels and module parameters get
Fluent catalogs in `i18n/`, generated in English from their declarations
and falling back from a regional locale to its language, then to English;
`cargo xtask i18n check` reports each language's coverage and stale
translations. A first French catalog and an accented pseudo-locale
(`x-pseudo`) are included; the desktop shows them in a next step.
