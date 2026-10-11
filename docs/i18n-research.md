# Internationalization: research and recommendation

Status: **proposal** (2026-10-11), for the i18n plan (translatable UI,
manifests and messages, locale-aware formats). It surveys the Rust
ecosystem, darktable and tvty (a Rust desktop application by the same
author that ships Fluent translations), and settles the plan's open
questions. darktable paths are relative to its source tree.

## Rust libraries (October 2026)

| Library | Format | Plurals and selectors | Keys checked | Runtime catalogs (plugins) | Notes |
|---|---|---|---|---|---|
| `fluent-bundle` (Mozilla) | Fluent `.ftl` | yes, in the translation (CLDR) | no | yes: `add_resource` at runtime | the reference implementation; stable, slow releases; `fluent-pseudo` for pseudo-locales |
| `i18n-embed` + `i18n-embed-fl` | Fluent | yes | yes, `fl!()` at compile time | yes, loaders per crate, files from disk | used by COSMIC; `sys-locale` detection, runtime switching |
| `fluent-templates` | Fluent | yes | no | yes (`ArcLoader`) | lighter; used by Ruffle |
| `es-fluent` | Fluent | yes | typed derives | yes | young (0.x), needs a very recent Rust; worth watching |
| `rust-i18n` | YAML, JSON, TOML | **no** CLDR plurals | no | yes | easiest start, wrong for "3 images selected" |
| `gettext-rs` | PO / MO | plurals, contexts | no | text domains | best translator tooling; process-wide `setlocale`, libintl dependency |
| ICU4X MessageFormat 2 | MF2 | yes | — | — | **not shipped**: tracker unicode-org/icu4x#3028 open, a first parser opened on 2026-10-07 |

All are licensed MIT or Apache-2.0, and ICU4X under Unicode-3.0; every one
of them is compatible with GPLv3.

Who uses what:
- **COSMIC**: Fluent through i18n-embed.
- **Firefox**: Fluent, translated in Pontoon.
- **Ruffle**: Fluent through fluent-templates, translated on Crowdin.
- **Slint**: built-in `@tr` over gettext PO.
- **No UI translation**: Zed, Lapce, Helix, rerun, iced.

**Formatting.** For locale-aware formatting there is ICU4X 2.x:
- **stable:** decimal, plural, date and time, list;
- **experimental:** units and relative time (`icu_experimental`).

Its data either compiles in (several MB) or loads at runtime from a blob
generated for the shipped locales. `sys-locale` is the usual way to detect
the system's language. `num-format` and chrono's locale feature are too
narrow.

