kind: internal

A pull request that adds or rewords an English message without its French translation now fails pr-lint (`cargo xtask i18n check --new-since REF`); the keys untranslated before it do not count.
