# darktable pitfalls: UI, platforms and project process

Scope: UI/UX, input devices, the GTK4 migration, HiDPI, accessibility,
translation, Windows/macOS/Wayland, packaging, default-behaviour changes and
process (review load, AI-assisted PRs, release-critical regressions).
Sources: darktable-org/darktable issues and PRs read with `gh` on
2026-10-12 (label sweeps sorted by comments, then threads read). Helios
exposure is judged against ROADMAP.md, docs/gui.md, docs/shortcuts-research.md,
docs/i18n-research.md, docs/testing-strategy.md and egui-pop's README.
Statements marked [guess] are inference, not read in a thread.

---

### 1. Toolkit migration done in master with no GUI test harness
- **What went wrong.** The GTK3 to GTK4 port had stalled for years (a
  tracking issue since 2023, an RFA branch abandoned by its author in 2025).
  In 2026 it went into master as a stream of "gtk4-prep" PRs, largely
  written by one contributor with AI help. Regressions followed in basic
  interactions: rating stars, selection, mouse wheel, cursor shape, preset
  menus. A member opened an RFC asking to revert it to a branch. The thread
  says the root cause is that darktable has **no automated UI testing**
  ("all the testing is user intensive and experience driven"; Lua actions
  cannot emulate mouse interactions). Maintainers kept it in master because
  only master gets enough testers, and accepted that the next release may
  slip.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/21984 ,
  https://github.com/darktable-org/darktable/issues/19448 ,
  https://github.com/darktable-org/darktable/issues/20433 ,
  https://github.com/darktable-org/darktable/issues/21885 ,
  https://github.com/darktable-org/darktable/issues/21624
- **Helios exposure: partly handled.** egui-pop ships a headless test kit,
  AccessKit labels and "every interactive node has a name". But egui and
  wgpu make breaking releases often (Helios pins egui 0.36 / wgpu 30), and
  the hardest regressions above came from *pointer* paths: hover, click,
  wheel and gestures.
- **Recommendation.** Do every egui/wgpu upgrade on a branch, gated by a
  scripted GUI scenario suite that injects **raw pointer, wheel and key
  events** (egui `RawInput`), not only command calls. It should cover rating,
  selection, slider drag and wheel, menus, dialogs and docking. Keep egui
  types behind egui-pop's API so an upgrade touches one crate.

### 2. AI-generated commit series break `git bisect`, and review cannot keep up
- **What went wrong.** Large PRs with many intermediate commits, believed to
  be AI-generated, were merged although only the last commit compiled.
  Bisecting regressions then ran into five or more unbuildable commits in a
  row. Separately, maintainers report that list traffic rose from about 5
  to 50+ emails a day. One wrote that GTK4 work "is pretty much a one man
  operation and the only one that may understand it is the AI", and that
  "my AI may not see what your AI sees". darktable answered with an
  `ai:generated` label and an `AGENTS.md` whose first rule is "every commit
  must compile on its own" and which requires disclosing AI assistance.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/21881 ,
  https://github.com/darktable-org/darktable/issues/21984 ,
  https://github.com/darktable-org/darktable/pull/22483 ,
  https://github.com/darktable-org/darktable/blob/master/AGENTS.md
- **Helios exposure: at risk.** Helios is developed with AI assistance, and
  its rule "every commit building and passing its tests" is a convention,
  not a check.