**egui 0.36:**
- **Shaping:** proper shaping (harfrust) since 0.35.
- **CJK fonts:** non-Latin fonts must be registered by hand until 0.37 loads system fonts.
- **Right-to-left:** **not supported yet**. Bidi is an open pull request (emilk/egui#8577), and `Layout::right_to_left` only mirrors placement.
- **IME:** usable since 0.35.

## darktable

- **gettext.** darktable uses gettext throughout (`_`, `N_`, `C_`, `NC_`, `Q_`), with 37 languages in `po/`. Translators send `.po` files as pull requests; there is no translation platform. A language ships with at most 20 untranslated strings.
- **Parameter labels.** They come from `$DESCRIPTION` comments in the parameter structs, wrapped in `N_()` by the introspection generator (`tools/introspection/ast.pm`) and extracted by xgettext. Contexts on them were tried and dropped because they broke existing translations (#5498).
- **An action's id is its English label.** `dt_action_locate` (`src/gui/accelerators.c`) stores `NQ_(path)` as the id and shows `Q_(path)`. Shortcuts files and Lua scripts (`dt.gui.action("iop/exposure/exposure")`) are language-independent, but **rewording an English label silently breaks user shortcuts and scripts**.
- **Numbers.** Numbers in files are parsed in the C locale (`g_ascii_strtod`, `uselocale`), and typed values accept both `,` and `.` (`src/common/calculator.c`). Nothing is formatted per locale.
- **Changing the language requires a restart** (`src/gui/preferences.c`).

## tvty

**Libraries and files:**
- `fluent-bundle` with `unic-langid`, `sys-locale` and `rust-embed`;
- 4 languages, one `.ftl` file per UI surface;
- flat kebab-case keys prefixed by surface, attributes for secondary text (`.about`, `.on`).

**Ids and fallback:**
- **Stable ids stay English.** A command's label key is derived from its id (`theme.next` gives `keys-theme-next`), and a setting's from its key (`setting-<key>`). A missing key falls back to the English text written in the code.
- **Fallback order** is the current language, then English, then the id, with a warning.

**Choosing and switching the language:**
- `TVTY_LANG`, then the setting, then the system language.
- The language switches at runtime: an atomic index and a repaint.
- Tests pin English.

**Tooling:**
- `tvty i18n` reports each language's coverage, missing and left-over keys, and how many strings are still hard-coded.
- CI fails on real faults only: an unparsable file, a key defined twice, an id English lacks.
- Two tests check that every looked-up id exists in English:
  - `every_word_asked_for_exists` scans the sources;
  - `every_built_word_exists` lists the ids built at runtime by hand.

**Remote text.** A remote service sends codes and facts, not English sentences, so the client phrases them in its language.

**What hurt:**
- no compile-time key checks;
- the hand-kept list of runtime-built ids;
- tuning the hard-coded-string scanner;
- a Spanish label that covered a setting's name.

## What Helios takes

1. **Ids are declared, never derived from labels.** This is darktable's
   lesson. Command ids, panel ids, value paths and choice values stay
   English and stable. Agents, scripts, keymaps, profiles and the value
   store only see them.
2. **Message ids are derived mechanically from those ids**, as in tvty:
   - `command-<id>` with `.title`;
   - `panel-<id>`;
   - `param-<module>-<name>` with `.label`, `.description` and `-choice-<value>`.

   English stays in the code (the command spec, the manifest), so a
   missing translation shows English, never an id.
3. **The ids built at runtime need no hand-kept list.** The registry, the
   panels and the manifests already list every command, panel and
   parameter. A test walks them and checks every derived id against the
   English catalog, which is generated from the same declarations.
4. **Agents get codes and facts.** Notices and errors that reach agents
   carry a code and arguments; the GUI phrases them.

## Recommendation

1. **Fluent, through `fluent-bundle`**, behind a small `helios-i18n`
   crate:
   - one bundle per locale, plus each plugin's catalog added under its id
     prefix;
   - fallback chain `fr-CA` → `fr` → `en` → the English in the code;
   - isolation marks off (egui would draw them);
   - runtime switching with a repaint.

   i18n-embed's compile-time `fl!` is not needed for ids derived from
   declarations. For the shell's few free strings, a `t!` macro checked
   by a test is enough.
2. **The English catalog is generated** from the declarations
   (`helios i18n extract`). Translators work on `.ftl` files in the
   repository, through **Weblate** (free for libre projects, native
   Fluent support). Pontoon is the other option.
3. **Tooling:**
   - `helios i18n check` lists, per locale, coverage, missing and orphan
     keys, and keys whose English changed since they were translated;
   - CI fails on unparsable files, duplicates and ids missing in English;
   - coverage is a report, not a failure;
   - a **pseudo-locale** (`fluent-pseudo`: accents, about 40% longer) is
     run by the GUI snapshot tests to catch layouts that do not absorb
     long strings.
4. **Formatting:**
   - values keep their raw numbers; only widgets format them, through
     ICU4X decimal and plural, with a data blob generated for the shipped
     locales;
   - units go through a Helios wrapper while ICU4X's are experimental;
   - typed entry accepts `,` and `.` as darktable's calculator does;
   - files and the protocol stay locale-independent.
5. **Locale choice:**
   - `HELIOS_LANG`, then the setting (a profile part with machine scope),
     then `sys-locale`;
   - tests pin English.
6. **Right-to-left:** designed for (logical start and end, no
   concatenated sentences), shipped when egui's bidi lands. CJK needs
   registered fallback fonts until egui 0.37.
7. **Languages at 1.0:** English and French, others as contributors bring
   them.

**Not chosen:**
- **gettext:** the English text is the key, so rewording invalidates
  translations, the same coupling darktable suffers from for ids. It also
  brings process-wide locale state.
- **rust-i18n:** no CLDR plurals.
- **MessageFormat 2:** not shipped in Rust. Fluent is its close relative,
  so a later move stays possible.
