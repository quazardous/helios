# User profiles and themes: research

Status: **research** (2026-10-11). It answers: how should Helios's themes
work, what does a user profile hold, and how does it move between machines
without carrying what it must not. Claims come from official documentation
where it exists, otherwise from the source code (VS Code on GitHub,
darktable's tree); those that could not be confirmed are marked
*unverified*. darktable paths are relative to its source tree.

## 1. Themes

### VS Code

- **Format.** A theme is a JSON file. It holds `colors` (workbench colour
  ids to hex), `tokenColors` (TextMate rules), `semanticHighlighting` and
  `semanticTokenColors`. Naming it `*-color-theme.json` gives completion
  and colour pickers while editing
  ([guide](https://code.visualstudio.com/api/extension-guides/color-theme)).
- **Contribution.** An extension declares themes in `contributes.themes`
  with a `label`, a `path` and a `uiTheme`. `uiTheme` is the base around
  the editor: `vs`, `vs-dark`, `hc-black` or `hc-light`.
- **Preview.** The theme picker previews each theme as the arrows move.
  "Generate Color Theme from Current Settings" turns overrides into a theme
  file ([themes](https://code.visualstudio.com/docs/configure/themes)).
- **Overrides per theme.** `workbench.colorCustomizations` accepts keys
  scoped to theme names (`"[Monokai]"`, `"[*Dark*]"`). The OS light or
  dark scheme selects a preferred theme of each kind.
- **No documented inheritance.** The loader has an undocumented `include`
  key, which reads a file of the same extension first and applies the
  theme over it (`colorThemeData.ts`). A theme cannot extend a theme from
  another extension. In practice people fork a theme or override it in
  settings.

### egui (0.36)

- **`Style`** holds text styles (`TextStyle` to `FontId`), spacing,
  interaction, animation times and `visuals`
  ([docs](https://docs.rs/egui/latest/egui/style/struct.Style.html)).
- **`Visuals`** holds about 36 fields
  ([docs](https://docs.rs/egui/latest/egui/style/struct.Visuals.html)):
  - `dark_mode`;
  - widget states (inactive, hovered, active, open);
  - `selection`;
  - fills and strokes (panel, window, faint and extreme backgrounds);
  - hyperlink, warning and error colours;
  - shadows and corner radii;
  - flags such as `striped` or `handle_shape`.
- **Dark and light.** `Options` keeps a dark and a light `Style`, and a
  `ThemePreference` (dark, light, follow the system); `sync_window_theme`
  also switches the window decorations.
- **Fonts.** `FontDefinitions` maps families to font data with fallback
  order. `set_zoom_factor` scales the whole interface.
- **Serialization, and its limit.** These types serialize with serde, but
  their fields change between egui versions. A theme saved as raw
  `Visuals` would break on an egui upgrade.

### darktable

- **Structure.** Themes are GTK CSS. `data/themes/darktable.css` (about
  2,800 lines) holds the rules; each named theme imports it and redefines
  about 100 `@define-color` variables. This is inheritance through CSS
  imports and named colours.
- **Load order** (`src/gui/gtk.c`): the theme, optional chunks from the
  preferences (condensed, rounded headers, accent colour, module borders),
  an OS file, then the user's `user.css` when enabled.
- **Preferences** offer the theme, the font size (or the system's), the
  DPI, and a CSS editor writing `user.css`. The manual warns that custom
  theme files "can break between versions".
- **What users complain about and edit.** It is mostly type, not colour:
  - font size, weight and family order: in 5.4 secondary text became hard
    to read, and was fixed in `user.css`
    ([thread](https://discuss.pixls.us/t/5-4-module-name-readability/55000));
  - DPI;
  - the contrast of secondary text;
  - an accent colour;
  - module borders.

  Other problems reported:
  - custom themes whose `@import` fails from the user's directory;
  - graph backgrounds that cannot be themed;
  - stale CSS breaking the interface after an upgrade
    ([thread](https://discuss.pixls.us/t/my-personal-darktable-theme/28942)).
- **Community themes** must be maintained at every release.

### Blender and design tokens

- **Blender** saves and installs themes as XML presets, organized per
  editor ([manual](https://docs.blender.org/manual/en/latest/editors/preferences/themes.html)).
  Its XML schema was not checked here (*unverified*).
- **Design Tokens (DTCG, 2025.10)** is a JSON format from a W3C Community
  Group ([format](https://www.designtokens.org/tr/drafts/format/)).
  - A token has a `$value`, and may have a `$type` (inherited from its
    group) and a `$description`.
  - Groups may `$extends` another group (deep merge).
  - Aliases are written `{group.token}`; cycles are errors.
  - Types: colour, dimension, font family, font weight, duration, number,
    and composites (border, shadow, typography).
  - Unknown data goes in `$extensions` under reverse-DNS keys, which tools
    must keep.

### Lessons

- **Tokens, not toolkit state.** A theme is a small set of semantic tokens
  (`bg.panel`, `fg.weak`, `accent`, `state.failed`…). Code maps them onto
  `egui::Visuals`; the GUI never uses a literal colour. DTCG already gives
  a format with aliases and inheritance.
- **Real inheritance.** A theme extends a base and overrides some tokens.
  darktable gets this from CSS; VS Code only within one extension.
- **A supported override layer.** User overrides live apart from the
  theme, like VS Code's `[Theme]` keys or darktable's `user.css`. They are
  validated, unknown keys are reported, and a missing token falls back to
  the base instead of breaking.
- **Type and scale come first.** Font size, weight, family fallbacks and
  interface scale are settings independent of the colour theme, since
  that is what most darktable complaints are about.

## 2. What a profile holds

### VS Code profiles

From the [profiles docs](https://code.visualstudio.com/docs/configure/profiles):

- **Contents.** A profile holds settings, keybindings, snippets, tasks, MCP
  servers, extensions and UI state (view layout and visibility). A profile
  may hold only some parts; the rest comes from the **Default profile**.
  Profiles do not inherit from each other.
- **Export and import.** A profile exports to a `.code-profile` file or a
  secret gist. Import previews its contents and lets parts be deselected.
  Settings scoped to the machine are never exported.
- **Folders.** A folder or workspace remembers its profile, and opening it
  switches to that profile. A setting or extension can be applied to all
  profiles.

### VS Code Settings Sync

From the [Settings Sync docs](https://code.visualstudio.com/docs/configure/settings-sync):

- **What syncs.** Settings, keybindings (per OS by default), snippets,
  user tasks, MCP servers, UI state, extensions with their enabled state,
  and profiles.
- **What does not.** Settings scoped `machine`, ignored settings and
  extensions, workspace tasks, and extensions in remote windows. A setting
  can opt out in its declaration.
- **Conflicts.** The first sync merges. When merging fails, sync stops and
  offers keep local, keep remote, or a diff to merge by hand.
- **History and backends.** The last 20 versions per part are kept in the
  cloud. Only Microsoft and GitHub accounts are offered, with no custom
  backend.

### Lightroom and Capture One

- **Lightroom Classic.**
  - Presets and profiles are `.xmp` files (older presets
    `.lrtemplate`), imported from the Presets panel
    ([FAQ](https://helpx.adobe.com/lightroom-classic/kb/presets-faq.html)).
  - Classic does not sync them: users copy the settings folder by hand.
  - It may store presets with the catalog (*unverified*).
- **Lightroom (cloud)** imports presets and profiles, then syncs them to
  every device of the same account
  ([help](https://helpx.adobe.com/au/lightroom/desktop/lightroom-for-mobile-tv-and-the-web/add-sync-mobile-presets.html)).
- **Capture One.**
  - Styles are `.costyle` files and packs are zips; an ICC profile a style
    uses must be installed apart
    ([help](https://support.captureone.com/hc/en-us/articles/360002620358-Importing-a-Styles-Pack)).
  - Workspaces have no export: users copy the folder.
  - Old threads report shortcut sets that did not move between macOS and
    Windows (plist against XML); their current state is *unverified*.

### darktable

- **The configuration directory** holds `darktablerc`, `data.db`,
  `library.db`, `shortcutsrc`, `user.css`, `styles/` and Lua scripts
  ([manual](https://docs.darktable.org/usermanual/development/en/preferences-settings/config-directory/)).
- **The two databases** (`src/common/database.c`):
  - `data.db` holds tags, styles, presets and locations;
  - `library.db` holds images, history and tag assignments.

  So the tag vocabulary is shared between libraries and the assignments
  are not.
- **`darktablerc`** is one flat file mixing three kinds of keys:
  - real preferences;
  - UI state (hundreds of `ui_last/*` and `plugins/*` keys);
  - machine tuning (OpenCL, resources, DPI, last paths).
- **Export and import.** Styles go out as `.dtstyle`, presets as
  `.dtpreset`. Shortcuts export per device (keyboard, MIDI, controller).
- **Moving to another machine.** There is no procedure: the advice is to
  copy the directory and relink photos
  ([thread](https://discuss.pixls.us/t/two-computers-two-libraries-questions/34951)).

### Lessons

- **Selectable parts and a Default.** A profile is a set of parts selected
  one by one:
  - settings;
  - keymap layers;
  - theme and overrides;
  - panel layouts;
  - plugins;
  - export targets;
  - AI and workshop preferences.

  A Default profile holds what a profile leaves out, as in VS Code. The
  photo library stays out of profiles.
- **Scopes.** Every setting declares its scope (`user`, `machine`,
  `workspace`), and plugins declare theirs in their manifest. One flat file
  must not mix preferences, interface state and machine tuning.
- **Workspaces point at a profile.**
- **Text and per-part export.** Each part is readable text and exports on
  its own; conflicts show as a diff.
- **Keys across systems.** Keymaps keep platform differences explicit:
  abstract modifiers, or per-OS layers.

## 3. Moving a profile safely

- **Secrets in VS Code.** Extensions keep secrets in `SecretStorage`,
  encrypted per platform and never synced (`vscode.d.ts`). Plain extension
  state syncs only for keys an extension opts in.
- **Machine settings.** Profile export removes every setting scoped to
  the machine (`settingsResource.ts`).
- **Extensions are references.** A profile lists an extension by
  identifier, enabled state, pre-release flag and, only when the user
  pinned it, its version (`extensionsResource.ts`). Without a pin, import
  installs the latest version, so two imports can differ.
- **darktable** keeps export passwords (Piwigo…) in the OS keyring
  (libsecret, KWallet, Keychain, Windows credentials) through
  `src/common/pwstorage/`, never in `darktablerc`.
- **Incident.** In October 2025, Wiz found over 550 valid secrets in
  published VS Code extensions, mostly from bundled `.env` and AI
  configuration files. Among them were marketplace tokens able to push
  malicious updates. Secret scanning before publication followed
  ([report](https://www.wiz.io/blog/supply-chain-risk-in-vscode-extension-marketplaces)).
  No case of a profile export leaking secrets was found; that is a gap in
  the sources, not a proof.

### Lessons

- **Secrets in the keyring only.** Secrets never live in a settings or
  profile file. Plugins get a keyring API, excluded from export and sync
  by design.
- **Plugins as references.** A profile lists each plugin by id, version
  constraint and registry. Import resolves them and shows a review before
  anything is installed ("installs X 1.2 from Y, which asks for network
  and files"). Code is never copied. Versions are pinned on export by
  default, so an import gives what was exported.
- **Declared secrets.** A manifest marks a setting `secret` or `machine`
  and the exporter enforces it. A secret scanner runs on export as a
  safety net.
- **Readable exports**, so users can check one before sharing it.

## Open questions, for the design

1. The theme format: DTCG JSON, or Helios's own schema convertible to it?
   How are egui-only settings exposed: as tokens, or as `$extensions`?
2. Can a theme extend a theme from another plugin? What happens when a
   base is missing?
3. One profile file, or a directory of per-part files? Is the panel layout
   in a profile by default, or per workspace only?
4. Do presets and styles (photo editing data) belong in profiles, or in a
   library of looks shared by every profile, as darktable and Lightroom
   keep them?
5. Keymaps across systems: abstract modifiers (`Mod` for Ctrl or Cmd), or
   bindings per OS? How do MIDI bindings travel?
6. Sync: through the vault, through files and git, or both?