- **Recommendation.** Add CI that builds and tests **each commit** of a
  push (`git rebase --exec`, or a per-commit job). Keep design notes that
  explain internals in prose, so a human or another agent can review
  without re-deriving them. Record, in the commit body or PR, what was run
  and what was not (darktable's AGENTS.md asks for exactly this).

### 3. Release-critical regressions in edit persistence
- **What went wrong.** Refactors merged in master silently broke edit
  persistence. In one case the history end was not remembered across a
  restart and history above it was lost; it was bisected to a large
  refactor. In another, history clicks and slider changes only sometimes
  reached the main view. In a third, old crop presets crashed the app at
  startup. Users found all three while editing, and a bisect by a user was
  needed each time.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/21943 ,
  https://github.com/darktable-org/darktable/issues/21185 ,
  https://github.com/darktable-org/darktable/issues/19958 ,
  https://github.com/darktable-org/darktable/issues/20193
- **Helios exposure: at risk.** The value store, undo and the vault are new
  code, and agents will refactor them often.
- **Recommendation.** Add round-trip tests as release gates: edit, set the
  undo position, quit, reopen, compare the state byte for byte. Load old
  presets and old documents from a frozen corpus. Treat any loss of user
  edits as a stop-ship label from day one.

### 4. Edits lost on crash: no explicit save, no journal
- **What went wrong.** darktable writes history to the database and XMP
  only when the user leaves the image or the view. A crash loses the whole
  session ("an hour or two" of work). Maintainers disagreed: fix the crashes
  rather than work around them, distrust heuristics, keep "no more colour in
  the UI". They also raised the cost of fsync.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/7830 ,
  https://github.com/darktable-org/darktable/issues/21943
- **Helios exposure: at risk until designed.** The value store has undo,
  but nothing read says when a gesture becomes durable.
- **Recommendation.** Append every committed gesture (one undo step) to a
  journal right away, and fsync on gesture end or on a short timer.
  Replay the journal on the next start and say so in a notice. This also
  serves agents' writes and remote clients.

### 5. Keyboard focus lost or stolen, so typing fires shortcuts
- **What went wrong.** During a long import, each imported image refreshed
  the GUI and took focus away from the module search entry. The user's
  typing then fired single-key shortcuts, which a reporter called
  "dangerous" for edits. Other reports: Ctrl+T opens the tag entry without
  focus (on Wayland), and the module search keeps focus after a module is
  chosen.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/7111 ,
  https://github.com/darktable-org/darktable/issues/14729 ,
  https://github.com/darktable-org/darktable/issues/13180 ,
  https://github.com/darktable-org/darktable/issues/4717
- **Helios exposure: mostly handled by design.** There are `when` clauses
  with `textInputFocus`, a single letter never fires in a text field, and a
  notice never takes focus. Still at risk from background updates (agents,
  imports, remote edits) that rebuild widgets and lose egui focus ids
  [guess].
- **Recommendation.** Make it a rule, with a test, that nothing but a user
  gesture moves keyboard focus. Keep widget ids stable across rebuilds.
  Run a test that types into a text field while an import and an agent
  write are in progress.

### 6. Keymaps across layouts and systems (keycodes, layouts, Cmd)
- **What went wrong.** darktable resolves keys through GDK keymaps. Under
  GNOME/Wayland with two layouts it used the *first* layout, so Colemak
  users got the wrong actions; its author explained that the design goals
  ("fallbacks") differ from GTK's and admitted "this seems to work like I
  want in most of the scenarios I considered". On Windows, a three-modifier
  combination was recorded with one modifier missing. On macOS, Ctrl was
  hard-coded (`GDK_CONTROL_MASK` everywhere) instead of Cmd, until the input
  system was rewritten (InputNG, 115 comments). After that rewrite, users
  had to redo their custom shortcuts.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/17968 ,
  https://github.com/darktable-org/darktable/issues/10729 ,
  https://github.com/darktable-org/darktable/issues/4384 ,
  https://github.com/darktable-org/darktable/pull/8078
- **Helios exposure: partly handled.** `Mod` maps to Ctrl or Cmd, and "keys
  are logical characters". But logical characters fail on non-Latin layouts
  (Cyrillic, Greek): `Ctrl+C` then gives a Cyrillic character [guess,
  general knowledge, not read in darktable].
- **Recommendation.** Match on the logical key first. For shortcuts with
  modifiers, fall back to the physical key when the logical one is not
  Latin; egui exposes `physical_key` [guess, check against egui 0.36]. Run
  tests with AZERTY, Colemak, Russian and a layout switch. Version the
  keymap file and migrate user bindings on every change; never ask users to
  redo them.

### 7. The wheel changes slider values while the user wants to scroll the panel
- **What went wrong.** The default "scroll over a widget changes its value"
  is called the feature that annoys users most. Users also change
  parameters without noticing, which then shows up as support cases ("a
  parameter they claim they did not change"). Lightroom scrolls the panel,
  and a value only takes the wheel after a click. The PR that changed the
  default stalled (45 comments). The objections: existing users' habits,
  and a "make it a preference" reflex.
- **Evidence.**
  https://github.com/darktable-org/darktable/pull/16869
- **Helios exposure: at risk, directly.** docs/gui.md, "Parameters", says
  "the wheel and the arrows step" on a slider.
- **Recommendation.** By default, the wheel scrolls the panel. A slider
  takes the wheel only once it has focus (click or keyboard), or while a
  modifier is held. Decide this before users get used to the other
  behaviour. The diamond and dot markers already show changed values, which
  helps.

### 8. Wheel, smooth scroll, touchpad and pinch confused
- **What went wrong.** On macOS, the Magic Mouse and trackpad made
  lighttable scrolling "nearly unusable" after one release, while a classic
  wheel worked. Pinch-to-zoom was missing for years. Once it was added, it
  did not work under the AppImage on X11 (two-finger scroll zoomed instead).
  One fix had to stop ordinary mouse presses from being classed as touchpad
  gestures, which "could poison scroll routing for the rest of the
  session".
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/6068 ,
  https://github.com/darktable-org/darktable/issues/18048 ,
  https://github.com/darktable-org/darktable/issues/21145 ,
  https://github.com/darktable-org/darktable/pull/21945
- **Helios exposure: at risk.** The viewport (pan and zoom) and the graph
  view (semantic zoom) both depend on this.
- **Recommendation.** Handle line deltas and pixel deltas separately, and
  pinch through egui's zoom delta. Make Ctrl+wheel zoom and a plain wheel
  pan or scroll. Keep no hidden per-session device state. Add a test that
  replays recorded event streams from a wheel, a Magic Mouse, a Windows
  precision touchpad and a Linux touchpad.

### 9. "Images to act on": hover against selection
- **What went wrong.** Actions apply to the hovered image or to the
  selection depending on context. This has "surprised users since 2009"
  (a 100-comment issue, then a compendium). Pastes went to the wrong images,
  export followed the active image, and a mouse movement changed the
  selection. The rule that emerged: "hover to see information, click to
  make changes". Changes to it kept being labelled default-behaviour
  changes.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/6025 ,
  https://github.com/darktable-org/darktable/issues/16850 ,
  https://github.com/darktable-org/darktable/issues/21885 ,
  https://github.com/darktable-org/darktable/pull/22285
- **Helios exposure: at risk.** Commands come from keys, menus, the
  palette and agents. An agent has no hover, so the targets must be
  explicit.
- **Recommendation.** A command that changes data takes an explicit target
  set (the selection, or a list of ids), never the hover. Hover only shows
  information. The status bar shows how many items the next action will
  touch.

### 10. HiDPI: pixel ratio per app instead of per window, and what "100%" means
- **What went wrong.** On 4K screens, lighttable previews were rendered at
  half resolution (65 comments; the log shows 96 dpi instead of about 192).
  A second darkroom window on a screen with another pixel density showed
  only a quarter of the image, because the ppd came from the main window. A
  mixed 1x/2x setup on Wayland still renders at low resolution (open). The
  100% zoom on HiDPI went to 200% by design. A PR adding a "real 1:1"
  shortcut was refused, and the discussion turned on "dots or display
  pixels".
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/5947 ,
  https://github.com/darktable-org/darktable/issues/7910 ,
  https://github.com/darktable-org/darktable/issues/20297 ,
  https://github.com/darktable-org/darktable/issues/10839 ,
  https://github.com/darktable-org/darktable/pull/11179
- **Helios exposure: at risk.** Auxiliary windows on other screens are a
  decided feature, and the viewport asks for tiles.
- **Recommendation.** Read `pixels_per_point` per viewport every frame, and
  request tiles in physical pixels of the window that shows them. Define
  100% once: one image pixel per physical screen pixel. Label it in the
  zoom control. Test 1x+2x and fractional (1.25, 1.5) screens with a panel
  moved between them.

### 11. Wayland: dialogs, fractional scaling, multi-monitor
- **What went wrong.** On GNOME Wayland with more than one monitor and
  attached modal dialogs, the preferences and copy dialogs "cannot be
  interacted with most of the time". Fractional scaling crashed the app in
  dropdowns (the Flatpak build fell back to native Wayland). A maintainer
  noted that XWayland is "the preferred way". Other open items: Wayland
  freezes, drag and drop on Plasma, a top bar under tiling window managers,
  and HDR/colour management on Wayland.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/16428 ,
  https://github.com/darktable-org/darktable/issues/18009 ,
  https://github.com/darktable-org/darktable/issues/19441 ,
  https://github.com/darktable-org/darktable/issues/20688 ,
  https://github.com/darktable-org/darktable/issues/19435
- **Helios exposure: partly handled.** Helios draws its own dialogs inside
  its windows, so there are no OS modals, and winit is Wayland-native. Its
  auxiliary windows and saved screen positions are at risk: Wayland does
  not let a client position its own windows [guess, general knowledge].
- **Recommendation.** Keep a CI or wbox matrix with GNOME and KDE on
  Wayland, fractional scaling, two monitors and monitor unplug. Restore the
  layout without relying on absolute window positions on Wayland. Track
  display colour management separately (see the colour theme).

### 12. Windows: console windows, focus stealing, non-ASCII paths
- **What went wrong.** On Windows, every external command run from Lua
  flashes a console window that takes focus. The issue has been open since
  2024, and the reporter showed the same thing with Rust's `mlua`
  `os.execute`, while Rust's `Command` does not. Setting the display
  profile on Windows called code that brought darktable to the front
  whenever the mouse passed over a thumbnail, and it broke Alt+Tab
  (bisected to a colour management fix). Release 3.4 crashed at startup
  for users whose profile path had a non-ASCII character
  (`C:\Users\Börje`).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/17193 ,
  https://github.com/darktable-org/darktable/issues/20442 ,
  https://github.com/darktable-org/darktable/issues/7489 ,
  https://github.com/darktable-org/darktable/issues/8782
- **Helios exposure: at risk.** There will be plugins, external tools and
  ICC display profiles on Windows.
- **Recommendation.** Build with `windows_subsystem = "windows"`. Start all
  child processes through one helper that sets `CREATE_NO_WINDOW`, and
  never let plugins call a shell. Never raise or focus the window from
  background code. Keep paths as `OsString`/`PathBuf` end to end, and run
  CI with a non-ASCII user profile and a path containing spaces.

### 13. Fonts and glyph coverage differ per OS
- **What went wrong.** The default theme rendered Cyrillic text with
  monospace-like spacing on Windows only; it was a regression between two
  releases and could not be reproduced on Linux. Windows also showed font
  problems in the elegant themes' menus and "super tiny" icons. A crash
  occurred after a system font was disabled.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/13084 ,
  https://github.com/darktable-org/darktable/issues/7659 ,
  https://github.com/darktable-org/darktable/issues/4005 ,
  https://github.com/darktable-org/darktable/issues/11778
- **Helios exposure: at risk, in a different way.** egui draws with its own
  embedded fonts, which avoids OS font lookup. But the default fonts do not
  cover CJK and other scripts [guess, check egui 0.36].
- **Recommendation.** Ship fonts with known coverage for every shipped
  locale, with a fallback chain declared in the theme (`font.family.ui`).
  Add a pseudo-locale and screenshot tests per script (Latin, Cyrillic,
  CJK, Arabic) at both densities.

### 14. Locale handling: global `setlocale` races, wrong default language, language switch failures
- **What went wrong.** Parts of the GUI sometimes started in English
  because OpenCL initialisation, running in parallel, reset the
  process-wide locale to "C" (a workaround for an AMD driver). As a
  maintainer noted, `setlocale` is not thread safe and `uselocale` does not
  exist on Windows. A fresh macOS install defaulted to Russian after locale
  logic was removed for Catalina. On Windows, switching language in the
  preferences does nothing (open). The Windows installer did not ship
  GLib's translations.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/18196 ,
  https://github.com/darktable-org/darktable/issues/4038 ,
  https://github.com/darktable-org/darktable/issues/20194 ,
  https://github.com/darktable-org/darktable/issues/9433
- **Helios exposure: mostly handled.** Helios chose Fluent with explicit
  bundles (`egui-pop-i18n`) and no gettext or `setlocale`. Numbers are to
  be parsed independently of the locale.
- **Recommendation.** Never call `setlocale`, including in FFI crates.
  Note that the darktable bridge links C code that may [guess]. Resolve the
  language with a tested order: user setting, `sys-locale`, then English.
  Show the resolved language in the about box. Add a CI test that switches
  language at runtime on each OS.

### 15. String style decided by taste: all-lowercase English
- **What went wrong.** darktable's labels are all lowercase by design. A
  request to capitalise them was rejected by the lead ("this makes the text
  unreadable to me"). The compromise was a separate "English (true case)"
  catalogue that has to be kept in sync. Users still struggle with it on
  Windows ("all those texts in lower case make it really hard to use").
  Related churn: the 'gray' spelling and the bullet between module and
  instance name.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/2078 ,
  https://github.com/darktable-org/darktable/issues/20194 ,
  https://github.com/darktable-org/darktable/issues/12036 ,
  https://github.com/darktable-org/darktable/issues/13989
- **Helios exposure: not decided** (nothing read on casing in the Helios
  docs).
- **Recommendation.** Write a short string style guide now: sentence case,
  US spelling, verbs on buttons (gui.md already wants "Delete 3 images").
  Check it with a lint on the English catalogue, so casing is never a
  second catalogue.

### 16. macOS: one maintainer, unsigned builds, and Homebrew dropping the app
- **What went wrong.** For about ten years, one volunteer built the macOS
  packages on hardware stuck at Xcode 12.4. When he stepped back, a
  145-comment meta-bug threatened to drop macOS unless someone took the
  role. Builds were never signed or notarised, so Homebrew deprecated the
  cask ("does not pass the macOS Gatekeeper check", disabled 2026-09-01).
  The reply was "none of those who contribute to darktable have an Apple
  Developer account", and a third party now keeps the cask. macOS updates
  also break the toolkit: on Ventura, preset menus did not take mouse focus
  (a GTK bug the project could not fix itself).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/13509 ,
  https://github.com/darktable-org/darktable/issues/19982 ,
  https://github.com/darktable-org/darktable/issues/12727 ,
  https://github.com/darktable-org/darktable/issues/15922
- **Helios exposure: at risk.** macOS is a hard requirement (Metal through
  wgpu), and there is a single developer.
- **Recommendation.** Build signed and notarised macOS packages in CI from
  the first public binary. The Apple Developer account must belong to the
  project, not to a person. Document the release steps so an agent can run
  them. Test each new macOS beta in CI. Decide the minimum macOS version
  explicitly.

### 17. Linux packaging: AppImage, snap, Flatpak and distro toolchains
- **What went wrong.** The AppImage looked for Lua scripts under its
  temporary mount path, and it forced X11 so touchpad gestures did not
  work. Snap users on 8 GB machines had frequent out-of-memory kills, and
  the advice was to "use a deb instead of snap". A Fedora toolchain update
  made a module fail with a missing vectorised `sqrtf` symbol.
  Flatpak-specific fallbacks reached crashing code paths (see the Wayland
  pitfall).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/17632 ,
  https://github.com/darktable-org/darktable/issues/21145 ,
  https://github.com/darktable-org/darktable/issues/13464 ,
  https://github.com/darktable-org/darktable/issues/18729 ,
  https://github.com/darktable-org/darktable/issues/3925
- **Helios exposure: at risk later.** Rust's static linking helps, but
  wgpu/Vulkan loaders, portals (rfd uses the XDG portal) and data paths
  stay.
- **Recommendation.** Pick one or two official Linux formats (Flatpak and
  AppImage). Build them in CI and run the GUI scenario suite **inside the
  packaged artefact**. Resolve resources relative to the executable or
  through XDG, never to the build path. Leave other formats to
  distributions.

### 18. Default-behaviour changes and redesigns that anger users
- **What went wrong.** The two-step import that replaced the "import
  files/folder" buttons drew an "improve or revert" issue. The lead kept
  it, then the first stage was removed. Changes to filmstrip clicks,
  scopes, "act on" and copy/paste of module order each needed RFCs. A large
  tone equalizer redesign silently broke existing presets (a missing
  version bump). Its pixls.us thread "became so heated that it has almost
  no value", and the PR was closed after 204 comments.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/8596 ,
  https://github.com/darktable-org/darktable/pull/8734 ,
  https://github.com/darktable-org/darktable/pull/18656 ,
  https://github.com/darktable-org/darktable/pull/22483 ,
  https://github.com/darktable-org/darktable/issues/16969
- **Helios exposure: at risk later.** Helios explicitly diverges from
  darktable's UI, which users will compare it with.
- **Recommendation.** Label default changes (darktable's
  `default-behavior-change`). Each one needs a release note, a migration
  that keeps existing users' effective behaviour, and a dated decision in
  the ROADMAP. Every parameter format change must bump the version and ship
  a tested upgrade path. Prototype redesigns behind a setting, and judge
  them with a short user test, not in forum threads.

### 19. Preferences bloat, and "make it a preference" as the default compromise
- **What went wrong.** Preferences grew every release. A cleanup issue
  found unused settings and scattered module-specific ones. An "expert
  mode" was rejected as "bad options design". When defaults are disputed,
  the reflex is to add a preference, yet "only a tiny minority of users ever
  open a preferences dialog".
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/7050 ,
  https://github.com/darktable-org/darktable/pull/16869 ,
  https://github.com/darktable-org/darktable/pull/11167
- **Helios exposure: partly handled.** Settings pages are generated from
  descriptors, show where each value comes from, and profiles can be
  exported.
- **Recommendation.** Every setting declares an owner and a reason, and a
  test lists settings that no code reads. Prefer one good default plus a
  command over a new preference. Make the generated settings searchable
  from the palette.

### 20. Long background work blocks the UI or gives no feedback
- **What went wrong.** The darkroom was "unusable during import": thumbnails
  flickered and focus was lost, because of image locks and signal storms.
  The new import dialog took about 10 s to appear on Windows with no
  feedback while it walked folders. Lighttable rating and filtering were
  very slow with large collections.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/7111 ,
  https://github.com/darktable-org/darktable/issues/8782 ,
  https://github.com/darktable-org/darktable/issues/9042 ,
  https://github.com/darktable-org/darktable/issues/4114
- **Helios exposure: mostly handled.** Helios has an engine and server
  split, windowed collection fetches, and queues with visible state
  (docs/architecture.md, gui.md queue panel).
- **Recommendation.** Make it a rule that a dialog or panel opens in one
  frame and fills progressively. Add frame-time budgets to the GUI
  scenario suite with 100k images and an import running.

### 21. Theme vs system appearance, and accessibility as an afterthought
- **What went wrong.** Users ask darktable to follow the system colour
  scheme, high contrast included. Maintainers refuse because the neutral
  UI avoids "false color perception", and say a high-contrast theme was
  added only recently. Other reports say the default theme's contrast is
  too low and a camera style is unreadable. darktable has no
  screen-reader story in any thread read.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/17378 ,
  https://github.com/darktable-org/darktable/issues/20146 ,
  https://github.com/darktable-org/darktable/issues/18237
- **Helios exposure: handled.** There are token themes including high
  contrast, a `viewport.surround` token kept neutral, AccessKit, and
  contrast checks in egui-pop-tokens.
- **Recommendation.** Follow the system's light, dark and high-contrast
  preference for the UI chrome only. Keep the image surround neutral
  whatever the theme. Keep the contrast checks as CI failures.

---

## Top 5 for Helios now

1. **Change the slider wheel rule in docs/gui.md before it ships**
   (pitfall 7). The wheel scrolls the panel; a slider takes the wheel only
   when focused or with a modifier. This is darktable's most complained
   default, and Helios currently copies it.
2. **Enforce commit hygiene for AI-assisted work in CI** (pitfall 2).
   Build and test each commit of a push, disclose AI assistance and what
   was run, and keep internals documented, so `git bisect` and outside
   review stay possible.
3. **Write a GUI scenario suite that injects raw pointer, wheel and key
   events** before the first egui/wgpu upgrade (pitfalls 1, 5, 8).
   darktable's GTK4 port went bad mostly because nothing tested the
   interactions themselves.
4. **Make edits durable and test it** (pitfalls 3, 4). Journal each
   gesture, replay it after a crash, and gate releases on
   quit-and-reopen round trips and an old-document corpus.
5. **Settle platform basics early.** Read the pixel ratio per window and
   define 100% as one image pixel per physical pixel (pitfall 10). Sign and
   notarise macOS builds in CI with a project-owned account (pitfall 16).
   On Windows, start child processes with no console and never take focus
   from background code (pitfall 12).
