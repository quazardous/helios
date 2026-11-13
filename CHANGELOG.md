# Changelog

All notable changes to this project will be documented in this file.

> This is a curated, human-readable record -- **not a commit log**. Each
> entry says *what changed and why it matters to a user*, in plain
> language, not *how* it was implemented. Skip internal refactors.
>
> **House style** for editors:
> - One short bullet per change. Multi-paragraph entries are only for
>   the major changes a user really needs to read in full.
> - No internal tracker IDs (`#NNN`, `PROJ-123`) unless that tracker
>   has a public link -- they're noise otherwise. Mention the change,
>   not the ticket.
> - **Version bump = SemVer**: any `### Added` entry is at least
>   MINOR; `### Fixed` alone is PATCH; breaking change is MAJOR
>   (MINOR before 1.0).
>
> **This file is generated at release** from the fragments in
> `changelog.d/`, written with each change (see CONTRIBUTING.md). Do not
> edit it by hand between releases; `cargo xtask changelog` previews the
> next section.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.88.0] - 2026-11-13

### Added

- Colour chart calibration measures raw shots: `calibrate.chart` (the `helios_calibrate_chart` tool, `helios control calibrate-chart` and the desktop command) now reads a raw chart shot on its own sensor data, before the camera's input profile, and reports each patch, the light and the correction found. Patches clipped on the sensor are left out, and a chart placed off the image is refused with its reason.
- Calibration profiles apply to photos: `calibrate.apply` puts a profile on photos' edits (a new `chartprofile` step right after input colour), or with `auto` the camera's profile nearest the shot's light, refusing with the reason when none fits; `none` turns it off. `calibrate.profiles.setDefault` chooses which profiles a camera's photos may get. On the desktop an apply is one step Ctrl+Z takes back.
- Colour chart calibration works end to end: a raw opened for the first time gets its camera's default profile (the status bar says which, or why none fits); the chart profile module's header in the Parameters panel names the profile, camera and light and switches profiles in one undoable step; "Develop one image" (version 3) carries the module; `calibrate.profiles.export` and `.import` share a profile as a `.helios-chartprofile.json` file, checked on import.
- Calibrate camera colour… opens the Calibration focus in a window of its own: open a ColorChecker shot, drag the four corner handles onto the corner patches, read the report (Delta E before and after, quality, light, clipped patches), name and save the profile, optionally as the camera's default. Its Profiles list sets defaults, exports, deletes and imports profiles. The chart profile header's "Make a profile…" opens it.
- Colour chart calibration saves profiles: `calibrate.chart` with `save` (`--save` on `helios control calibrate-chart`, the MCP tool, the desktop command) keeps the correction found as a calibration profile in the library, with its camera, light and quality, and answers its id. `calibrate.profiles.list`, `.get` and `.delete` read and remove them; on the desktop a save or a deletion is undone with Ctrl+Z. A raw from the library is measured with its own edit's settings.
- Chart profiles can now carry a residual table: after the matrix, a hue and saturation table (DNG's layout) corrects what the matrix leaves on the chart. It is kept only when it lowers the Delta E of patches left out of the fit; the Calibration report says whether it was kept, with Delta E without and with it. Profiles with a table export and import; older profiles still load and render as before.
- `look.camera.fit` fits a camera look to each raw's embedded JPEG and writes it into the edit, all photos as one undoable step, from the command line, agents (MCP) or the desktop's commands; a photo it cannot fit is answered with the reason in words and left as it was. The look's colours now follow colour calibration, and every template showing a raw gets the look's tone step, so no view of a looked edit renders flat.
- The darkroom's Camera look entry: "Fit to the camera's JPEG" fits the open raw's look and says how well (held-out Delta E against the default render's) or, refused, why in words; a switch turns the look on and off, and Refit fits it again. Each is one undoable step of the edit; with a chart profile on, the colour half stays off and says so.
- The camera look has its place in the raw chain (not yet turned on from the app): "Develop one image" carries its colour half after the chart profile and its tone half before sigmoid, both off, so renders are unchanged. On, they render a fitted look exactly as the fit computed it; a chart profile keeps the colour to itself, the look's tone staying on top.
- The camera look fit (not yet in the app) follows the camera's JPEG more closely: a hue/saturation table catches the hue twists a matrix cannot, kept only when it clearly helps; the colour matrix is refined on Delta E 2000; and a nearly black frame gets a tone curve alone instead of a refusal.
- Agents and the `helios` CLI reach the desktop's window commands through its library's command endpoint: `commands.list` lists them beside the node's (`scope`: node or window), each saying whether it applies in a given window and, when greyed, why ("Only in the Helios window"); `command.run` runs one in a window under the agent's name, refused with that reason where it does not apply.
- Every command reaches agents and scripts through one pair generated from the registry: `helios commands` lists the node's and the desktop window's commands with their params' schema and, when greyed, why; `helios run ID --param k=v` (or `--json`) runs one as the CLI. `helios-mcp` offers the same as `helios_commands_list` and `helios_command_run`, run as the agent, a greyed command refused with its reason.
- Assisted culling in the Library: photos measured soft, with blown highlights or with crushed shadows carry a badge in the grid; a Culling facet and the `cull:sharp`, `cull:soft`, `cull:clipped` and `cull:unmeasured` filters select them in the vault; the sort menu orders by sharpness, sharpest first; smart album rules can test `focus`, `clip_white` and `clip_black`. The Stacks panel says why it proposes a photo on top.
- Google Photos: pick more photos whenever you want. "Pick photos…" is in the account source's menu and header, and a finished picking offers "Pick more photos…": each opens a new picking, its photos are added to the same source, those already imported are said so and left as they are. Download addresses that expired are renewed on the way. The account's address is shown ("Google Photos — name@example.com"); an account connected before asks one reconnection to show it.
- Plugins can declare their own health checks in their manifest, through the same contract as Helios's modules: Help > Health check runs them through the plugin, with a time limit, and a check whose plugin crashes or does not answer in time shows as that check's problem. The Google Photos plugin uses it to say which account needs reconnecting, with a Reconnect button.
- Import history: every import leaves a record in the library (its sources, choices, counts, when and who asked), whether the desktop, the node, the CLI or an agent ran it. The import window lists the latest under its sources, each with Import again, which runs the same import with the same choices. Agents use `import.history.list` and `import.history.again`.
- Import glances for remote sources: a connector that gives previews (Google Photos does) has its items' glances kept in the glance cache, in memory and on disk, by connector, account, item and its version, so a pick opened again, even after a restart, asks nothing of the service. Reading ahead keeps to the limits the connector declares (requests a minute, a daily hint; a source plugin's manifest says `thumbnails` and `limits`) and to the cache's budget, never pushing out a glance shown.
- Library: drag photos from the grid or the filmstrip onto an album or the quick collection in the Albums section to add them, one undo step. The selection goes when the photo under the pointer is in it, else that photo alone. A smart album or an album set refuses the drop and says why. "Add the selected photos" in an album's menu stays the keyboard way.
- Library: album sets group albums, smart albums and other sets, as Lightroom's collection sets. The Albums section shows each set as a group that opens and closes, with how many photos its albums hold together; a click shows them all in the grid. "New set…", "Move to set…" and deleting a set (its albums kept, or deleted with it) are each one undo step.
- Library: the quick collection. B puts the selected photos in it, or takes them out when they are all in, one undoable step per press. It is pinned first in the Albums section with its count, a click shows only its photos, and each photo in it has a "quick" badge in the grid. Its menu saves it as an album (emptying it or not) or clears it.
- Library: the recent filters, as darktable's collection history. The filters used are kept per library, newest first, ten at most, and survive a restart; a filter can be pinned. Ctrl+Shift+K lists the pinned and recent filters and the smart albums; type to narrow, Enter applies one to the grid.
- Smart albums: "Edit the rules…" opens a rule editor for any tree of rules: add, remove and reorder conditions and nested groups, switch a group between all, any and none, pick each condition's field, operator and value, undo and redo each edit. A condition that is not valid says why under it, and the rules are applied only once all are valid. The capture date takes relative periods (today, this year, the last N days, weeks or months), kept relative so the album follows the calendar.
- Smart albums: an album whose rules choose its photos, kept up to date. "New smart album…" in the Library's Albums section asks a name and conditions (stars, colour label, flag, tag, camera, file type, folder, a range of days), all or any of them; agents may set nested rules over more fields (lens, ISO, aperture, title…) with `library.collection.rules`. A smart album shows with its own mark and its live count, scopes the grid like an album, and each change of its rules can be undone.
- macOS: the menu bar follows the window in front. With the import window or Settings in front, the main window's commands are greyed as in that window's own bar, and what is chosen runs in that window. Menu entries show their Cmd shortcuts where the menu can answer them without taking a key from a text field; Cmd+C, Cmd+V, Cmd+Z and the other editing keys stay with the field.
- Clients read the library at scale through the node: cursors page a query's photos window by window (`cursor.open`, `cursor.window`, `cursor.rank`, `cursor.refresh`, `cursor.close`), each photo read once even while another client writes, and a cursor left unread expires. `library.fields.set` writes photos' fields. A client cache keeps repeated reads off the node and drops exactly what the node's events say changed.
- Plugins now receive the settings set for them: their own (Settings > Plugins) when they start and with each call, and a source plugin also its account's and its source's. A change reaches a running plugin at once, or restarts it when the setting asks for that. Secrets go from the system keyring to the plugin alone. In the import window, a source's parameters and its account's settings use the same controls as Settings, and a refused value says why.
- Quick settings (Ctrl+Shift+Q, or File > Quick settings) edit the settings changed most often, such as the theme, the interface size, the language and the decision nodes, without opening Settings, in the window you are working in. The settings you changed lately follow them, typing searches every setting, and each one links to its page in Settings.
- Settings > Accounts manages the import accounts: each shows the sources using it, when it was last used and its status (OK, needs reconnecting, or access refused when the service refused its token). Rename an account in place, reconnect it, or disconnect it: Helios revokes its access where the service allows, forgets its tokens, and asks whether the sources using it are kept or forgotten. "Add account…" is offered per service.
- The Settings window edits the focus it was opened from: opened from the main window, its library and workspace; opened from the import window, the library it imports into. While both windows are open, a header names the focus edited ("Settings for") and a switch picks the other.
- Settings > Plugins: each installed plugin has a page with its permissions and the settings its manifest declares, drawn from its configuration contract (toggles, numbers within their range, choices, paths, and secrets typed masked and kept in the system keyring). Values are saved at once: the app's in `plugins.json`, a library's (where the plugin allows one) with the library. A value a plugin's contract refuses is left out and said, and a change made by another window shows live.
- Workshops (experimental): Make it automatic now writes the workshop's limits, prefilled from the test photos: each slider's range and how far it may move from the test value, each target's tolerance, how far a photo may be from the test photos, each AI junction's minimum confidence, the AI calls a run may make, and whether a breach is flagged, held or stops the run.
- Workshops (experimental): a run's review. `workshop.review` and the Workshops panel list the run's photos, those to review first, with their flags, where each value comes from and the values before and after. Per photo: accept a held one, revert a written one, set a value by hand, or decide a junction left to its fallback, recorded in the decision book so a re-run takes it. Re-run the held and decided photos. Your agent can decide junctions and fix values over MCP.
- Workshops (experimental): `workshop.run` runs the applied workshop on the Library's selection, the photos in view or given photos, as a job. Each photo is aimed, its AI junctions answered within the run's budget and recorded, so a second run asks nothing, and checked against the limits: written with each value's provenance, flagged, held or the run stopped. A dry run says the photos, J, A and what is already answered. The run is one undo step.
- Workshops (experimental): a second test photo joins the workshop session once you move the workshop's sliders on it, and Make it automatic reads every test photo. A row kept as AI can now be confirmed: the workshop keeps it as a junction, asked on each photo, with the test value as its fallback.

### Changed

- The `helios` CLI and the agents' MCP server use the running Helios: while the desktop has a library open, `helios import`, `import-darktable`, `stacks`, `batch --sidecar --vault` and an agent's macro step on an image reach the desktop's library instead of opening it a second time. With no Helios running, they start a background node that exits when idle.
- The desktop's library writes ratings, flags, colour labels, tags and stack expansion through its own node, off the window's thread: a busy library no longer stalls a frame while a mark is saved. `library.fields.set` takes an `op` (`add`, `remove`, `set_if_absent`), and `library.fields.get` reads photos' fields back.
- A memory card or camera is recognised by its volume (the filesystem's UUID or serial, else its label and size), not by where it is mounted: the same card in another reader keeps its saved choices, browser and automatic import, and another card mounted at the same place is another source. Sources saved before take their card's identity the first time it is seen. The card's label and identity show beside it.
- The import window has the main window's title bar and menus: the menu button, the logo and the name, then File, Edit, View, Window and Help. What only the main window does (Export…, Copy files, Copy the image) is greyed there and its tooltip says where it runs; the rest acts from the import window. File > Close window (Ctrl+W) closes the window it is chosen in.
- The Settings window saves through one store: Helios's settings go to their layer (your own and the library's through the node, this machine's in its file) away from the interface, and the interface's own settings to `interface.json` in the configuration folder. A failed save shows in the Settings window. A setting another client of the node changes shows at once, and nothing is written back.

### Fixed

- "Choose a folder…" in the import window opens its folder picker over the import window, not over the main window: the import window waits for the answer, the main window stays usable, and closing the import window closes its picker too. The same holds for the library folder asked when picking from an account and for "Change…" where copies go.
- Closing the import window while the main window is hidden (behind the browser opened to pick Google Photos, or minimised) no longer leaves a black, frozen window on screen: the import window hides at once and the main window comes back to the front.
- Many clients editing the same photos through one headless node no longer slow every other call down: opening a photo's edit (`library.edit.head`, `library.edit.save`) found its newest version by reading its history from the vault once per diverged version, holding the vault the whole time. It now reads the photo's versions once.

## [0.87.0] - 2026-11-13

### Added

- Every desktop command now has an icon, shown in menus and the command palette: the ones that had none (collections, macros, masks, the network, workshops, scopes, zoom, labels and flags) take a Phosphor icon. The status bar's items (images, zoom, history, picks and rejects) show an icon before their text.
- An import now measures, for each photo, how sharp it is and where its sharpest area lies, how much of it is blown to white or crushed to black in each channel, and its brightness (mean, median and spread). Photos imported before are measured by a background job in the jobs centre, which can be paused or cancelled and resumes where it stopped. These measures will help pick the best frame of a burst.
- Dialogs can be copied for a bug report. A failure's toast offers "Details": an error dialog whose Copy button copies the message with Helios's version and commit, the system and display server, the command that failed and its error, in a collapsed "Details" section too. Questions copy their text with the same details, hidden. No secret is ever copied (client ids, tokens) and the home folder is written `~`.
- Saved import sources can import on their own: "Import automatically" in a source's menu, or in the wizard's choices step, sets a card to import when it is plugged in and a folder to import new files as they appear (once a file holds still). The import runs without the import window, with the source's own choices; photos already in the Library are skipped. A change made to the sources by another Helios window is followed.
- The import window's Contents column shows the selected source in one of the ways its connector offers, with a switch between them: "All photos" (one flat grid), "Folder tree" (a folder and its subfolders at a time) and "By date" for folders, "All photos" and "By date" for cards, "Pick in the service" alone for Google Photos. The way chosen is remembered for each source.
- "Edit source…" in the menu of a source in the import window's Sources column opens the add-a-source steps on it: its folder or account and its connector's own parameters, its import choices (copy or add where they are only when the source allows both), and its name. A recent folder or a card is saved that way. "Forget this source" takes a source off the column. Both can be undone.
- "Add a source…" in the import window is now a wizard: choose the kind of source (a folder, cards and cameras, or a service such as Google Photos, each with a line saying what it does), set it up (pick the folder, or sign in to an account), set its import choices (those the source cannot honour are left out) and give it a name. The source is then listed under "Saved sources" with its own choices, selected and ready to import from. Cancel at any step adds nothing.
- macOS: the menus are in the system's menu bar, with the application menu (About Helios, Settings… Cmd+,, Services, Hide, Quit Cmd+Q) and the Window menu's Minimize and Zoom. Closing the last window keeps Helios running, as macOS applications do: the Dock icon or File > New Window brings the window back, Cmd+Q quits. Linux and Windows are unchanged.
- The node's inspect, control and batch endpoints speak JSON-RPC 2.0: the method is a command's id, errors carry standard and Helios codes, one connection carries several calls and the event notifications of `events.subscribe`, `node.hello` gives the protocol version and the client's permissions, and `$/cancel` stops a wait still running. The older `{"cmd": …}` requests are still answered. See docs/protocol.md.

### Changed

- Stack suggestions now propose the best frame of a burst on top using the culling measures: the sharpest frame that is not blown to white, with the reason given ("sharpest, not clipped"). Photos not measured yet keep the previous choice and are never preferred over a measured one.
- `helios import`, `import-darktable` and `stacks` no longer open the vault: they run on the node holding it, found beside the vault (`.helios/NAME.node.json`) or named by `--node` / `HELIOS_NODE`. With none running, the command starts `helios-node --idle-exit`, shared by commands run at once, which exits when idle.
- In Settings, a setting can be set for one library or workspace only where that makes sense: the library defaults (decision nodes, finished batches kept) may differ per library; every other setting is app-wide and no longer shows a "Set for" choice. Where the choice remains it is labelled, and its tooltip says what "This library" and "This workspace" mean and which wins. A value saved for a library or workspace on a setting that is now app-wide moves to the app's value, so none is lost.

### Fixed

- A raw shot with the camera set to Adobe RGB now shows its embedded preview in the right colours: the preview is read in the colour space the raw's Exif names, no longer always as sRGB, which made it look dull.
- Connecting or reconnecting an account (Google Photos) and "Pick photos…" open the browser at once: the page is opened as soon as it is ready, through the system's own opener (the desktop portal inside the Flatpak), instead of waiting for the import window's next frame. The dialog shows the page's address, to copy, with "Open again". Every "Pick photos…" starts a new picking session; the dialog says which account the browser must be signed in to, and a session that expires is reported ("The picking session expired, start again") with "Start again", instead of being waited out.
- The Flatpak release build now includes the built-in Google Photos sign-in client, like the other packages.
- Closing the Import window no longer waits on work it left: a connection in progress is given up, and a source's choices are saved to the library off the interface's thread, so a busy library cannot hold a frame. If the interface is ever held more than 2 seconds, Helios now logs what it was doing and leaves a `stall-*.txt` note in its crash folder.
- The Import window no longer freezes while the library is busy or a plugin is slow to stop: its accounts are read in the background, and allowing a plugin's permissions stops and restarts it off the interface's thread. Finishing or cancelling an import in a window that took the main window's place (the main window closed first) now brings the library back instead of quitting Helios.
- "Pick photos…" copies the picked photos into the account's own Into folder, else the import window's, else the library folder; when no library folder is known yet, it is asked for (and remembered) before the picking page opens, instead of the import failing afterwards. Signing in to Google Photos now asks for offline access, so the account stays connected beyond an hour instead of needing a reconnection.

## [0.86.0] - 2026-11-12

### Changed

- Import previews keep what you looked at. A preview shown in the import grid is kept longest; previews read ahead for the rest of a source go first when the cache is full, and reading ahead stops instead of pushing out what was shown, going on when room comes back. A folder's previews are now kept on disk like a card's, so a folder opened again, even after a restart, shows them without reading its files. Settings > Storage > Caches says how much of the previews is shown and how much read ahead.

## [0.85.0] - 2026-11-12

### Added

- The node lists its commands and runs them by id: `commands.list` gives every control verb with its label, the JSON schema of its params, what it needs and whether it can run here, with the reason when it cannot (a debug-only verb, a vault verb on a node started without `--vault`); `command.run` checks the params against that schema, then runs the verb as its request would, saved under the origin that ran it (`cli`, `user`, `agent:NAME`) with its undo group. Both are served by the batch endpoint in every build; `helios control commands` and `helios control run ID JSON` use them.

### Changed

- Google Photos no longer asks for an environment variable: release packages carry Helios's own OAuth client, and Settings > Accounts > "Use my own OAuth client" takes yours (client ID and secret, kept in the system keyring) and says which client is used. With no client at all, connecting says to set one there. The `HELIOS_GOOGLE_PHOTOS_CLIENT_ID` and `HELIOS_GOOGLE_PHOTOS_CLIENT_SECRET` variables still work, after your own client and before the built-in one.

## [0.84.0] - 2026-11-12

### Added

- Each import source keeps its own choices. The import window's Choices column follows the source selected (the folder ticked last, or an account clicked): changing a choice saves it on that source, with Undo, and "Reset to defaults" gives it the default choices again. The sources are kept by the node with the person's settings, so they come back after a restart. Choices a source cannot honour are hidden: a remote source such as Google Photos always copies and has no darktable edits to read. Cards keep the default choices.
- Plugins ask before they act: an action of a plugin that lacks a permission it declares (connecting a Google Photos account) opens a dialog listing each permission with the plugin's reason, Allow or Don't allow, instead of failing with "grant it with `plugin.grant`". The answer is remembered in your settings and follows you; Settings > Plugins lists every plugin's permissions with a switch each, and a permission a plugin's update adds is asked for again.

## [0.83.0] - 2026-11-12

### Added

- Albums in the Library: an Albums section in the Library panel lists them with how many photos each holds; "New album…" makes one from the selected photos, and each album's menu adds or removes the selected photos, renames or deletes it. A click shows only the album's photos, kept with the workspace. Every change is one undo step, and agents have the same commands (`library.collection.*`).

## [0.82.0] - 2026-11-12

### Added

- The person's and the library's settings follow live: when another client of the node changes them, the desktop reads them again and the Settings app shows the new values without a restart. `helios-node --vault` reads and writes these settings through its control endpoints (`helios control settings-get LAYER`, `helios control settings-set LAYER TOML-FILE`, the `settings.layer.get` and `settings.layer.set` requests, also on the batch endpoint), each change announced on its bus as `settings.layerChanged`.

## [0.81.0] - 2026-11-12

### Added

- Colour chart calibration, first call: `calibrate.chart` measures a ColorChecker shot (a decoded picture) from the centres of its four corner patches and answers the report, writing nothing: Delta E per patch before and after, the light found, the exposure, the matrix. As a node request (`helios control calibrate-chart`), the `helios_calibrate_chart` MCP tool and a desktop command. Raw shots come next.
- `helios-node --vault` imports through its control endpoints (`helios control import FOLDERS-OR-IMAGES...`, the `library.import` request, also on the batch endpoint): each import is announced on the node's bus as `library.imported`, and the node then runs the backup strategy's after-import backup, as the desktop does.
- `helios-node --vault` runs what the desktop runs without a window: it watches the machine's disks and cards, backs up to a backup disk of the strategy when one is plugged in, and runs the health check on its schedule. It opens the vault once, and SIGTERM or Ctrl-C now stops it cleanly: running backups are cancelled after their current copy, and the vault is closed and marked clean.
- macOS on Apple silicon: each Release now carries `helios-<version>-macos-arm64.dmg`, a disk image holding Helios.app, with the command line inside it. It is not notarised yet: see the README for how to open it the first time.
- Settings open in a window of their own, as the import window does: Helios's title bar, their own size and place kept, closing alone, and Window > Settings opens them. Closing the main window while they are open moves them into it. "Pop in" still shows them over the main window, and that choice is kept.
- Experimental workshops: "Make it automatic" in the Workshops panel turns a piloted session into rows, one per slider you set (a target a measure explains, the same value, or kept as AI) and per AI zone (a fitted parametric mask), with how much AI per photo is left. Confirm writes the workshop's next version as one undo step. Agents read and amend the rows over MCP (`helios_workshop_generalise`); only you confirm.

### Changed

- The desktop notification of a long job's end now covers every job the desktop sees, whoever started it: imports, backups (an after-import backup, a disk plugged in), health checks and the jobs of a followed node, beside exports and batches. The image map's upkeep and the exports inside a batch stay quiet.

### Fixed

- Closing the main window while the import window is open moves the import window into it at the import window's size and place, with its title bar: its title and the minimize, maximize and close buttons. The import window draws Helios's title bar rather than the system's. The Window menu no longer offers Switch to window…; it keeps Helios's windows and the list of those open.

## [0.80.0] - 2026-11-12

### Added

- Pick photos from a source plugin in the import window: a connected Google Photos account shows "Pick photos…", which opens Google's picker in your browser, waits until you press Done (Cancel stops it), then imports what you picked as copies. A sign-in Google refuses marks the account "needs attention" with Reconnect.
- A Google Photos source plugin, bundled with Helios: sign in with your own Google OAuth client, pick photos in Google's Photos Picker, and Helios lists, downloads and imports them as copies (Google keeps Exif but drops the location). A refused or revoked access asks to reconnect the account. Setup is in docs/google-photos.md.
- A right click on a folder or a volume in the Library's Folders section offers "Show in file manager", "Scan this folder" (the folder is read again for new or changed files) and "Import new photos here" (the import window opens on it); greyed for a disk not plugged in. The folder chosen in the tree is kept with the workspace: it is there again when the workspace opens.
- The Library's Folders section starts at the volumes the photos are on (each disk or card by its name, with how many photos it holds), their folders below. A volume not plugged in is greyed and says when it was last seen; its folders still show their photos, from the thumbnails Helios keeps.
- Sources that sign in with an online account (OAuth) open the system browser on the service's consent page and get the answer back on a local port, with PKCE. The tokens are kept in the system keyring, renewed when they expire, and revoked on signing out; an access the service no longer honours shows the account as needing attention. The client id comes from the environment (`HELIOS_<SOURCE>_CLIENT_ID`), never built in.

### Changed

- Settings has two panes: a tree of sections on the left, the page on the right, with a divider you can drag and whose width is kept. The import window's carousel follows the thumbnail size control, its middle photo no longer drawn over its neighbours. A folded side bar unfolds on a click anywhere on its strip, and cards, tips and popups stay in a steady order over the panels.

### Fixed

- A backup keeps running when its window closes: it finishes, and any other window's Queue panel shows it. Quitting while a backup runs asks first; confirming cancels it, and the copy under way is recorded before Helios closes the library. The health check keeps its schedule with or without a window.
- An export keeps running when its window closes: it finishes, and any other window's Queue panel shows it. Quitting while an export runs asks first; confirming cancels it before Helios closes the library.
- A card or disk plugged in is noticed with the import window closed: the library's node watches the volumes, so the card's toast comes, the import window opened afterwards lists the card, and a backup disk plugged in is backed up, whatever window is open.
- Ctrl+Q quits Helios from any window, asking first only when jobs are running. Ctrl+W closes the window in front, as its close button does. Closing the main window while the Import window is open no longer quits: the Import window takes its place, and closing the last window quits. The Import window has a short menu bar, with File and a Window menu listing Helios's windows.
- Window > Main window brings the main window back after it was closed while the Import window was open: the Import window goes back to a window of its own, and the main window shows the library again.

## [0.79.0] - 2026-11-12

### Added

- The import window's Sources column gets "Add a source…" for sources that need connecting first, such as an online account: Helios walks through the steps the source asks for, then lists the account with its state. Settings > Accounts lists every account with Reconnect and Sign out; passwords and tokens never go into the library.
- The Library has a Folders section: the folders holding your photos, each with how many photos are there or below, counted when the section opens. Clicking a folder shows only its photos (the rest of the search is kept); clicking it again shows the whole Library.
- Import from a Lightroom Classic catalogue: pick a .lrcat file and Helios brings in its photos with their stars, rejects, colour labels, keywords (with their hierarchy) and collections (as tags under "Lightroom collection", until albums arrive). The catalogue is only read, from a copy while Lightroom has it open. Virtual copies are folded into their master photo; develop settings are not imported yet.
- Lightroom catalogue import reads each photo's develop settings (from the catalogue, else its .xmp sidecar). Exposure, white balance as shot and orientation are mapped onto Helios's exposure, white balance and flip modules, not applied yet; every other basic-panel setting (contrast, highlights, shadows, whites, blacks, texture, clarity, dehaze, vibrance, saturation, custom white balance, crop) is listed per photo with its value in the import log.

## [0.78.0] - 2026-11-12

### Added

- A memory card plugged in again shows its import previews at once, even after Helios restarted or with the card mounted elsewhere: each card's previews are kept on disk, by the card, the file's path on it, its size and modification time. Nothing is read from the card but the files that changed. Previews not seen for 30 days are pruned, and the Caches page counts and clears them with the other caches.
- The Library's whole search runs in the catalogue: words, dates, rating, labels, flags, tags and backup state, and new keys for the camera, lens, file type, folder and presence (`camera:"Z 8"`, `type:nef`, `folder:/photos/2024`, `presence:offline`). While a filter is in force, the Library counts its photos per rating, label, flag, camera, lens, file type, presence and backup state; a click on a count adds it to the filter.
- The Library can be sorted by capture time, import time, file name, rating, last edit or file size, either way, from a menu in its toolbar or by an agent. Photos show by capture time, oldest first, unless sorted otherwise; each window keeps its own sort with its workspace, and the photo count no longer wraps onto a line of its own in a narrow Library panel.
- Workshops (experimental) can be designed from your words: describe what you want in the Workshops panel ("backlit, the sky too contrasted") and your own AI agent, connected over MCP, proposes a workshop. Helios checks it on the open photo, drops sliders that change nothing and narrows ranges that clip, then applies it as one undo step; the photo's pixels are shared only if you allow it for the session.
- While you refine a workshop (experimental), your AI agent can propose values: they show in the Workshops panel as ghost values you accept (one undo step), override by moving the slider, or reject. Each proposal and each of your answers is kept in the session's record, with who made it.
- Workshops (experimental) now record how you pilot them: each step on a designed or applied workshop is kept with who made it (you or your agent), why, and what the photo's zones measured before and after. The Workshops panel shows the session with a Keep button; a session you do not keep is dropped when it ends.

### Changed

- The Import window opens as a window of its own, beside the main window rather than inside it: the main window stays usable while photos are chosen, and its status bar says which folder or file is being read. Closing it closes the import alone.
- Import glances, library thumbnails and offline previews of a raw file now read only its embedded previews and metadata, not its sensor data: about a tenth of the file for NEF, ARW, RAF, CR2, DNG and ORF, a third for CR3. On a card reader, a network share or a slow disk, previews come up faster. The pictures are the same as before.

### Fixed

- The graph's navigation hint, hand mode indicator and overview now lie under any window, palette, menu or dialog over the graph, instead of being hidden while one covers it (egui-pop 0.9).
- Where Helios has a single window (in the browser), the Import window opens centred in it again, with a close button, rather than in the top left corner over the menus.
- The Import window fits its content to the window again: only the photo grid scrolls, its last row is no longer hidden under the buttons, and the backup line below the choices shows again. The choices scroll on their own only when taller than the window.
- A workspace holding two macros whose files sort in another order than they were added (a workshop designed twice) no longer reads its own files as changed outside Helios: the layout is no longer reloaded under the panel in use, and no false "changed outside Helios" note shows.
- A workspace whose files change outside Helios reloads only the part that changed: a new network keeps the panels open on screen, a new layout applies without touching the network, and files rewritten with the same content reload nothing. The layout copy saved with the workspace now follows every layout change, so a reload never brings back an older layout.

## [0.77.0] - 2026-11-11

### Added

- Compare is now a workspace of its own: File > New workspace offers "Compare", and C (or the Compare command) on two to four selected photos opens them side by side, each drawn from its edit. Zoom and pan stay synchronised unless you turn that off, the arrow keys move the active photo, and its rating, flag and colour label are at hand.
- Builds made between releases carry their own version, a pre-release of the next patch counting the commits since the last release (`0.76.1-dev.18+be34c639`), shown in About, in crash reports and by `--version`; a release build shows its plain version.
- Helios for Linux as a Flatpak: each release now carries `io.github.quazardous.Helios-X.Y.Z-x86_64.flatpak`, a sandboxed bundle that sees the Pictures folder and removable media (see docs/releases.md, "The Flatpak", for its permissions and how to grant another folder). The tarball now also carries Helios's AppStream metadata.
- Semantic sliders for macros and workshops (experimental): one slider of a macro's interface can move several inner parameters at once, each along a curve of up to 8 points ("Backlight lift" raising the exposure while easing the contrast). The interface editor creates them with "New semantic slider" and draws each curve; agents give the same `drives` list to `macro.interface`. Moving one is a single undo step.
- Workshops (experimental) can aim a target inside a zone: a workshop names zones such as a sky or a foreground, each a mask drawn from the photo, and a target such as "sky midtones at -1 stop" is measured on that zone's pixels only. Two new measures help with backlit photos: a zone's contrast, and the gap in stops between two zones.

### Changed

- The import window's reading line names the folder or the file being read, shortened in its middle when the line is too narrow.
- The search field's dates now mean the day a photo was taken, by the camera's clock, instead of the day its file was last modified, and they are filtered in the catalogue in one query. Capture time (with its time zone offset when the camera wrote one), camera, lens, focal length, aperture, shutter, ISO, size, orientation and file type are kept for every photo at import; a background job fills them for photos imported earlier, and resumes where it stopped.

### Fixed

- Helios ended by the system (a plain `kill`, a logout or a shutdown, Ctrl+C in its terminal, its terminal or console closed, the Windows session ending) now closes as Ctrl+Q closes it, and the next start no longer says it closed unexpectedly.
- The import window's thumbnails fill fast: the photos in view are read first, on a few threads at once, each preview scaled down before its colours are converted, instead of one file after another in the folder's order. A photo already in the Library shows its Library thumbnail, and the previews read stay in memory (within a budget), so opening the window again on the same folder shows them at once.
- The import window draws a photo with its thumbnail at once when the thumbnail is already known (read earlier in the session, or in the Library's thumbnail cache): opened again, or scrolled, its cells no longer show empty first. Only photos never read fill in afterwards.
- A Library of more than about 32,000 photos opens again: it could not be read at all past that size. The Library is now read 2,000 photos at a time, each batch in one query, and never touches the photo files while reading: offline and missing badges come from the last scan, sizes and dates from what the import saw. After an import, a scan or an undo, only the photos added and those whose copies moved are read again.
- Cached thumbnails show at once: the cells in view are read from the thumbnail cache on the frame that draws them (on a few threads, within a frame's time), instead of waiting in the reading queue behind the rest of the library, so a library opened again no longer fills its grid one picture after another.

## [0.76.0] - 2026-11-11

### Added

- Settings › Storage › Caches shows what each cache holds (thumbnails and large thumbnails, against their budget) and clears them: one kind with its "Clear", or all with "Clear caches"; photos, edits and the catalogue are never touched. The thumbnail folder is tagged as a cache (`CACHEDIR.TAG`), so backup tools skip it, and existing thumbnails stay valid. `helios-node` answers `cache.stats` and `cache.clear` too.
- Windows has an installer beside the portable zip: `helios-<version>-windows-x86_64-setup.exe` installs Helios for the current user, without administrator rights, adds it to the Start menu and uninstalls from Settings. It is not signed yet, so SmartScreen warns on first start.

### Changed

- On Windows, Helios's window opens without a console window beside it, shows its icon in Explorer and the taskbar, and draws sharp on high-DPI screens; started from a terminal, `helios-gui --version` still prints there.

### Fixed

- The graph's navigation hint ("Middle-drag, Space+drag or H to pan…") and its overview no longer show on top of the import window or a dialog lying over the graph; the hint comes back once the window is closed.
- Exports render on the desktop's own engine: they share the render cache and its budget, so exporting the image open in the darkroom does not compute again what it already holds, and a large export no longer grows memory without bound (the exporter's own cache had no limit).
- The desktop applies Settings › Storage › Caches › Render cache (`system.cache.budget`, or `system.cache.share` of memory): only `helios-node` did, the desktop kept a quarter of memory whatever was set. The render cache is listed on the Caches page and cleared with the others, and evicting many entries at once no longer scans the whole cache for each one.

## [0.75.0] - 2026-11-11

### Added

- Scenarios run live: `HELIOS_AGENT_WRITE=1` lets an agent on the socket change the image and the library, quick open goes to `file:<path>`, and the box's `scenarios` job replays `crates/helios-gui/tests/scenarios` (import a folder, exposure in the darkroom, undo and redo) on a throwaway library in a headless compositor, with invariants after every step, shots and the journal (docs/gui.md, "Agents").
- The agent socket can be turned on from the settings (Developer, "Agent socket"): it starts and stops at once, read-only; `EGUI_POP_AGENT_SOCKET=1` still turns it on (docs/gui.md, "Agents").
- `EGUI_POP_AGENT_SOCKET=1 helios-gui` serves egui-pop's debug bridge, read-only, on a private socket in the runtime directory, for agents; `egui-pop-agent-mcp` connects an MCP client to it (docs/gui.md, "Agents").
- Agents and scenarios read Helios's state from three debug collectors: `helios.library` (images shown, picked and rejected, the selection, the image under the cursor, the size of the image open), `helios.values` (the image's parameters by path, as `param.get` reads them) and `helios.jobs` (jobs counted by state, each job's kind and state), so a scenario checks `Expect(Value(..))` instead of a command's result.
- Backups in view: the import window says what backups the photos will get ("2 copies: Backup disk (plugged in), NAS (not seen for 3 days)", or "No backup set up" with a Storage button). Library thumbnails show each photo's backups (backed up, short, unverified, no backup) and the search takes backup:met, backup:short and backup:none. A card left in the reader is said safe to format once a backup completes its photos.
- Backups: the Storage page sets the library's backup strategy (how many copies, on which disks, one off site, after each import or when a disk is plugged in) and has "Back up now". Each copy goes to Helios Backup/ on the disk, from the Library copy, written aside, read back and checked before it counts, never overwriting; the job pauses while the disk is unplugged. A card is safe to format only once its photos have their backups.
- Catalogue snapshots: Helios takes a verified copy of the catalogue (ratings, tags, edits) daily and on demand ("Snapshot now" on the Storage page), keeps five next to it and copies each to the backup disks of the strategy plugged in, under Helios Backup/catalogue/. The Storage page lists them, here and on the disks, and restores one after a confirmation: the current catalogue is kept aside and Helios starts again on the snapshot. The health check reports the last one, whether it can be restored, and warns past 7 days.
- The controls gallery's bench saves and loads a bench as text, in the browser as natively; its presets are buttons of their own.
- The controls bench gains a context bar (task, detail, image, tool, device, density, and the space its width gives), a switch between panel, dock, toolbar, float and HUD, a pane saying where each control is placed and why, saving and loading a bench (`controls.bench.save`, `load`), and headless scenarios measuring steps, reach, shift and reachability.
- The controls gallery has a bench page: drag controls from a searchable shelf (business controls by purpose, implicit controls by value type) onto a card laid out by the slot engine, reorder, remove and group them into cases with a legend, help and a separator, resize the bench, and try every value live with undo. Agents build it with `controls.bench.add`, `remove`, `move`, `case`, `list` and `clear`.
- Controls draw by their slot: the controls gallery shows exposure as a full, compact or vertical slider, a knob and a read-out, each in a box of its own width, and a card of implicit controls where a curve and a colour wheel draw their editors when wide and fall back to rows of their values when narrow. Parts a card folds wait under "More (n)", which says why on hover and shows one again on a click.
- The default library: without `--vault`, Helios opens `library.db` in its data folder (`~/.local/share/helios` on Linux, `%LOCALAPPDATA%\helios` on Windows, `~/Library/Application Support/helios` on macOS), made on the first launch, so File > Import… works from the desktop entry. A library that cannot be opened is said in a toast, and the import window then shows why with its Import button off.
- A web demo of the controls framework: a mixer channel and a light declared as data, composed on the bench (shelf, cases, width handle, context bar, modes, presets, undo and redo, and why each control takes its form), at https://quazardous.github.io/helios/egui_hud/.
- The health check watches the backups: a warning when no backup strategy is set, when photos are short of their copies, when a backup disk has not been plugged in for more than 14 days (a setting), when copies are not verified yet, and for each inserted card not yet safe to format, each with its fix (Back up now, or the Storage page). It also checks that the library opens and that its disk keeps 5 % or 2 GB free.
- The health check also says which GPU Helios renders on, with its driver (a software adapter while a GPU is present is a warning), compiles every GPU kernel once per adapter and lists those falling back to the CPU, starts each plugin to see it answer its handshake (a crashing or unsupported one is shown with its log and a fix: disable it or open its folder), and says there are no catalogue snapshots yet.
- The image map's blacklist: Helios never records system folders, its own data, caches (CACHEDIR.TAG), build trees, hidden folders, trash and other photo apps' libraries, with defaults per OS. The settings system.map.exclude (globs, global or per volume, ! to re-include), system.map.defaults, system.map.record, system.map.limit and system.map.keepMissing are declared.
- Forget what the image map saw: "Forget this folder" in a source's menu of the import window and "Forget what was seen here" for each volume on the Storage page drop the files not in the Library, and the folder is not recorded again until listed on purpose (commands library.forgetFolder and library.forgetVolume).
- The image map's background job (kind map in the jobs centre) reads the files it knows: those in view in the import window first, then Library files never verified, volumes just plugged in, duplicates to confirm by their full hash, and the rest. It never reads blacklisted places, waits while an import, export, backup or render runs and on battery, can be paused and cancelled, and prunes the map at most once an hour.
- The image map records what the import window lists: every file of a folder, card, drop or paste, with its volume and path there, size, time, the partial hash when already read, and its capture time and camera once glanced. Blacklisted and forgotten folders are left out; a file listed again unchanged keeps what was read of it. The vault can forget a volume or folder and prune the map (system.map.limit, system.map.keepMissing).
- The image map scans a volume when it is plugged in: the folders it knows there first, then the rest, a stat per file and no read, in the background, pausable, giving way to imports and other jobs. Which volumes it scans follows system.map.record (every one, those with a role, or none when only imports record); once a scan ends, the cap on files outside the Library drops those seen longest ago. library.map.scanNow scans a volume, or every one allowed, on demand.
- The import window has presets (named choices saved and deleted from its footer), totals of the photos ticked, and a Contents column that takes the spare width. Files or folders dropped on Helios, or pasted, now open the import window on them instead of the old form.
- Importer plugins speak a version 2 of their protocol: a scan streams its photos in pages, with progress and a cancel between pages, and an image may carry its edits as a darktable history. The darktable library importer uses it: each photo's edit is rebuilt from library.db, so edits with no .xmp (write_sidecar_files off) are no longer lost; when both exist, the newer one is taken, and the import's log says which and why.
- Settings app: Helios's own settings (caches, image map, workers, decisions, history, workbench, experimental) get a page for the first time, in the Library, Storage, Hardware and Interface families. Each is edited where its kind belongs: machine settings on this machine, decisions and history for the library too, the person's in their profile; `--set` shows as the command line's value. Changes take effect at the next start.
- Backups: "Set up backups…" on Settings › Backup › Strategy opens a wizard: how many copies, which disks (required before Next), one off site (asked only for two copies or more), when copies are made, then a summary; finishing sets the whole strategy at once. Cancel asks first once something was entered. Agents run it with `admin.wizard.*`, or set the strategy directly with `library.setUpBackups`.
- Settings › General › Advanced exports your settings to a file and imports them back. Export takes every setting you changed, or one family or section, a library's or workspace's values, and your shortcuts if you like; secrets are never written. Import shows each change (now and in the file) to tick, and what is left out and why (unknown, retired, wrong type or out of range); the changes apply as one undo step. Agents use `settings.export` and `settings.import`, with `dryRun`.
- The Settings app's search now finds keyboard shortcuts, by the action's name or by its keys, and the settings on pages that draw their own controls; choosing a result opens its page with the item highlighted. A setting such a page edits no longer shows a second time on another page.
- Settings app: the Storage scope lists the volumes in a table (state, kind, roles, space, photos, backup) with "Backup disk on/off", "Scan now" and "Forget" on each, and holds the catalogue snapshot settings; the Backup scope shows the strategy and its disks plugged in, with "Back up now". The scope and section titles are translated. The Storage window stays until the pages cover it.
- Read-only commands for the storage: `library.volumes` lists every volume known (state, roles, space, photos, whether the backup strategy copies to it) and `library.backupStrategy` the library's strategy with its disks plugged in; `library.volumeRole` gives a volume a role or takes it back. Commands now declare whether they only read, which the Settings app and read-only agents rely on.
- Thumbnails follow their files: in the background, the photos on screen first, then those shown recently, then the Library, Helios checks each cached thumbnail against its file (size and time, then a partial and a full read only when they changed) and makes a new one when the original was edited elsewhere. It waits on battery and while imports, exports, backups or renders run.
- `helios --version` and `helios-gui --version` print the version of Helios.
- A Windows build: each release now carries `helios-<version>-windows-x86_64.zip`, a folder that runs where it is unpacked, with `helios-gui.exe` (the window) and `helios.exe` (the command line, `helios batch` included). It is tested on Windows without a GPU, so processing runs on the CPU. There is no installer and no signature yet.

### Changed

- A Library photo whose file another application rewrote stays in the Library with its marks and edits: Helios records the file's new content (keeping the old one in its history), makes its thumbnail again and marks it "changed outside" in the grid. The Health page counts these photos and offers to acknowledge them, which clears the mark.
- The Compare panel renders an edited raw again once a slider drag ends, not at each in-progress save while the drag goes on.
- Controls bench: the generic controls, one per value type (a number, a toggle, a choice, a curve, a colour, a text), are listed under "Miscellaneous", last on the shelf, with any control given no group. A checkbox stays a checkbox whatever its domain: the egui_hud demo's mute, power, colour tag and effect are now uses of the generic toggle and choice, not controls of their own.
- Controls bench: a case is a group inside the bench's one card, framed across the card's width: its parts still pack densely under its legend, but a later control never moves up above the legend or into the case.
- The Parameters panel draws each module as a card of the controls layer: every parameter has its control (exposure's `exposure` and now `black`, through the new black level control with a detent at 0) or its implicit one, laid out by the slot engine in the panel's width and drawn by the managed renderers, with the same gestures, undo steps, marks, menus and accessible names as before. `controls.context` reports the panel's own box.
- egui_hud separates controls (a value and a behaviour) from faces, a library per value type that draws them: checkbox, switch, push button, lamp; slider, fader, knob, endless knob, scrub field, meter, read-out; menu, segmented buttons, radio list; the curve, colour and text editors. Kits name the faces a control accepts (`"faces": ["fader", "knob"]`), numbers may wrap (the demo's hue, on the endless knob), and the bench picks a control's face.
- The file choosers use egui-pop's file browser, with its view modes and their switcher; a dragged tab is refused by the window's bars.
- Exposure is the first business control: its slider holds briefly at 0 EV while dragged (Alt drags through), steps by 0.01 EV with the arrows and the wheel (Ctrl 0.001), and resets with a double click. Agents get `controls.catalogue`, `controls.describe`, `controls.set`, `controls.nudge` and `controls.reset`, and a new controls gallery shows each control's forms (docs/design/controls.md).
- The import window's Contents is egui-pop's file browser: details, thumbnails, filmstrip, columns, carousel or agenda by day (Ctrl+1 to Ctrl+6), the mode kept for the window. The selection is what is imported; badges say RAW+JPG, in your Library and offline. A folder whose disk is unplugged lists what the image map recorded, with the library's cached thumbnails. Folders or cards dropped on the window open as its sources.
- Undoing an import no longer forgets what Helios saw: the photos leave the Library, but Helios keeps knowing the files, so importing them again is instant and reads nothing. Files already seen are imported without being read again, and photos only seen (or whose import was undone) are no longer taken for duplicates.
- An image opened from the library shows its cached 1024 px thumbnail at once, scaled to fit, instead of a blank view; the full render takes its place when it is complete (docs/performance.md, "The loupe's first frame").
- Ctrl+, and File > Settings open the Settings app. Preferences > Keyboard shortcuts opens its Input › Keyboard shortcuts page, and the pointer gestures, the fonts and the theme editor are its pages too. The settings window of before stays under General › Advanced.
- The health check now opens in the Settings app at Health, and its old window is gone. The findings are listed worst first with their tone, each with "Fix…" (a fix that is not easily taken back asks first) and "Ignore", the ignored ones last with "Show again"; a status line and "Check now" sit above. "No backup is set up" opens Backup › Strategy. The catalogue's snapshot controls show only with a library open.
- Storage now opens in the Settings app at Storage › Volumes, and its old window is gone. Each volume's name is edited in place, with a toggle per role, the off-site and RAID flags, a space bar and where it was last seen. "Mark this disk…" and "Forget…" ask first. Backup › Strategy edits the strategy in place, and Storage › Catalogue lists the snapshots with "Snapshot now" and "Restore…", which asks first.
- Library thumbnails come from a cache beside the catalogue (.helios/thumbs, 2 GB by default, setting "Thumbnail cache"): they show at once on the next start, and offline photos keep theirs, with an "offline" badge. A miss reads the file's smallest embedded preview, scaled while decoded, on a few threads, the photos in view first. A file with no readable preview shows its type and a "no preview" mark instead of an empty cell.
- Large libraries scroll within a graphics memory budget (Thumbnail textures setting, 512 MiB): thumbnails out of view are freed, oldest first, and read back from the thumbnail cache when they return. The cache is no longer listed while the window opens. Compare shows the cached 1024 px picture at once while the full render comes.
- The image is a canvas of egui-pop's navigation commands: with the pointer over it, Ctrl+Plus, Ctrl+Minus, Ctrl+0, Shift+1 and Ctrl+arrows zoom, fit and pan it, as they do the node graph. With no canvas under the pointer, Ctrl+Plus and Ctrl+Minus still zoom the image in the darkroom.

### Fixed

- The thumbnail cache and thumbnail texture sizes set in the library's user settings now apply: they were refused there (they belong to the machine), so the defaults were used. They are read as this machine's and move to its settings file at the next save.
- A darktable edit opened in Portrait retouch or the blank workspace puts darktable's exposure (its first instance, whatever its name) on the whole-image base exposure, no longer on the masked exposure above it. In "Develop one image" a named first instance now lands on the exposure too.
- The darktable library importer now opens libraries on Windows: it built a `file:/D:/…` address that SQLite refuses, now `file:///D:/…`. A library from a darktable older than 4.0 is refused with a message naming its schema and asking to open it once in darktable 4.0 or later, which upgrades it.
- darktable drawn masks land right on turned photos: importing darktable sidecars or a darktable library places each masked module's shapes on the raw and turns them as darktable turns the image, by the EXIF orientation or by the edit's own flip (a flip set by hand wins, a disabled flip leaves the photo unturned). Masked modules were left out before for lack of the image.
- Scrubbing a parameter that has no dedicated control now crosses its range in a couple of hundred points of drag (feather, 0 to 200 px: one pixel a point) instead of a fixed hundredth a point, which took 2000 points to reach 40 px. Whole numbers move by one at least every four points; Ctrl still scrubs ten times finer.
- The import window keeps its size: it no longer grows on its own, frame after frame, up to the size of the main window. It opens at four fifths of the main window (1200×800 at most) and changes size only when you resize it; its three columns and footer fit in the room it has.
- Import without a library open (Helios started without `--vault`) no longer fails with "no command `library.import`": the import window says "No library is open" and its Import button is off, and the import commands refuse with the same reason, before anything is queued.
- helios-node gives a picture's EXIF orientation to the modules reading it, as the desktop does: a network or a workflow run with a `flip` step turns a JPEG, PNG or TIFF shot in portrait upright.
- JPEG, PNG and TIFF pictures shot in portrait show upright: their EXIF orientation turns them through the flip step, as raws are turned, in the viewer, exports, thumbnails and the import window, and a flip set by hand still wins. darktable drawn masks on pictures are now applied, turned with the picture, instead of being left out.
- A raw shot in portrait now gets an upright thumbnail when it is read from its embedded preview, as darktable turns that preview by the raw's EXIF orientation, and its preview shown before the development (or through a network that takes pictures) is upright too.
- A picture shot in portrait whose thumbnail was cached before pictures were turned no longer stays sideways: the thumbnail cache marks the entries it stores with its reader's version, and an older entry of a picture its EXIF orientation turns is read again, while those of untagged pictures and raws are kept, with no rebuild.
- Windows: catalogue snapshots are taken again (they failed with "Access is denied"), so schema upgrades, scheduled snapshots, their copies on backup disks and restores work there.
- Settings: a toast in the corner no longer covers a wizard's Finish button (as "Use this strategy" in the backup wizard) and takes its click; toasts keep off the wizard's footer.

### Security

- The SQLite built into Helios is updated from 3.53.2 to 3.54.1 (RUSTSEC-2026-0459, CVE-2026-119375): older versions could return stale memory when opening a database whose -wal file has a different page size. Helios only opens its own library and darktable library files, so the risk was low.

## [0.74.0] - 2026-11-07

### Added

- File > Storage… lists every disk Helios has seen, online first, to name it, give it its roles, mark it off site or RAID and, for a weakly known disk, write its identity file. Photos on an unplugged disk show an "offline" badge and say which disk to plug in, and missing photos a "missing" one.

## [0.73.0] - 2026-11-07

### Added

- Help > Health check lists what Helios's background checks found (free space where Helios writes, the catalogue's integrity), each with a fix and "Ignore", and a status bar badge shows when something needs attention.

## [0.72.0] - 2026-11-07

### Added

- The import window's Choices column: copy into the Library (the default for a card) or add where they are, a destination with its folder layout (by day, month, year or one folder) and a naming pattern, each with a live example, plus duplicates and darktable edits. The choices are remembered per library, and `helios import --layout` picks the layout from the command line.

### Changed

- A photo on a disk that is unplugged is now offline, not missing: Helios
  knows which volume each copy is on and its path there, keeps the photo,
  its marks and its edits, and finds it again when the disk comes back,
  even at another mount point or drive letter. The volumes are scanned in
  the background at start, when a disk is plugged in or out, and every
  five minutes. Repair never marks an offline photo missing.

## [0.71.0] - 2026-11-07

### Changed

- A raw now gets darktable's colour calibration by default: its white
  balance goes to the camera's D65 reference and colour calibration adapts
  the colours from the light the camera measured when the shot was taken,
  as darktable's scene-referred workflow does. A darktable edit's colour
  calibration set to the camera's illuminant now carries over. Edits made
  before keep their look, and pictures are unchanged.

## [0.70.0] - 2026-11-07

### Added

- The import window shows what the ticked sources hold, grouped by day, from their embedded previews: photos already in your Library are greyed ("new only" is on), a RAW and its JPEG share one cell, and a count line and a type filter sit above. "Import 2,980" imports exactly the ticked photos.

## [0.69.0] - 2026-11-07

### Changed

- The Compare panel shows each raw as it is edited (its latest version
  developed from the sensor data) instead of the JPEG preview the camera
  embedded in it. The preview is drawn first and replaced once the render
  is ready, which runs in the background and follows each change to the
  edit. An image opened from the library no longer holds the window while
  a raw is developed: its embedded preview shows meanwhile. Pictures show
  as before.
- A raw opened in Portrait retouch or in a blank workspace now gets the
  same default exposure as in the other workspaces (+0.7 EV, with the
  camera's exposure compensation taken back), under the masked
  adjustment. A picture looks as before.

## [0.68.0] - 2026-11-07

### Added

- File › Import… (Ctrl+Shift+I) and the empty Library's "Import photos…" open
  the new import window. Its Sources column lists the cards and cameras
  inserted first (by label and capacity, refreshed as they come and go),
  then recent folders, favourites and "Choose a folder…"; several sources can
  be ticked, and the first card is ticked as it opens. A card inserted while
  the window is closed shows a toast that opens it on the card. Import copies
  a card into the Library and adds a folder where it is.
- A raw shot in portrait is shown and exported upright: a `flip` step, as
  darktable's, applies the camera's orientation. It can be set by hand
  (none, 90° clockwise or counter-clockwise, 180°, mirror), and a
  darktable edit's orientation carries over. A darktable edit's drawn
  masks do not follow a turned image yet.

## [0.67.0] - 2026-11-07

### Changed

- A panel's tab dragged to another area of the window now offers the same
  drop zones as inside one area: join a group of tabs, split it on a side, or
  slot the tab between two others. An area a panel cannot go to shows why.
  Dialogs keep their buttons in view when the window is short.

## [0.66.0] - 2026-11-07

### Added

- An import is one undo step: Edit › Undo (Ctrl+Z) with the Library shown
  takes back the photos an import added, with their darktable edits, and
  Redo puts them back. Files are never touched: copies stay in the library
  folder.

## [0.65.0] - 2026-11-07

### Added

- After copying photos from a card, the import says whether the card is
  safe to format: every photo on it is in the Library with a checked copy,
  or how many are not. Helios never formats the card itself.

### Changed

- A card or folder taken out during an import no longer fails every photo
  left: the import pauses, says "Reinsert EOS_DIGITAL to go on" (with the
  card's name), and goes on by itself once it is back. It can still be
  cancelled while it waits.
- Copying photos into the library reads each one from the card only once:
  the copy is checked against what was read, then takes its name. Imports
  from slow cards are faster for it.

## [0.64.0] - 2026-11-07

### Added

- A template's source can now take an image, a picture or a raw: a raw
  file then goes through its sensor data (levels, white balance,
  highlights, demosaicing and camera colours) before the rest of the
  network, instead of its embedded preview.

### Changed

- A raw now opens brighter, as darktable develops it by default: +0.7 EV,
  with the exposure compensation set on the camera taken back. Pictures
  keep their exposure as they are.
- "Develop (preview)", "Batch export", "Import, cull, de-duplicate",
  "Portrait retouch" and a new blank workspace now show and edit a raw
  from its sensor data, as "Develop one image" does, instead of its
  embedded preview. A workspace made before keeps its network: make a new
  one from the template to get this.

### Fixed

- A darktable edit imported with a raw now applies its white balance,
  highlights, demosaicing and input colour settings in every workspace that
  develops raws, not only in "Develop one image"; its white balance was
  left out everywhere before.
- Exporting a raw now always develops its sensor data, at its full size,
  as the darkroom shows it. It is never exported from the small preview its
  camera embedded: a workspace that cannot develop raws fails that photo
  and says why.

## [0.63.0] - 2026-11-07

### Added

- Export photos from the library: File > Export… (Ctrl+Shift+E), the
  Library's context menu or the palette writes the selected photos, or every
  photo the library shows, as JPEG, PNG or TIFF, in the colour space, size
  and file names you choose. Each photo is an export of its own under one
  "Export N photos" job, in the jobs centre and the Queue panel; a photo
  that fails is listed in the Log panel with the reason, the others still
  written, and the end offers to open the folder. A new "Batch export"
  workspace template opens on your picks with the export ready.

## [0.62.0] - 2026-11-07

### Added

- File > Import from → darktable library brings in the photos of a darktable
  library (darktable 4.0 to 5.6): pick darktable's configuration folder, and
  each photo comes with its stars, reject, colour labels, tags, title and
  description, its edits through its sidecars as before. The library is only
  read, from a copy while darktable is running; photos on a drive not plugged
  in are counted in the Log panel.

## [0.61.0] - 2026-11-07

### Changed

- Helios Dark and Light draw far fewer lines: panels have no outline, so
  where the image meets a side panel a single thin line is left. The line
  between two panels of one area, and the scroll bars, show only under the
  pointer.
  
  The drag cursor now shows on hover only where dragging is the only thing
  to do (elsewhere it appears once the drag starts), splitters show a split
  cursor, and a name too long for its row ends with "…" with the full name
  under the pointer.

## [0.60.0] - 2026-11-07

### Added

- Plugins can now add importers: each appears under File > Import from, asks
  for the folder or file it reads, and brings the photos into the library with
  the ratings, labels and tags it knows, without overwriting what the library
  already holds; what it found and left out is listed in the Log panel.

## [0.59.0] - 2026-11-07

### Added

- Importing photos is always within reach: File > Import… (Ctrl+Shift+I),
  an "Import photos…" button in the empty Library and Filmstrip, and files
  or folders dropped on the window. A workspace without an import source
  asks where the photos go (referenced in place, or copied into the library
  folder) and can remember the answer. Imports also bring each photo's
  darktable edits (its `.xmp` sidecars, duplicates included) in as versions,
  on by default; what darktable had that could not be applied is listed in
  the Log panel, which now shows import reports. The Vault panel is now
  called Library.

### Fixed

- In a narrow side bar, the Masks panel keeps each row's eye and lock in
  view, and the Vault's search bar and empty-library text wrap instead of
  running past the panel's edge. A macro's buttons in the Modules panel wrap
  under its name the same way.

## [0.58.0] - 2026-11-06

### Changed

- Helios moves to the latest egui-pop. Every draggable shows a cursor for its
  kind of drag, panel content no longer shifts when hovered, and Helios's own
  lists (queue, parameters, modules, decisions and others) scroll with Page Up,
  Page Down, Home, End and the arrows, their bars in the theme's colours.
  An undo group a command opens is now one step of the image's history, and
  the default themes give plots Rerun's grid and time cursor.

## [0.57.0] - 2026-11-06

### Added

- Experimental: with workshops on, the Workshops panel lists the workshops
  Helios ships, your library's and the workspace's, finds them by what you
  want (the name first, then the keywords, then the intent), applies one
  from its row, and shows an applied workshop's sliders with their targets
  and an "Aim again" button that aims it on the open image as one undo
  step. Agents get `helios_workshop_suggest` and `helios_workshop_aim`.
- Experimental: with workshops on, "Save as workshop…" in the Modules panel
  turns a macro into a workshop: a form asks what it is for, its keywords
  and, per slider, a target the image should measure (prefilled with what
  the open image measures now), saved as the macro's next version in one
  step that Ctrl+Z takes back. Publishing it to your library still asks
  first. Agents get `helios_workshop_save`.

### Changed

- Helios looks like Rerun by default: Helios Dark and Helios Light take the
  Rerun viewer's colours, corners and sizes (near-black or white neutral
  greys, Rerun's blue as accent; values from its re_ui, MIT OR Apache-2.0),
  and which one shows still follows the system's light or dark mode. The
  previous look, warm greys with the sun gold accent, stays available as the
  Helios Classic Dark and Helios Classic Light themes (System › Themes, or
  "Theme: Classic Dark" in the command palette).

## [0.56.0] - 2026-11-05

### Added

- Experimental: with workshops on, the `workshop.apply` command (and the
  `helios_workshop_apply` tool for agents) inserts a workshop after the node
  the image is shown from, its sliders aimed on the open image, as one step
  that Ctrl+Z takes back whole; with workshops off, agents are told which
  setting turns them on.
- Experimental: a macro's file may carry a workshop block, what it is for and
  the brightness or clipping each of its sliders aims at, so a workshop gives
  each image its own values. Agents see it in the description of the shipped
  composites.
- Experimental: Helios ships a first workshop, Lift midtones (an exposure then
  a tone curve), whose brightness is aimed so that the image's midtones land
  at the same level whatever the photo; with workshops off it is a plain
  macro in the graph's Add menu.
- Experimental: a Workshops setting (Interface, Experimental, off by default)
  adds a Workshops panel, empty for now, at the next start; it will find,
  apply and aim macros by what they are for. `helios-gui --set KEY=VALUE`
  gives a setting for one run, such as `--set interface.experimental.workshops=true`.

## [0.55.0] - 2026-11-04

### Added

- The file picker lists the system's folders (Desktop, Documents, Pictures...)
  and the mounted drives in its side bar, lets a bookmark be renamed or moved,
  and has more keys: Alt+Left and Alt+Right for back and forward, Ctrl+H for
  hidden files, F5 to read the folder again.
- Help > Report a problem (also in the command palette and the About dialog)
  opens a new issue on Helios's GitHub repository in the browser, already
  filled with the version and the platform.

### Changed

- The keyboard goes further: the widget the keyboard focuses shows a ring,
  toolbars, the activity bar and the status bar are each one Tab stop whose
  tools the arrows go through, Shift+F10 or the Menu key opens the focused
  widget's context menu, F10 opens the menu bar, and Ctrl+Page Down and
  Ctrl+Page Up go to the next and previous tab of a panel group.
- In the History panel, a snapshot not saved yet keeps its own name, followed
  by "(not saved yet)" in italics, and its tooltip says it is kept at the
  edit's next save. Agents reading the history get a `pending` flag on each
  snapshot instead of a name to parse.

## [0.54.0] - 2026-11-04

### Added

- Hovering a step or a snapshot in the History panel shows the image as
  it was there, with what differs in the tooltip; moving off it shows the
  live image again. Nothing is changed or saved by a preview.
- Macros: Publish… in the Modules panel copies a macro's version into
  your library (`<config>/composites/`), and every workspace's graph
  offers it in its Add menu, under "Macros ›". A dialog asks first and
  says where the file goes: publishing is not undone (remove the file to
  take it back), and a later change is published as a new version.
  Agents use `helios_macro_publish` (with `dry_run`); the file is written
  only once you accept.

## [0.53.0] - 2026-11-04

### Added

- Macros: folding a macro changed while open asks what to do with the
  changes: save them as the macro's next version (the default), expose
  them, discard them, or keep it open; each is one undo step that keeps
  the image's look. Other nodes on the macro keep their version, and the
  Modules panel offers them the newer one ("Update to 2"), carrying their
  values by name. Agents pass `changes` to `helios_macro_fold`.

### Fixed

- The graph's edits (add, connect) know the workspace's own macros: a
  second node of a macro made in the workspace can be added and wired.
- Macros: moving a macro to another version (Update, or a new interface)
  keeps the image's values that did not need to change; they went back to
  the macro's defaults on the desktop.

## [0.52.0] - 2026-11-04

### Added

- History snapshots: "Take a snapshot" in the History panel names the
  edit as it is, kept with the image once the edit is next saved (shown
  "not saved yet" until then). A click restores it as one step that Undo
  takes back, with a toast saying so. Agents have
  `helios_history_snapshot` and `helios_history_restore`.

## [0.51.0] - 2026-11-04

### Added

- Macros: Interface… in the Modules panel (and next after making a macro)
  chooses the parameters a macro shows, with their names, labels, groups,
  ranges and defaults, saved as a new version of the macro as one undo
  step that keeps the image's look. Agents have
  `helios_macro_interface`.
- The Parameters panel shows a macro's parameters under the groups its
  author gave them, each heading folded and unfolded by a click.

## [0.50.0] - 2026-11-03

### Added

- Macros: Ctrl+G in the graph (or Make macro… in the Modules panel) packages
  the selected nodes as one node, keeping the image's look; Ctrl+Enter opens
  it in place or folds it, Ctrl+Alt+G dissolves it. Each is one undo step,
  and agents have the same as MCP tools.

## [0.49.1] - 2026-11-03

### Fixed

- Editing the workspace's network with an image open (adding or removing
  a node, a reset, an import) no longer clears the image's undo history:
  the image stays open through the new network with its values, and the
  edit can be undone like any other step.

## [0.49.0] - 2026-11-03

### Added

- A composite file can now say how each parameter it exposes is shown: a
  label, a description, a unit, a section, a narrower range, a soft range,
  the options a choice keeps and a default. Exposed sliders also keep the
  inner module's unit, steps and soft range.

## [0.48.0] - 2026-11-03

### Added

- What Redo would have made again is no longer lost when a new step
  follows an undo: the History panel's "replaced" toggle shows those
  steps, and a click on one brings the edit back there.
- The History panel's "Undo this step" takes back one step of an image's
  edit alone, even under later steps, as long as none of them changed the
  same values; it is then on the redo list. Agents reach it as
  `history.undo_step`.

## [0.47.0] - 2026-11-03

### Added

- A workspace template may say which images its library opens on: a
  `library.query` with a minimum rating, colour labels, flags and tags (the
  picks only, for instance), checked against its schema. The workspace's
  library settings (File › The workspace's library) take it as typed in the
  search field, `flag:pick rating:3`, and an undo takes it back.

## [0.46.0] - 2026-11-03

### Added

- A new built-in workspace template, "Develop one image", develops a raw file
  from its sensor data rather than from its embedded preview: its levels,
  white balance, highlights, demosaicing and camera colours, then exposure
  and a tone curve. "Develop (preview)" stays for JPEG, PNG and TIFF pictures.
- A workspace template can now say that its image is a raw file's sensor
  data rather than a picture: its network then starts from the raw itself,
  with the camera's levels, colour filter, white balance and colours handed
  to the modules that develop it.

## [0.45.0] - 2026-10-29

### Changed

- `helios import-darktable` now keeps a darktable sidecar whose photo is
  missing from its folder, instead of only listing it: the vault keeps its
  whole text and what the import made of it, so nothing a darktable user had
  is lost.
- A darktable edit set to a mode Helios does not have yet (highlights
  reconstruction other than clip, a demosaic method other than RCD or
  Markesteijn, sigmoid's RGB ratio) is still applied with the closest mode
  Helios has, and now says so: the import report lists it under
  `substituted`, with the mode asked and the mode used, the history step
  records it, and the import's warning names it.

## [0.44.0] - 2026-10-29

### Added

- The file picker shows a preview of the selected file: images, text,
  Markdown and folders, and the system's thumbnails for other files, RAWs
  included where the system has a thumbnailer for them.

### Changed

- When the History panel cannot undo, redo or jump to a step (a drag still in
  progress, a step that is not yours), it now says why in a short notice.

## [0.43.0] - 2026-10-29

### Added

- Agents can hold the places and modes of the desktop that has a workspace
  open, with the new `helios_context_hold` and `helios_context_release`
  MCP tools: while held, moving to another place or mode is refused with
  the agent's reason. `helios_context_read` lists the holds.
- Sub-workspaces can be entered: a double click on one in the graph, `I`
  with the graph focused, quick open or `subworkspace.enter` shows its
  nodes only, in the Graph, Modules and Parameters panels, its name in the
  breadcrumb and a layout of its own. Inside, a value of a node outside
  it cannot be changed, by the user or an agent; `U` goes back up. A
  sub-workspace deleted while shown, or gone when the workspace opens
  again, is left with a notice.
- Tips: on the desktop, a small card now and then teaches one thing about
  Helios (the command palette, full screen, the zoom keys, typed values,
  ratings and picks, the mask display, profiles...), showing the keys as
  you bound them. A tip for a command you already use never shows, and
  none comes twice in a day; "Got it" dismisses one for good and "No more
  tips" all of them. Help > Tips… lists every tip. In English and French.

## [0.42.0] - 2026-10-28

### Added

- A batch running on a node can now be steered while it runs:
  `helios control workflow-cancel`, `workflow-retry`, `workflow-prioritize`,
  `workflow-pause` and `workflow-resume` (and the matching MCP tools)
  cancel or retry some of its images or all of them, move images to the
  front or the back of the queue, and pause or resume it; a paused batch
  stays paused when the node restarts.

### Changed

- Ctrl-C on `helios batch` attached to a node now cancels only its own
  images: images another client added to the same run go on.

### Fixed

- A PNG tagged Rec.2020 by its cICP chunk with the 10- or 12-bit BT.2020
  transfer code, as video tools write it, is now reported as Rec.2020 rather
  than by its raw code points. Its colours were already read right.
- Colours on the edge of an export's colour space, such as the pure blue of a
  loaded sRGB picture, are now written unchanged. The rounding in colour
  profiles places them a hair outside the space, and the gamut mapping then
  drained part of their saturation: a reloaded sRGB blue exported again came
  out a duller, greener blue.

## [0.41.0] - 2026-10-28

### Added

- Help > About Helios opens a dialog with Helios's logo, version, licence and
  source code link, and a "Copy version info" button for bug reports; the
  system menu's About opens the same dialog. Agents read the same facts with
  the `about.read` command, and scripts open it with `about` instead of
  `workbench.about`.

## [0.40.0] - 2026-10-28

### Added

- The graph view's add menu offers composites (such as the masked
  adjustment) next to the modules, each at its newest version.
- Selecting a node in the graph view brings up the Parameters panel, opening
  it again if it was closed, so its settings are at hand.
- The graph view keeps the wire style you picked (curves, orthogonal or
  straight) from one session to the next.
- Agents edit a workspace's network node by node through the MCP server:
  connect, disconnect, move, add, remove and paste copied nodes, each one
  step they can undo, with a dry run. When a desktop holds the workspace
  open, the edit shows in its graph view at once and is one step of its
  Edit > Undo.

### Changed

- `helios batch` now hands its images to the node as one batch the node
  works through itself: running the same batch again only exports what is
  new, and with `--vault` a batch interrupted or cut short by a crash is
  finished by running it again, writing no file twice (`--again` starts
  over). A batch that needs a person to answer a decision now ends with the
  images left reported, instead of waiting.

### Fixed

- A node added in the graph view (or by an agent) is kept while it waits
  for its wires, instead of the addition being refused; only the nodes the
  image or an export reads from need every input wired.
- The graph view's node headers follow what the viewport renders: a node
  reads done as soon as the tiles on screen are computed instead of idle
  until a full-size render, and says cached when it was served from the
  cache without computing.

## [0.39.0] - 2026-10-28

### Added

- An agent's edit through the desktop (a workspace, network, sub-workspace
  or paste tool) is one undo step, named with the agent in the History
  panel. An agent can pass the same `group` name to several calls to make
  them one step labelled with that name; a step that changed several
  values says how many.
- "Undo an agent's last changes alone" in the command palette takes back
  what an agent last did even after you went on working, unless you have
  changed the same values since. Agents undo, redo and jump only through
  their own steps, and can undo one of their batches alone with
  `helios_history_undo` and its `group`.

## [0.38.0] - 2026-10-28

### Added

- A History panel lists the edit's undo steps, newest at the bottom, the
  steps a redo would bring back muted, an agent's named with its origin; a
  click goes back or forward to that step. It sits in the end side bar,
  closed until opened from the command palette. Agents read and walk the same history
  through the new `helios_history_*` MCP tools.
- JPEG, PNG and TIFF pictures now open in the colour space they are tagged
  with (an embedded ICC profile, PNG's colour chunks, or a camera's Adobe RGB
  setting), so their colours are no longer shifted. Untagged pictures are
  still read as sRGB, and the load report says so; a broken profile does not
  stop the load, the report gives the reason.
- A batch kept in a vault now survives its node: started again after a
  crash, the node finishes the batch without writing any file twice, and a
  second node on the same vault takes over the batches of one that died.
  Finished batches are removed after 30 days (`system.workflow.keep_days`),
  or at once with `helios control workflow-forget`.
- An undo, a redo or a jump that takes back a change made in another place
  (the library, the darkroom, a workspace's network) says where in a toast,
  "Undone in <place>", with Go there to go back to it.

### Changed

- Colours an export's colour space cannot hold (for instance the most
  saturated reds and greens of a wide-gamut edit, exported to sRGB) now keep
  their hue and lightness and lose only saturation, instead of each channel
  being clipped on its own, which shifted hues. Colours the space can hold
  are written unchanged.

## [0.37.0] - 2026-10-28

### Added

- A node can now take a batch as a run of the workflow level: each image is
  exported by the node itself, retried when a second try may help, and
  followed per step, and submitting the same batch again redoes nothing that
  is done. Output file names are decided once, when the batch is submitted.
- Agents can read and use the clipboard through MCP: `helios_clipboard`,
  `helios_copy` and `helios_paste` copy from and paste into the desktop
  holding a workspace (a parameter, the graph, the masks, the library),
  each with a dry run, in a slot of the agent's own: an agent never reads
  or overwrites what you copied unless you allow it.
- Files and images now go between Helios and other applications. Edit ›
  Copy files puts the selected images' files on the clipboard as a file
  manager copies them; Edit › Copy the image puts the edited image,
  rendered whole, there for an image editor or a chat. Edit › Paste from
  other applications imports files a file manager copied, or an image a
  browser or an editor copied (saved as a PNG file in the library folder),
  once you confirm.
- Every panel has an icon: a folded side bar shows its panels as icons
  instead of the first letter of their names.

### Changed

- The Masks panel, the Scopes and the Navigator open as floating palettes
  in a new layout, stacked together against the image's right edge
  beside the side bar, out of the image's centre; they dock back from
  their header and a saved layout keeps where they are.
- Icons are now drawn from the Phosphor icon set, and more commands carry
  one: the culling ratings, flags and next image, copy and paste, import and
  export, the views and more of the mask tools.

### Fixed

- In a floating Masks panel, a row clicked keeps the keyboard: Delete and
  F2 act on it as they do with the panel docked.

## [0.36.0] - 2026-10-28

### Added

- Menus and the command palette show an icon beside the commands it fits:
  undo, redo and reset, opening and creating a workspace, zoom, the culling
  flags, the mask tools and the window's buttons.
- Exported JPEG, PNG and TIFF files now carry the colour profile of the space
  they are written in, so other applications show their colours as Helios
  meant them. PNG files also say their colour space in the newer cICP tag.
- The title bar has layout controls, as in VS Code: a button opening the
  layout menu (arrangements, the side bar's side, the bottom panel's width)
  and one toggle per side bar and for the bottom panel.
- In the darkroom, hovering the status bar's zoom offers Fit, Fill and 100 %,
  and hovering its history offers Undo and Redo. On a narrow window the image
  counts stay in the bar longest; the other items go into its "…" menu.
- Helios follows the desktop's "larger text" setting (GNOME and KDE Plasma on
  Wayland, Windows) for its text size, until you pick a size of your own in
  the settings.

### Changed

- Ctrl+Z in the theme editor now undoes the editor's last change instead of
  the image's. Menus keep their entries aligned in an icon column, with ticks
  and radio dots.
- The interface is now drawn in Inter, with values and code in JetBrains Mono
  NL (no ligatures, digits of equal width), both bundled with Helios.

## [0.35.0] - 2026-10-28

### Added

- Images can be copied from the library grid or the filmstrip and pasted
  into another library: the images it lacks are imported from their files
  once you confirm, and paste special chooses whether the files stay where
  they are or are copied into the library folder. Files and folders copied
  in a file manager can be pasted into the library to import them the same
  way.

## [0.34.0] - 2026-10-28

### Changed

- The mask display (the coloured overlay and the grey mask) is now laid over
  the image on the graphics card, so showing a mask no longer sends the picture
  back to the processor when the edit runs on the GPU.

## [0.33.0] - 2026-10-27

### Changed

- The image view now computes on the graphics card that draws the window and
  shows the result straight from there: an edit made of modules that run on
  the GPU no longer copies the picture back to the processor and up again.

## [0.32.0] - 2026-10-27

### Added

- Drawn mask shapes can be copied and pasted, from the image or the Masks
  panel, onto the same image or another one: pasted shapes are scaled to
  the image's size, and the paste warns when its proportions differ.
- Nodes of the graph view can be copied and pasted, with their current
  parameters and the wires between them: a pasted node whose name is taken
  gets a new one (exposure_2), and paste special can take some nodes only.
- A parameter can be copied and pasted: Ctrl+C and Ctrl+V on a focused
  slider, or Copy value, Copy path and Paste in its context menu. A copied
  value reads as it is shown ("+0.30 EV") and now pastes back, onto any
  parameter of the same kind, as one undo step.

### Changed

- Helios now starts on the Photoshop pointer profile, which egui-pop split
  from its former Adobe one: the wheel pans the image (Ctrl and the wheel
  sideways), Alt and the wheel zoom, a drag pans as the Hand tool does, and
  Shift steps a value coarser and Alt finer. A slider still takes the wheel
  only once focused. A saved Adobe profile is read as Lightroom Classic,
  which stays a choice in the settings.

## [0.31.0] - 2026-10-27

### Added

- vkdt's colour module (`vkdt_colour`): the camera's matrix, white balance
  towards the camera's white (as shot, or set by hand), exposure, saturation
  and highlight clipping. A chain using it leaves out Helios's own white
  balance, since vkdt balances here. It runs on the GPU, and the nightly
  check compares it with vkdt's own render.
- vkdt's film curve (`vkdt_filmcurv`): a Weibull tone curve that compresses
  scene light for display, its colour kept by vkdt's default hue-preserving
  step or applied per channel. It runs on the GPU, and the nightly check
  compares it with vkdt's own render.
- vkdt's colour grading (`vkdt_grade`): lift, gamma, gain and offset per
  channel, applied to all tones alike or weighted by zone (shadows,
  midtones, highlights). It runs on the GPU, and the nightly check compares
  it with vkdt's own render.

### Fixed

- The modules ported from vkdt now carry vkdt's copyright notice and its
  BSD-2-Clause licence, shipped with the source, and each ported file names
  the vkdt sources it comes from.

## [0.30.1] - 2026-10-27

### Fixed

- A slider drag held for more than a second and then cancelled with Escape is no longer left as the latest edit if the application quits or crashes right after.
- Each saved step of an edit now records who made it (you, an agent, a script, or the decision answered on a node) instead of always naming the desktop user or the control client.
- Undo after opening another image no longer reaches the previous image: its steps could write its values into the new one.

## [0.30.0] - 2026-10-27

### Added

- Agents and scripts can bypass a node and bring it back (the MCP tool
  `helios_node_bypass`, `helios control bypass NODE on|off`): the node keeps
  its parameters and the image renders as if it were absent. A bypassed node
  is saved with the image's edit, and a template can place a node bypassed.
- The Modules panel lists the modules of the image's network: a click
  selects one, and its Bypass toggle (or Ctrl+/ on the selected module)
  renders the image as if the module were absent, keeping its settings. A
  bypass is one undo step and is saved with the edit.

## [0.29.0] - 2026-10-27

### Added

- A node's exports now say which node of the graph they render, so the
  desktop can list each node's work on its own.
- The Queue panel now shows the work queued at the selected node: each
  export with its state, since when, its attempts and where it runs, with a
  summary and Cancel. With the "Queue panel without a queue" setting off,
  the panel closes while the node has nothing queued.

## [0.28.0] - 2026-10-25

### Added

- A "darktable classic" keymap package adds darktable's own keys over the
  defaults in one step (F1 for the red label, L for the library, Ctrl+Shift+L,
  R and B for the panels) and comes off the same way; the keys it cannot
  take, and why, are listed in the GUI notes.
- A default keyboard map: function keys F2 to F5 label images beside 6 to 9,
  Space and Backspace step through images while culling, Alt+1, Alt+2 and
  Alt+3 zoom the image to 100 %, fill and fit, Ctrl+B, Ctrl+Alt+B and Ctrl+J
  fold the side bars and the bottom panel, and Ctrl+K Ctrl+S opens the
  shortcuts editor. Culling keys no longer act in the darkroom, and a culling
  key taken away in the shortcuts editor stays away.
- Settings › Appearance has a theme editor: the theme in use edited token
  by token, with each colour's contrast, undo and redo, changes shown at
  once, and the dark and light themes edited as a pair. Themes are opened
  and saved as token files.

### Changed

- The command writing the theme in use with its overrides merged into one
  file is now `theme.exportMerged`: `theme.export` is the theme editor's,
  which exports only the tokens changed. Keys bound to the old name need
  binding again.

## [0.27.0] - 2026-10-22

### Added

- A decision waiting for an answer now shows as a card in the new Decisions
  panel: the model's proposal with how sure it is and why, Accept, Correct
  with a form for the answer, and "accept all above" a confidence. A badge
  in the status bar counts the decisions waiting, and agents answer them
  with the `decision.*` commands.
- A model's proposal for a decision now keeps how sure the model is and why,
  so whoever answers it (in the desktop or through a node's decisions list)
  sees them beside the options.

## [0.26.0] - 2026-10-22

### Added

- The Graph panel shows the workspace's network: move, connect, add and remove nodes, each change one undo step saved with the workspace; node headers show the engine's status, the overview folds nodes into stacks under their principal node, and the selection follows the Parameters panel both ways.

## [0.25.0] - 2026-10-22

### Added

- The first time Helios starts, a small notice asks "Coming from darktable?": one click sets darktable's mouse and wheel habits, another keeps Lightroom's. It never blocks the window, shows once, and the choice can be changed later in Settings › Input.

### Changed

- The wheel scrolls the panels again by default: Helios now starts with Lightroom's mouse habits (Adobe's pointer profile), where a slider takes the wheel only once clicked, so scrolling past sliders never changes a value. A pointer profile you already chose is kept, and darktable's stays one choice away in Settings › Input.

### Fixed

- With a pointer profile whose click zooms the image (Adobe's), drawing a path or clicking a shape's edge no longer zooms the image too.
- A parameter's tooltip now names the finer and coarser keys and when the wheel changes the value as the pointer profile in use has them, instead of always darktable's.

## [0.24.0] - 2026-10-21

### Added

- Settings › Appearance › Fonts sets the text size, from 85 % to 200 %, and the interface and monospace fonts; numbers on sliders no longer jitter while dragged.
- Floating palettes can be stacked together or grouped as tabs, anchored to the window's edges, and resized by dragging their edges.

### Changed

- The mouse now works as in darktable: the wheel over a slider changes it (Ctrl+Alt and the wheel scroll the panel), a middle click on the image cycles 100 %, 200 % and fit, and a double click on the image goes back to where it was opened from. Settings › Input picks another pointer profile, or keeps the wheel over sliders from changing values.
- The status bar starts with where you are, from Helios to the image open in the darkroom, and a coloured chip for each mode on (culling, mask editing, compare). It then shows what matters there: the number of images in the library, the zoom and the history in the darkroom, the picks and rejects while culling. A right click on an item hides it or moves it, for that place.

### Fixed

- Opening an image from the library shows it: the darkroom starts from the workspace's default layout, the image in the centre, instead of keeping the library's grid.
- Keyboard shortcuts now work in a detached panel's window, and a dialog opens over the window you were working in.
- Dock tab labels are no longer garbled after switching between the light and dark themes.

## [0.23.0] - 2026-10-21

### Added

- Agents can export a mask as a grey picture of its values (black where the
  mask is 0, white where it is 1) with the export tool's new `as_mask`
  option, the same grey the mask display shows.
- Press M with the image focused (or use the Masks panel's Show mask button,
  or the palette) to see a mask on the photo: the member or group selected
  in the Masks panel, else the module's whole mask, as a coloured overlay
  or alone in grey. It stays on while you edit, and themes set its colour.

## [0.22.0] - 2026-10-21

### Added

- Part of a workspace's network can be delegated to a named sub-workspace, nested if wanted, and deleted again, each as one undoable step with a dry run; templates and workspace edits carry them, and importing a network copies its sub-workspaces. Entering a sub-workspace comes in a later release.

## [0.21.0] - 2026-10-21

### Added

- A third built-in workspace template, "Develop (preview)", sets exposure and
  the sigmoid tone curve on a picture, with the image large and its
  parameters and scopes beside it. A raw file is developed from its embedded
  preview for now, not from its sensor data.
- Workspace templates opened on the desktop may now use the sigmoid tone
  mapper, which maps scene light to the display's range.

## [0.20.1] - 2026-10-21

### Fixed

- An agent that asks for the running jobs right after seeing a job start on the event stream now always finds that job in the answer.

## [0.20.0] - 2026-10-20

### Added

- Settings > Appearance has an accent colour: pick any colour and the theme builds its accent shades from it, in light and dark.
- The theme editor can preview a draft in its own pane before applying it, generate a palette from one colour, and add font files in a new Fonts tab.

### Fixed

- After switching themes several times, docked panels no longer keep the colours of the other light or dark variant.

## [0.19.0] - 2026-10-20

### Changed

- User kits and themes read from disk are compiled once and kept in your cache folder, so the next start reads them faster. A theme file with a broken or unknown token now loads with a warning instead of failing.

## [0.18.0] - 2026-10-20

### Added

- Helios's light and dark themes are now a pair: Settings > Appearance can follow the system's light or dark mode, or keep one, and View > Toggle light and dark switches between them.

## [0.17.0] - 2026-10-20

### Added

- In a narrow window, the menus that no longer fit in the menu bar move into a "…" menu instead of being cut off, and Alt with a menu's letter opens it wherever it is.

## [0.16.0] - 2026-10-20

### Added

- Agents can read and move a workspace's places and modes through MCP (helios_context_read, _request, _go and _set_mode), each with a dry run and a revision check: a desktop that has the workspace open moves at once, otherwise a desktop opening the workspace later comes back where the agent left it.
- The window now knows where you are and how you work: the library, the darkroom on an image and the workspace's network are places with their own arrangement, and culling, mask editing and compare are modes. A breadcrumb and mode chips in the status bar show them; Alt+Left goes back, Escape leaves a compare or a path being drawn, and View offers the library, the network and culling, whose keys then work from anywhere.
- A workspace opens again where it was left, even after a crash: the same place, the image open in the darkroom again, culling and compare as they were. An image that was deleted or moved since, or a network that is another workspace's, is left out with a message, and the rest comes back.

## [0.15.0] - 2026-10-20

### Added

- The title bar shows the jobs going on, the stack suggestions waiting for
  an answer and the profile in use, in full screen too: a click opens the
  Jobs panel, the Stacks panel or the Profiles menu.
- `Ctrl+K Z` enters zen mode, the image alone on the screen; Escape
  brings the bars and panels back.

### Changed

- In full screen the title bar stays, with its menu, logo and buttons: a
  button leaves full screen in place of maximize. Zen mode (View > Zen
  mode) shows the image alone, Escape brings the workbench back.
- A tab dragged in the workbench shows a ghost of itself under the
  pointer, drawn by the theme's kit.

## [0.14.0] - 2026-10-20

### Added

- The keyboard shortcuts editor can change keys for the open workspace alone
  as well as for every workspace (the default), and says after each key
  whether it comes from the workspace's template, the workspace or you. A
  workspace's own keys are kept with it and undone with Edit > Undo.

### Fixed

- An agent editing a workspace that is open on the desktop now goes through
  the desktop: the change shows at once, is one step of Edit > Undo, and the
  desktop's next save no longer overwrites it. A workspace's files changed
  outside Helios are reloaded as one undo step, keeping what you changed
  meanwhile unless the same part changed on both sides, which a warning names.

## [0.13.0] - 2026-10-20

### Changed

- With a mask group selected, the Parameters panel names its operator by
  its effect (union, intersection, …) with a small preview, and lists each
  member's opacity and inversion on a line of its own, by name, instead of
  "8 values".

### Fixed

- A parameter offered as a list of choices in the Parameters panel now
  opens when clicked; it used to close at once, so its value could not be
  changed there.

## [0.12.0] - 2026-10-19

### Changed

- Helios runs on egui-pop's micro-indicators: status dots, badges, flags
  and swatches are drawn by the theme's kit, with shapes that keep each
  state apart without relying on colour alone.

## [0.11.0] - 2026-10-19

### Changed

- Helios runs on egui-pop's visual kits: every themable part of the
  interface (frames, menus, tables, progress and status indicators) is now
  drawn from the theme's data, ready for deeper themes.

## [0.10.2] - 2026-10-18

### Fixed

- With the interface in another language, Helios's own entries in the menus
  (such as opening a workspace in a new window) now show in that language
  too, instead of staying in English.

## [0.10.1] - 2026-10-18

### Fixed

- The workspace entries of the menu (edit, library settings, reset, save as
  template, add parts) now show a short name instead of a long sentence
  written for agents.

## [0.10.0] - 2026-10-18

### Added

- Every event an agent or a script reads from Helios (the inspect endpoint, the
  MCP tools) now says when it happened, as a wall-clock time and as a monotonic
  time for measuring delays.

## [0.9.1] - 2026-10-18

### Fixed

- Launching Helios from the desktop no longer keeps a busy cursor for
  several seconds after its window is up.
- The window manager's focus setting is read while the desktop starts, and
  a desktop slow to answer no longer holds the window up: past a quarter
  of a second the window opens with click to focus.
- The desktop opens its GPU device while it reads its settings and the
  vault, instead of after, so its window comes up sooner.
- An image named at launch, or handed over by a later launch, is decoded
  in the background: the window opens at once and says "Opening…" in the
  status bar until the image shows. An image that cannot be read is now
  said in the window rather than stopping the launch.
- The library is read from the vault while the window opens, instead of
  before it: a large library no longer holds the window up, and the status
  bar says when it is being read and how many images it holds.

## [0.9.0] - 2026-10-18

### Changed

- A workspace created from "Import, cull, de-duplicate" shows the jobs
  centre beside the filmstrip, so an import's progress is in view while
  culling.
- The Vault panel keeps its search field, import button and marks bar in
  place while the library is empty, instead of showing them only once
  images are imported.

## [0.8.0] - 2026-10-14

### Added

- Hovering a mask handle on the image now says its gestures in the status
  line: what Shift snaps, Ctrl for fine moves, and when they apply a
  double click to reset and the wheel to adjust.
- In the Masks panel, a blur, feather or levels refinement is now a row of
  its own under what it refines: select it to edit its parameters. A
  member wired to two ports of a group shows once per port.
- A template can now ask for values when a workspace is created from it
  (the import source, a file it refers to), and carry an icon shown beside
  its name.
- A workspace can now be edited after it was created, and the result is
  kept: its name, description and icon, its library settings, its keys and
  tools. The File menu saves it as a template (after a review of what goes
  in and what is left out), resets it to its template, and adds parts of
  another workspace or template, each as one step that Undo takes back.
- Agents can now work on a workspace through MCP: list its parts, edit
  it, reset it to its template, add parts of another workspace or
  template, and save it as a template, each with a dry run first and an
  undo after.

### Changed

- A mask member's row now says its inversion and opacity in its group in
  words ("inverted", "opacity 30 %", "hidden"), in the interface's
  language, rather than a bare percentage.
- The Masks panel names a group's operator by its effect (union,
  intersection, subtract from the first, exclude, multiply...) rather
  than its formula, with a small preview of what it does to two shapes.

### Fixed

- A workspace's panel arrangement is saved as soon as it changes, not only
  when Helios closes: a crash or a forced quit no longer loses where the
  panels were.
- The Masks panel's buttons, hints and messages are now translated into
  French: some had been left in English.

## [0.7.0] - 2026-10-13

### Changed

- The Masks panel names the module whose mask it shows, and an empty mask
  now says that the module applies everywhere until a shape or a parametric
  member limits it, instead of showing a lone row.
- The Masks panel's buttons are translated and their hints give the keys
  and gestures that do the same (F2, Delete, drag and drop, C, E, G and P on
  the image); the delete button is now called Take out, as in the row's
  menu, and warns that it is not an undo step yet.
- Taking a group that holds members out of a mask now asks first, since its
  members go with it and taking out cannot be undone yet. Shapes and empty
  groups are still taken out at once.

### Fixed

- The Masks panel now shows a blur, feather or levels applied to a module's
  whole mask on the mask's own row: everything that shapes the result is
  listed.

## [0.6.0] - 2026-10-13

### Added

- A process's inspect endpoint pushes its events as they happen
  (`subscribe` with a kind prefix), and a client that reconnects picks up
  from the last event it saw, told when some were missed.
- Cancel in the jobs centre stops a node's job on that node: a batch run
  by `helios batch` can be stopped from the desktop.
- A node shows the jobs it runs now and the last ones that ended
  (`describe jobs`), so a client that lost touch catches up.
- The desktop follows a running node's jobs (`helios-gui --node
  NAME=INSPECT`): a node's batch shows in the jobs centre with its exports
  under it. When the node goes out of reach its jobs show as lost, then
  catch up once it is back, or leave the list after a minute.

### Fixed

- The batch endpoint's refusal now lists every request it answers, in
  English and French.
- A batch whose `helios batch` process dies no longer shows as running on
  its node for ever: the node ends it as abandoned when the process's
  connection closes, or after ten minutes with nothing asked for it.

## [0.5.1] - 2026-10-13

### Fixed

- A freshly opened library now fills in its thumbnails as they are read,
  instead of waiting for the mouse to move or a key to be pressed.

## [0.5.0] - 2026-10-13

### Added

- The Masks panel and the Scopes can float over the image as palettes:
  float them from their tab, drag them by their header, dock them back on
  a side, and hide them all with Tab. The Portrait retouch template opens
  with the Scopes floating over the image.
- A Navigator panel shows the whole image with a frame around the part in
  view: drag the frame or click elsewhere to move the view, zoom from its
  buttons or the wheel, double click to fit. It can dock in a side bar or
  float over the image.
- Ctrl+P opens quick open: type the name of a module of the image's
  network, an image of the library or a recent workspace, and Enter goes
  there. Every command stays in the command palette (Ctrl+Shift+P), with
  its shortcut.

### Changed

- The histogram now measures the edited image as the pipeline outputs it,
  shows when shadows or highlights clip, and moves the exposure when you
  drag it, as in darktable: one undo step per drag, Escape to give it up.

## [0.4.0] - 2026-10-13

### Added

- The library is a culling tool: a fast thumbnail grid and filmstrip,
  Lightroom's culling keys (0 to 5, P, X, U, 6 to 9) with optional
  auto-advance, picks, rejects, colour labels and tags saved in the vault, a
  search with filter chips, and a Compare panel for two to four images.
- A Duplicates panel lists the exact duplicates an import found and what
  its policy did with each. With the `ask` policy, each file now waits for
  your answer there: keep it as one more copy, or skip it.
- A second built-in workspace template, "Import, cull, de-duplicate": the
  library grid with Compare and the loupe, the stack review and the
  duplicates beside it, a card copied and renamed on import with its
  duplicates asked about, stacks proposed after each import, and the
  culling keys with auto-advance.
- Stacks show collapsed in the library, expanded with S or their chevron.
  A Stacks panel reviews the suggested stacks: each burst or bracket with
  its proposed top image, to accept all, accept only the ones you check, or
  decline.

### Fixed

- A value typed in a parameter's slider is now kept when you click
  elsewhere, instead of being dropped, and the keyboard goes where you
  clicked. In French, the space between a value and its unit is a narrow
  no-break space, as French typography writes it.

## [0.3.0] - 2026-10-13

### Added

- `helios batch` shows on the node it drives as one batch job with an
  export job per image under it, and cancelling the batch there (`helios
  control job-cancel`) stops it: the exports in flight are cancelled, the
  others are not started, and no file is left half-written.
- An import now shows in the jobs centre with its progress, the file at hand
  and a log line per file, and can be cancelled from there (or with Ctrl-C
  for `helios import`): it stops before the next file, keeps the files
  already imported and leaves no half-written copy.
- A node's exports are jobs people follow on its bus, and `helios control
  job-cancel ID` (the `job.cancel` command, also an agent tool) cancels one
  by its id, queued or running, without writing a file.

## [0.2.1] - 2026-10-13

### Fixed

- Closing Helios no longer keeps the window on screen under a busy cursor:
  the window goes at once, and the catalog is closed after it.
- Thumbnails are scaled down as they are read, so a large library no longer
  slows the window while they load.

## [0.2.0] - 2026-10-13

### Added

- The GPU calibration reports as a job once the compute manager is given
  the jobs (`Manager::report_to`): each device's reference modules as items,
  its speeds as log lines, a device that failed as a failed item, and a
  cancel that stops it between two modules.
- The desktop has egui-pop's jobs centre: the Jobs panel and the status
  bar's jobs item with its popover list every background job of the
  desktop's process (an export, a calibration) with its overall percentage,
  "n of m", the current item, the time left, a log console per job and
  Cancel. Helios's status line is now a block of egui-pop's status bar. A
  long export or batch that ends while no Helios window has the focus is
  said on the desktop from the job's end.
- `helios_core::jobs`: background work reports how it goes as `job.*`
  events on the bus (started, progress with done and total, the current
  item and step, log lines, an item failed, finished, failed, cancelled),
  the shape egui-pop's jobs centre folds; a job is cancelled by its id
  (`Jobs::cancel`). `Scheduler::canceller` cancels jobs without holding the
  scheduler.
- An export reports as a job (`job.*` events on its bus): started when
  queued, its render, encode and write steps, a log line with the file
  written, and its end; `helios_export::submit_reported` takes the jobs
  whose `cancel` stops it, queued or running, writing nothing.

### Changed

- The Parameters panel's sliders are egui-pop's slider family, built from
  each parameter's descriptor: Ctrl drags and steps ten times finer and
  Shift ten times coarser, Page Up and Page Down take coarse steps, Home and
  End go to the soft range's ends, Escape gives a drag up without a trace,
  and Enter types a value or an expression of the current one (`x * 2`).
  A logarithmic bar and a picker button follow the module's hints. Every
  finished change runs the `slider.change` step, so a plugin can veto or
  snap it; a drag is still one undo step and steps in a row one group.

## [0.1.1] - 2026-10-13

### Fixed

- The example plugins (exiftool, gphoto2) load again in 0.1.0: they still
  declared that they work only with Helios 0.0.x.

## [0.1.0] - 2026-10-13

### Added

- An agent sets up local AI models through MCP: `helios_ai_*` tools list and
  add providers, route capabilities, try prompts on an image, probe the
  machine, recommend models that fit, pull them as cancellable jobs, bench
  them and install Ollama for the user, step by step and only when each step
  is confirmed. `helios doctor ai` says what is available and why not, and a
  network can ask its decisions through the routing map (`"resolver": "routing"`).
- AI model providers with declared capabilities (Ollama, any OpenAI-compatible
  server, Anthropic, a local command), a routing map from capability or prompt
  to provider and model, and prompts as versioned files that a workflow can
  pin. Local-first: only Ollama on loopback is known until a provider is added;
  keys stay in the system keyring or the environment, images are downscaled
  before they leave, and every call is logged.
- A mask can hold an AI object member: the subject, the sky, a person or
  what a prompt names, found by a model and recorded with the edit, so the
  edit renders the same mask later without asking again.
- Decision nodes can be answered by a model: any program reading and
  answering JSON, or an OpenAI-compatible or Anthropic endpoint whose key
  comes from the environment, applied at once (full-AI) or kept as a
  proposal to accept (AI-assisted), never asked twice about the same pixels
  and capped by `system.ai.calls`. `helios control decisions` and the
  `decision-*` commands and MCP tools let you or an agent ask, review and
  answer; `decide_exposure` is a first decision module.
- `helios-node --batch-control PATH` serves, in every build, the few control
  commands a batch export needs (give a source a file, export, cancel), on a
  socket only its user can open; the other control commands stay in debug
  builds.
- `helios batch` applies a network to many images and exports each one to a JPEG, PNG or TIFF file, optionally on top of the image's darktable sidecar, with one JSON report line per image. A failed image does not stop the others, existing files are never overwritten, and Ctrl-C leaves no half-written file; it works with release builds.
- Path fields in dialogs have a Browse button that opens the system's file
  chooser, or Helios's own folder picker when there is none. Opening a
  workspace uses it and lists the recent workspaces.
- The render cache stays within a memory budget (a machine setting, 2 GiB by
  default): what was used least recently is evicted and computed again when
  asked for, and content still being computed is never evicted.
- An example plugin drives cameras through gphoto2: list the connected
  cameras, take a picture and download it into a chosen folder, with
  progress.
- Nothing is computed twice: a network shares any content it has already
  computed, or is computing, between nodes, branches and simultaneous
  renders, and a parameter change only recomputes what it affects.
- Compute units: Helios lists the CPU and every GPU of this machine
  (software ones off), with per-device settings kept for this machine only
  (enabled, role, priority, thread, memory and cache budgets), a short
  benchmark, and routing of each render to the device that fits, falling
  back to the CPU and saying where it ran. Remote helios-node units can be
  declared and probed; `compute.*` commands drive it all.
- A running node can take another image for its source and export a node to a JPEG, PNG or TIFF file on request (`helios control load-source` and `helios control export`, also as MCP tools). An export renders the network as it was when asked, and an export cancelled while rendering writes nothing.
- darktable's duplicates of a photo (`IMG_0001_01.CR2.xmp` and so on) now
  import as versions of that photo, each kept apart, next to the base edit,
  which stays the one a batch exports. The new `helios import-darktable`
  keeps a whole folder's edits in a vault, listing sidecars whose image is
  missing and importing photos that have no sidecar. A camera file name that
  merely ends in digits is not taken for a duplicate.
- Helios's exposure is now checked against darktable's own render: the same
  image goes through darktable-cli and through Helios, and the results must
  agree within darktable's colour difference thresholds (they agree to
  within a ten-thousandth of a Delta E). The check runs nightly.
- In debug builds, a running node can be driven from the command line with
  `helios control`: render a node, change a parameter, cancel a job, pause
  and resume work, and make a computation or a source read fail once to test
  what happens on errors. A failure injected this way is never cached.
- A network can hold decision nodes: points where a judgment is needed, made
  by an AI, proposed by an AI and confirmed by a human, or made by a human.
  Until a decision is answered, what depends on it waits, without computing
  anything. Every answer is recorded with who gave it, which model and which
  inputs it saw. A saved edit renders again from its answers without asking
  anyone, and asking again keeps the earlier answers.
- Raw files from Fujifilm X-Trans sensors are now demosaiced with
  Markesteijn's method, darktable's default for them, ported from
  darktable: Helios's render of an X-Trans file now matches darktable's to
  within a ten-thousandth of a Delta E. Blown highlights are clipped a
  touch lower, at darktable's level, so that clipped areas match too.
- Raw files from Bayer sensors are now demosaiced with RCD, darktable's
  default method, ported from darktable: fine detail and edges come out as
  darktable renders them, without the colour fringes of the previous
  bilinear method (still available as `method: bilinear`). X-Trans sensors
  keep the bilinear method until Markesteijn is ported.
- Helios's direction is written down: a roadmap with dated decisions, the
  module contract, the internal architecture, a testing strategy built for
  AI-driven development, and GUI notes backed by research on what users ask
  of darktable and its equivalents.
- The desktop speaks the language of your choice: a Language setting
  (System by default, which reads `HELIOS_LANG` then the system's
  language) switches it while Helios runs, and the menus, the command
  palette, panel titles, settings, dialogs and the shortcuts editor follow.
  Helios ships English and French; a missing translation shows English.
- A first Helios window: `helios-gui [workspace]` opens a workspace in a
  VS Code-like shell, with side bars, editors and a bottom panel docked from
  the workspace's saved layout, panels that can be moved to their own
  window, and a command palette (Ctrl+Shift+P or F1) running every action.
  The interface can picture any panel or named widget, and record a video
  of it, by itself (`gui.snapshot`, `gui.record.start`), or headless with
  `helios-gui --snapshot <target> <file.png>`. `packaging/linux/install.sh
  --dev` adds a "Helios Dev" launcher running a source checkout's build.
- `helios doctor display` says what the display offers: the session and,
  on Wayland, what the compositor's colour management supports, with the
  encoding the image is drawn in and why. The desktop's debug bridge says the
  same beside what its window resolved (`helios.display`), and shows
  egui-pop's `viewport` collector.
- Edits are kept as versions in the vault. Two edits made at the same time
  on the same image are both kept, side by side, instead of one
  overwriting the other, and a merge records where it came from. Writing
  the same edit twice changes nothing. darktable edits imported from XMP
  sidecars become versions too, marked as imported.
- Edits are saved as you make them: with a vault, each step (a slider
  released, an undo or redo, a parameter set by an agent, a decision
  answered) is kept at once as a new version of the image, and a long drag
  is saved every second while it goes. Opening the image again, even after
  a crash, continues from the last saved step, decisions included.
- An example plugin reads an image's camera, lens, exposure, GPS position
  and keywords with ExifTool, each value with the tag it came from.
- A network can write files: an export renders a node, converts it to sRGB
  and saves it as JPEG, PNG (8 or 16-bit) or TIFF (16-bit), with the format
  taken from the file name. A file is never seen half written, two exports to
  the same file wait for each other, and a node reports each export and says
  why one failed.
- darktable's exposure module, in manual mode (exposure and black level), is
  the first module ported, with darktable's own formula.
- External plugins (proof of concept): a plugin is any program speaking
  JSON-RPC over its standard input and output, described by a manifest that
  is checked before it runs. Helios starts it when first needed, restarts it
  after a crash without affecting anything else, and holds it to the
  capabilities the user grants (`plugin.list`, `plugin.grant`,
  `metadata.read`, `device.capture`).
- A network can start from files: camera raws (Bayer and X-Trans sensors,
  through rawler) and JPEG, PNG or TIFF pictures. A raw comes in as its sensor
  data, untouched, with its camera's description (maker, model, exposure,
  levels, white balance, color matrix) handed to the modules that read it; a
  picture comes in converted to the working color space. The preview a raw
  carries can be read without decoding the raw, for fast culling.
- Modules can run on the GPU (Vulkan, Metal, through wgpu): a module may
  carry a GPU version of its computation next to its CPU one, and the GPU
  result is checked against the CPU reference before it is trusted.
  Exposure is the first.
- The engine can compute on the GPU: given a GPU device, every step whose
  module has a GPU variant (exposure, blur, sigmoid so far) runs there, the
  rest stays on the CPU in the same network, and the result matches the CPU
  render. A GPU kernel that fails now reports an error instead of crashing.
- When a step cannot run on the GPU, or the GPU is lost, the desktop now says so in a toast, in the interface's language, instead of only recording it; a module that falls back is named once.
- On the GPU, images now stay in GPU memory from one step to the next: a
  chain of GPU modules uploads its source once and reads its result back
  once, and a render's result can be handed over in GPU memory, so a viewport
  on the same device shows it without a copy back to the CPU.
- A headless node renders a network described in JSON, and the `helios`
  command observes it: its state, its events, and waiting for an event, so
  scripts and agents never have to sleep or parse logs. A wait that times
  out says what happened instead.
- Raw files now come out in true colors: an input color module, ported from
  darktable, turns the camera's own colors into the working color space with
  the camera's color matrix. Exposure can take back the exposure bias the
  photo was shot with, as darktable's "compensate camera exposure" does.
- Image processing modules can be written and tested on their own: a module
  declares its parameters, ports and needs, and the framework checks every
  output against that declaration. Contract tests generated from the
  declaration catch non-finite values, renders that differ from one run to
  the next, and tiles that differ from a whole render.
- Jobs declare a priority and what they need (GPU memory, AI calls, a
  destination…): what someone is waiting for runs first, a limited resource
  holds jobs back instead of overloading it, and a job whose needs can never
  be met fails at once, saying why.
- A new "Notify when a long job ends" setting, on by default: when exports
  and batches run from the window, one that takes more than ten seconds
  and ends while you work in another application will say so with a
  desktop notification, finished or failed, in your language.
- Settings are declared once and resolved through layers (defaults, global,
  project, user, run), and every value says which layer it comes from. A
  value set where it does not belong, such as a machine setting in a project
  file, is refused with the reason.
- `helios import` brings a folder, a file or a mounted card into a vault (referenced, or copied with `--copy --into DIR` and renamed with `--rename`), with `--duplicates skip|keep|ask`; `helios stacks` lists, makes and dissolves stacks and asks for, accepts or declines stack suggestions. See docs/library.md.
- With a vault open, the desktop runs the library operations as commands: `library.import` (in the background, as a job, the library filled again when it ends) and `library.stacks.suggest`, `accept`, `decline`, `list` and `dissolve`.
- Hovering an image in the library or the filmstrip now shows its file's size and the day it was last modified, written the interface's way (`25,3 Mio · modifié le 12 oct. 2026` in French).
- Importing is now a library operation: a folder or a mounted card is referenced in place or copied into the library folder by capture date, renamed by a pattern, with its XMP sidecars, as a job with progress. Exact duplicates are found by content, within the import and against the library, and skipped, kept or listed for an answer; nothing is ever deleted. Each image gets near-duplicate signals (a perceptual hash, capture time, exposure, frame number, sharpness).
- Library: `helios-gui --vault FILE` shows the vault's images as a grid (the
  Vault panel) and a filmstrip, with their thumbnails (a raw file's embedded
  preview). Rate them 0 to 5 or reject them (keys 0–5 and R, as in
  darktable's lighttable), toggle color labels, and move with the arrows; the
  changes are written to the vault, audited like any other.
- Helios now keeps a log file and writes a crash report when it fails, both
  in your state directory (`~/.local/state/helios` on Linux). Nothing is
  sent anywhere: after a crash, the next start offers the report, and
  Help > About Helios says where both are.
- A brush stroke mask, `mask_brush`: darktable's brush, a curve painted with
  a round brush whose radius, hardness and density change from point to
  point. The drawn shape masks (circle, ellipse, gradient, path, brush) can
  now be used in a network description too.
- Drawn masks are edited on the image: the mask's shapes are outlined, the
  selected one has handles to move, size, rotate and feather it (Shift
  snaps the angle, Ctrl moves finely, Escape cancels, each drag one undo
  step), paths gain and lose points, and new circles, ellipses, gradients
  and paths are added with `C`, `E`, `G` and `P` or the `mask.*` commands.
- Mask modules build darktable's flexi mask groups as a network: `mask_combine` folds up to eight masks with one of seven operators (maximum, screen, minimum, product, sum, difference, exclusion), `mask_feather`, `mask_blur` and `mask_levels` refine a member or a group, and `mask_blend` applies a module through the finished mask.
- A parametric mask, `mask_parametric`, selects pixels by ranges of luminance, red, green, blue and perceptual lightness, chroma and hue, each with a feathered edge and its own inversion, as darktable's parametric masks do. Connect it to a module's input or to its output to mask on either.
- The drawn-mask parity job also builds darktable's flexi masks pull
  request at a pinned commit: it renders two flexi fixtures and every
  classic one again (which darktable migrates to flexi masks as it reads
  them), and keeps darktable's own decoding of the fixtures' masks, which
  a test compares with what Helios reads.
- Drawn masks imported from darktable are checked against darktable itself:
  a bench box job renders hand-made sidecars (one per shape kind, two
  groups) on raws of the corpus with darktable-cli, built from a pinned
  upstream commit, and with Helios, and compares the masks in Delta E.
- A raster mask, `mask_raster`, reuses the mask of one module in another, optionally inverted and scaled, as darktable's raster masks do. The reused mask is not computed again, and stays the same in tiles and previews.
- A model can answer an AI object mask: your own program reading JSON and
  writing a PNG, or a local server speaking the same JSON, picked by name
  per mask member. Nothing is configured by default, and a model that finds
  nothing or fails leaves the mask as it was and says why.
- Each row of the Masks panel shows its member's icon and ends with an eye
  and a lock: the eye turns the member's opacity off or back on, the lock
  keeps it from moving or leaving the mask and its shape from being
  edited on the image (a locked group keeps its members). A row's context
  menu acts on the row right-clicked, and a rename starts from the plain
  name.
- Masks can be drawn: circles, ellipses, linear gradients and paths (polygons
  or Bézier curves), each with a feather, an opacity and an inversion,
  combined with the other mask members by every group operator. Shapes are
  placed in image pixels, so they stay put at every zoom and crop, their
  edges are anti-aliased, and parts of the image away from a shape cost
  almost nothing to render.
- The Masks panel shows an AI object member with its own icon, and the
  Parameters panel what it looks for (the kind of object or a prompt, and
  which model to ask); changing it asks again.
- A Masks panel shows the selected module's mask as a tree of groups and members, with their icons, operators, inversion and opacity, kept up to date with the image and the Parameters panel. Selecting a member there selects it on the image and back; members are renamed with F2, inverted, faded, reordered or moved between groups by drag and drop, deleted, and new shapes or parametric members added.
- `helios-mcp` lets an AI agent drive a running node with the same verbs as
  the `helios` command: read its state and events, wait for it, render,
  change parameters, inject faults, and read every module's parameters and
  ports from its manifest.
- New workspace from template: pick a template by its scenario and a
  directory, and the workspace opens with the template's network, panels
  and keys; an image opened there goes through the template's network.
- Every node reports the same normalized status (idle, stale, running, done
  or failed, with its load), ready for the node headers of every surface; the
  graph also proposes principal nodes and stacks, so a large network can be
  shown as an overview.
- Normalized dialogs: confirmations, forms, pickers, menus and notices are
  drawn by the shell alone, the same way everywhere from the theme's tokens
  (a test fails if a panel draws its own). A destructive confirmation is
  named by its verb, in the danger style, with the focus on Cancel; warnings
  and errors show as toasts. `dialog.current` and `dialog.answer` let an
  agent read and answer any dialog, `dialog.assumeYes` confirms by policy
  for scripts. Detached panel windows get Helios's frame.
- A module may declare optional inputs: left unconnected, the module gets an empty image in their place and the render goes on, so one module can take a variable number of members up to a fixed maximum.
- Parameter widgets: the Parameters panel shows each node's values as
  widgets generated from their descriptors: sliders over the soft range
  (drag, wheel, arrows, double click to reset, Enter to type a value with
  its unit), toggles, lists and fields. A drag is one undo step and keeps
  agents off the value meanwhile; a value changed from its default, moved
  by someone else (and who) or held by another's gesture is marked; the
  context menu resets or copies the path or the value.
- Modules can now speak photography: raw mosaics with their Bayer or X-Trans
  pattern, what the camera recorded (levels, white balance, color matrix,
  exposure) offered as context a module declares, and the color spaces of
  the pipe, which works in linear Rec.2020 like darktable.
- A plugin's manifest now gives a title and a description, and the plugin can translate them, with its commands' titles, in its own catalogues; a catalogue that tries to change another plugin's or Helios's text is refused. The ExifTool example plugin ships English and French.
- A first workspace template, "Portrait retouch": an exposure brightened
  through a feathered mask, the Masks panel and the parameters docked, the
  mask tools at hand, in English and French.
- A profile can differ per system or per machine: a bigger font on the
  laptop, another shortcut on macOS. Its parts may hold variants for a
  system (`macos`, `linux`, `windows`), a high-density screen (`hidpi`) or
  a machine named once in `machine.toml` in the config directory; Helios
  applies those that match where it runs, the most specific last, and
  `profile.resolved` says which values came from which variant. Variants
  for other machines stay in the file, travel with an export and show in an
  import's diff; `profile.export` with `this_machine` writes only this
  machine's view.
- Profiles. Your theme, your own shortcuts and your panel layout can be
  saved as a profile (`profile.export`), read back on another machine
  (`profile.import` shows what would change, `profile.import.apply`
  applies it), and chosen by name (`profile.use`); each workspace remembers
  its profile. Profile files are readable text and never carry secrets: an
  export that would is refused. A panel you closed now stays closed when a
  layout is restored.
- A camera raw can now be developed from start to finish: two new modules,
  ported from darktable, prepare the sensor data (black level and scale) and
  demosaic it (bilinear, Bayer and X-Trans), reading the camera's levels and
  filter pattern on their own. A raw goes through to an exported picture.
- A loaded raw says where its image sits on the sensor: the full photosite
  grid and the corner of the crop kept. darktable's drawn masks are placed
  on that grid, so a mask read from a sidecar lands where it was drawn.
- Previews: any part of a network can be rendered at a reduced scale, half
  or a quarter of full resolution, and look like the full render reduced.
  Sizes in a module (a blur radius) are given in full-resolution pixels and
  follow the scale on their own; previews and exact renders are cached
  apart. A Gaussian blur is the first module to use it.
- `helios sandbox new|start|stop|clean` gives a node its own directory,
  settings and sockets in one command each, and an agent can do the same from
  MCP. A sandbox is never created over a directory holding something else, and
  only a directory marked as a sandbox is ever removed.
- Background work runs as named, cancellable jobs, announced when they are
  queued, start and end; `helios inspect wait-idle` returns as soon as nothing
  is left to do, and the number of jobs run at once is a machine setting.
- Scopes: the Scopes panel shows a histogram, a waveform or an RGB parade of
  the image, in stops (where exposure and clipping read best) or linear,
  switched from the panel or the palette ("Scopes: …").
- The desktop's foundations, ahead of its window: every action is a named
  command that a menu, a key, the palette, an agent or a test runs the same
  way, each run remembering who asked; keys are resolved by the framework in
  layers (yours over the workspace's over the defaults over what modules
  suggest), can be removed for good, and conflicts are listed; panels are
  declared and a workspace's arrangement is saved and repaired when panels
  or screens change.
- `helios batch --sidecar --vault FILE` keeps each darktable sidecar it
  applies: the image's darktable edit becomes a version marked as imported,
  and the sidecar's rating (or reject) is kept, but only when the image has
  none yet, so it never overwrites a rating chosen in Helios.
- A raw now develops into a finished picture: the sigmoid tone mapper,
  ported from darktable (its recommended one), rolls the scene's highlights
  off into the display's range instead of clipping them, keeps middle grey
  where it is, and keeps hues. Contrast, skew and the display's white and
  black targets can be set.
- Helios now runs once per user: opening an image or a workspace from the
  file manager, or a `helios://open?path=...` link, while Helios runs hands
  it to the open window, which comes to the front and shows it.
  `helios-gui --new-instance` still opens a window of its own.
- A `stacks.suggest` decision proposes stacks from the import signals: frames close in time (or of one burst) that look alike, brackets included, the sharpest proposed on top. Nothing is applied until a person accepts all the groups or some of them, and the answer is recorded with the proposal; the resolver is picked by name, rule-based by default.
- System menu: the Helios logo at the top left of the window (or Alt+Space)
  opens a menu to open a workspace (in a new window) or a recent one,
  switch profile or theme (the one in use ticked), list the keyboard
  shortcuts, act on the window, read about Helios or quit. Menus are now
  declared: panels and plugins add commands to a menu by its id, with keys
  shown from the keymap.
- Tabs move between areas: drag a panel's tab onto another side bar, the
  editors or the bottom panel (or the edge where a hidden one would be) and
  it moves there; the area is highlighted while dragging, or says why the
  panel may not go there (the image and the graph stay in the editors).
  `<panel>.moveTo` does the same from keys, the palette or an agent, under
  the same rules; a saved layout breaking them is repaired.
- A node can also be reached over loopback TCP (`tcp://127.0.0.1:PORT`), for
  systems without Unix sockets: only local connections are served, and each
  must present a session token the node writes to a file only its user can
  read.
- The GUI now takes its theme from the egui-pop library: Helios's dark, light and high-contrast themes are small overlays of egui-pop's, keeping only the viewport surround, node state colours and the danger button. Headless runs settle faster, with no cursor blink or animation.
- Themes. Helios ships Helios Dark (the default), Helios Light and Helios
  High Contrast, chosen from the palette ("Theme: …"). A theme is a Design
  Tokens file (`*.tokens.json`) of named colours and sizes; it can build on
  another and change only a few of its values, and `theme.export` writes
  the theme in use as one file to share. A theme with a mistake still
  works: the faulty values fall back to the dark theme's, and Helios says
  which.
- The Helios logo and name are back at the start of the window's title bar,
  next to the button that opens the system menu.
- Translation catalogs: Helios's commands, panels and module parameters get
  Fluent catalogs in `i18n/`, generated in English from their declarations
  and falling back from a regional locale to its language, then to English;
  `cargo xtask i18n check` reports each language's coverage and stale
  translations. A first French catalog and an accented pseudo-locale
  (`x-pseudo`) are included; the desktop shows them in a next step.
- Value store: every edited value lives at a stable path (`node/…`,
  `settings/…`, `view/…`) with a descriptor from the module's manifest and
  hints (ranges, steps, unit). `param.set` and its siblings set a value or
  a step, say whether a range held it back, refuse a stale write or one
  during someone else's drag, undo by gesture, and announce each change
  with who made it.
- Ratings, color labels, titles and descriptions are changed by commands that
  skip values already stored, keep only the last change of a batch, and record
  who changed what and when; collections are read through cursors, so opening
  one or showing a screen of thumbnails costs one query each, and refreshing
  says exactly which images came in and went out.
- The vault identifies an image by its content, not by where it is: a folder
  import records every copy of a file as a location, a file already known is
  never read again, and a repair scan finds moved or renamed files by content
  while reading as little as it can.
- A vault that was not closed properly (a crash, a kill, a power cut) is
  checked before it is used. If the check fails, the vault opens read-only
  and the error names the latest verified snapshot to restore from; the
  vault keeps its last five snapshots.
- A workflow's journal (which images went through which step, what failed
  and is due again) can be kept in the vault, so it survives a restart and
  lives in the same database as the catalog.
- The vault now records its schema version and upgrades older vaults step by
  step, after taking a verified copy of the database next to it. A vault
  written by a newer Helios is refused with a clear message instead of being
  silently changed, and an upgrade gets past leftovers from old versions
  (stale views, rows pointing to deleted images), reporting what it found.
- Your own settings and each project's can live in the vault
  (`helios-node --vault FILE`): they follow you to any machine that opens
  it. A settings file given the first time is imported once; after that
  the vault's copy is the one used.
- Image viewport: the Image panel shows a node's output on the GPU, a proxy
  first, then the exact visible part in tiles reused when panning, never
  blocking the window. Zoom and position are values moved by mouse,
  `viewport.zoom.*` or an agent, a changed parameter renders again, and
  `gui.snapshot` of `viewport.scene` saves the data before the display
  transform. `helios-gui --image FILE` shows a picture.
- vkdt's local contrast (`vkdt_contrast`): a guided filter on the
  luminance, its blurs computed at lower resolutions, then the detail
  boosted against it. It is the first module made of several kernels, each
  computed, cached and tiled on its own, those reading one image on the
  GPU; the nightly check compares it with vkdt's own render.
- vkdt modules can be described in Helios: their parameter, connector and
  interface files read into Helios manifests and write back unchanged, for
  every module of vkdt. Manifests gained what that took: array and text
  parameters, pixel formats beyond 32-bit float, named channel layouts (a raw
  mosaic, RGBA), ports shaped like another port, and storage buffers.
- The first modules ported from vkdt: its exposure and its unsharp mask
  (`vkdt_exposure`, `vkdt_usm`), each with a GPU variant translated from
  vkdt's shader. A nightly check builds vkdt and compares Helios's results
  with vkdt's own.
- A study of vkdt's module format, with an inventory of its 99 modules, maps
  what vkdt modules declare onto the Helios module contract and lists what
  the contract still lacks to host them.
- Raw files come out with their colors: a white balance module, ported from
  darktable, applies the camera's as-shot balance (or the user's), and a
  highlights module keeps blown areas neutral instead of letting them turn
  magenta.
- `helios doctor display` and the debug bridge say where the image goes with the Wide-gamut preview setting on: `tagged`, `wsi` or `srgb`, with the encoding and the reason.
- A Wide-gamut preview setting (Display, off by default): the image asks to
  be drawn in Rec.2020 with gamma 2.2, so colours outside sRGB are not
  clipped where the window can carry it; elsewhere it stays sRGB, and the
  interface always does. In English and French.
- The Wide-gamut preview setting now shows the image in a wide gamut on Wayland compositors with colour management: tagged BT.2020 with a 2.2 power on KWin 6.3 and later, sway and Weston, Display P3 on GNOME and Hyprland. Colours outside sRGB are no longer clipped there; the interface is unchanged, and elsewhere the image stays sRGB.
- Helios draws its own window frame: a title bar with the workspace's name,
  minimize, maximize and close buttons in the interface's style, a double
  click to maximize, and resizing from any edge. F11 toggles full screen,
  which also hides the title bar.
- A workflow can be applied to many items over time: each item's progress
  through each step is journaled, workers claim items without ever taking one
  twice, a choice skips the branches it did not take, failures are retried
  after a growing delay, and work held by a worker that died is given back.
- darktable's drawn masks come over with a sidecar: a module blending
  through circles, ellipses, gradients, paths, brushes and their groups
  (with each member's inversion and opacity, the mask's inversion, blur,
  contrast and brightness, and the module's opacity) gets the same mask as
  Helios nodes. A mask Helios cannot reproduce exactly (a clone or AI
  shape, a curved gradient, a parametric or raster mask, another blend
  mode...) is never approximated: the module is kept aside, and the
  import's warning says why.
- Sidecars written by darktable's flexi masks (the pull request
  darktable-org/darktable#22553, masks version 7) are read too: groups with
  their operator (maximum, screen, minimum, product, sum, difference,
  exclusion), inversion, bypass and opacity, hidden and disabled members,
  and their blur, contrast and brightness refinement, as a tree of Helios
  mask groups. A later masks version is kept aside with a warning.
- darktable edits can be read: Helios opens a darktable XMP sidecar, decodes
  each module's settings by name with darktable's own description of them,
  and carries the modules Helios has over to its own, with their values. A
  report lists everything else (modules Helios does not have yet, settings
  without a counterpart, older module versions still to migrate), so
  nothing is lost without a word.

### Changed

- Decision nodes can be answered through the AI routing map: a decision
  naming a capability (`segmentation`), or naming no resolver, goes to the
  provider and model routed for it. The OpenAI-compatible and Anthropic
  endpoints of the `http` resolver now share the AI providers' code.
- The render cache now uses at most a quarter of the machine's memory by default
  instead of a fixed 2 GiB; the share, or a fixed size, is a machine setting.
  A render never evicts what it still needs, so a small budget no longer makes
  it compute the same step twice.
- The desktop's own notices (a profile applied, a theme exported, an edit not saved), the Open a workspace dialog, and its refusals (undo while a drag is in progress, a mask member dropped where it cannot go) now follow the interface's language; French is included.
- The viewport's display transform encodes for the target the window draws
  for, instead of always for sRGB: on a target wider than sRGB (Rec.2020,
  extended sRGB) colours outside the sRGB gamut are no longer clipped.
- Helios now follows the desktop's focus mode (focus on click or on hover) from the start, and a vault whose images cannot be listed or imported when Helios opens is reported in the window instead of closing it.
- The window's title bar and edges are now egui-pop's: a menu button at the top left opens the Helios menu (a right click on the title bar and Alt+Space too), the edges are easier to grab on high-density screens, and a "Window frame" setting gives the desktop's own frame back.
- Panels are now placed by egui-pop's workbench: a tab's menu moves, detaches or closes its panel, named arrangements can be saved, switched and reset, and Helios offers an "Image" arrangement (the image and the filmstrip alone) for small screens. Layouts saved by earlier versions open unchanged; the areas are now called the centre, the start and end side bars and the bottom panel.
- Export and batch errors (a file that cannot be written, a format not known, a network with several sources, an image cancelled) now carry a translation key with their English, so an interface or an agent can show them in its user's language; French is included.
- The desktop is now built on egui-pop's application framework: a menu bar, a command palette that ranks commands by a fuzzy match with the recent ones first, a settings window (Ctrl+,) with a shortcuts editor where a key is changed by pressing it, and the window's size and place kept between runs. Dialogs, toasts and menus are egui-pop's; for now a path field browses with the in-app picker only, and a picker is a list of buttons without a search.
- The warning a darktable import gives when it cannot apply some history items now comes with a translation key, each reason translated too, beside its English line; French is included.
- A form's Browse… button opens the new in-app file picker: a click on a file highlights it and Open (or a double click) chooses it, a filter field narrows the list or takes a typed path, and Cancel goes back to the form.
- A brush stroke in the Masks panel now has its own icon (a pencil),
  instead of the generic one.
- A locked path in the Masks panel also keeps its points: none can be
  added, removed or smoothed until it is unlocked.
- A row's menu in the Masks panel now says each command in a word or two
  (Rename…, Invert, Lock or unlock…), in the interface's language, instead
  of the command palette's long titles.
- When the node refuses a request (a file it cannot load, a node it does not have, a job already ended), its answer and the batch's line for that image now carry a translation key beside the English, so they can be shown in the user's language; French is included.
- The Parameters panel shows each parameter's label, description and choices in the interface's language (`exposition`, `noir` in French), from the modules' translations.
- The Parameters panel writes values in the interface's language (`0,50` and `12,5 %` in French), shows exposures signed (`+0.70 EV`), and its tooltips and notices (a value held to its range, a typed value that is not a number) are translated.
- Pickers (move a panel, choose an area...) have a filter field again: typing narrows the list, the arrows move through it and Enter takes the highlighted entry.
- A node read by several others over different areas is now computed once over
  the area they all need, each reading its part of it: a network of 58 mask
  nodes renders about nine times faster with a sixth of the memory, and an area
  inside one already rendered is shown without computing anything.
- The image viewport runs on egui-pop's viewport. 100 % now means one image
  pixel on one screen pixel at any window scale, and the zoom value shows
  100 % there. Zoom in and zoom out walk a ladder of whole ratios (50 %,
  100 %, 200 %...), and the wheel stops at 100 % when it crosses it. Edits
  re-render in tiles, the old image staying on screen until the new tiles
  arrive. New: `viewport.compare` shows before and after side by side
  (before is the edit as it was opened; hold `\` to see it alone), and the
  working-space values of the pixel under the pointer show in a corner.
- A darktable edit that uses modules Helios does not have yet (or a version,
  a blend or a mask it cannot apply) still imports: those steps are kept
  untouched, listed as not applied with their name and version, and one
  warning per sidecar sums them up, instead of being left out. A masked or
  blended step is no longer applied to the whole image, and a damaged history
  entry no longer stops the import. The import report now lists these steps
  under `not_applied`, which replaces `unknown_modules`,
  `unsupported_versions` and `undecodable`.

### Fixed

- Raw files from a camera the decoder does not list (a DNG from a Ricoh GR, for
  one) no longer carry the padding of their metadata in the camera name, so
  the same camera reads and compares under one name.
- Connecting nodes in a network where many nodes read the same inputs no longer slows down exponentially: the check that refuses cycles now looks at each node once.
- The outlines of drawn mask shapes stay crisp on screens scaled to 125 % or 150 %, in the main window and in a detached image window alike. Picturing a part of a detached panel now says it is not supported yet instead of saving a wrong crop of the main window.
- A parameter slider no longer lets its panel scroll on after a wheel step,
  the window's own title bar is hidden in full screen, and the mask outlines
  on the viewport stay sharp at 125 % and 150 % scaling.
- A profile's key bindings now keep where they apply (their `when` clause) and the default keys they take away, and applying a profile says how many bindings changed and which keys conflict.
- A profile's high-density screen variant now follows the screen itself, also after the window moves to another screen, and no longer the interface zoom (Ctrl+Alt+= and Ctrl+Alt+-); focus rings stay sharp at fractional scales.
- GPUs are now opened with everything they allow instead of wgpu's small default limits, so images over 128 MB, such as a 24 MP photo in full precision, no longer fail on a GPU that was chosen to run them. When a driver refuses its own limits, the device opens with less, says why, and work that no longer fits goes to the CPU.
- GPU steps over about 4.2 megapixels, such as a full 24 MP photo, no longer fail: kernels now run over a two-dimensional grid of workgroups instead of a single row capped by the GPU.
- Helios now opens one GPU device per process and shares it between the window and the engine, instead of letting the window open a second one. On Linux with a Vulkan loader older than 1.4.350, two devices could crash the application when one was being opened while the other was in use.
- A failing GPU no longer fails a render. A step whose GPU code fails or runs out of GPU memory is computed on the CPU instead, with the same result, and the event log says which step and why. GPU memory now has a budget per device: when it is spent, older results kept on the GPU are dropped first. A GPU that stops answering or is lost (a driver reset) is restarted once and the render replayed; if that fails, the rest of the session renders on the CPU, and a message says so. No wait for the GPU can hang the application any more.
- Toggling a color label in the library no longer erases a label another
  program put on the image meanwhile, and taking a reject back restores the
  stars the image had. When a rating or label cannot be saved, the library
  shows the saved value again and says why, instead of showing a change that
  was lost.
- The day the library shows for a file's last change is the one of the system's time zone, no longer UTC's: a file saved late in the evening no longer reads as the next day.
- The names and locks given to mask members in the Masks panel are now
  saved with the image's edit, so they are still there after closing and
  reopening it.
- In the Parameters panel, a path's points no longer spill across the
  panel and push its feather and opacity out of view: they show as a
  summary ("12 points") that opens to list them, a point a line.
- The wheel over the Parameters panel scrolls it instead of changing the slider under the pointer; a slider takes the wheel once clicked, or while Alt is held. A held arrow (or a turned wheel) on a slider is now one undo step, and a value being typed lets go of the keyboard when you click elsewhere, so the image's keys work again.
- Rendering a network where several modules read the same inputs, as chained masked modules do, no longer slows down exponentially with the chain's length: each shared step is planned once per render. Six chained masked modules render about three times faster and with a seventh of the memory.
- Raw files whose sensor has masked photosites (many Canon cameras) now take
  their black level from the median of those photosites, as darktable does,
  instead of their mean. Shadows were slightly lifted and tinted before; the
  raw chain now matches darktable's within a Delta E 2000 of 0.14 on average.
- Raw photos previewed at reduced size keep their colours: the sensor data is
  no longer averaged before demosaicing, which mixed red, green and blue
  photosites into grey. Demosaicing now runs at full resolution and the result
  is reduced after it, as darktable does.
- A node started in a sandbox no longer stops when Ctrl-C is pressed in the terminal that started it: it runs until `helios sandbox stop`.
- Selecting a node is no longer an undo step: Ctrl+Z takes back the last
  edit, not the last selection.
- The system menu no longer repeats the window's own buttons: its Window
  submenu is gone.
- A long notice no longer runs under its close button: its text wraps before it.
- Edit > Undo (`Ctrl+Z`) and Redo undo parameter changes again, each one saved in the image's history, and the menu names what they take back ("Undo drag exposure"); they wait while a drag is in progress. The interface settings keep their own undo, in the settings window.
- A batch of rating or label changes is now saved whole or not at all: a
  crash or a failed write no longer leaves a change without its audit entry
  or half a batch applied, and several Helios processes can share one vault
  without failing as busy or recording a wrong previous value.
- Panning or zooming the image is no longer an undo step: undo goes back
  to the last edit, and moving the view no longer saves a new version of
  the edit.
- Raw edits saved by older darktable releases that never touched highlight
  reconstruction now import with the clipping darktable itself gives them
  when it opens them, instead of without it.
- Edits imported from darktable sidecars now run their modules in darktable's pipe order, custom orders and multiple instances included, instead of the order they were last edited in, so they render as in darktable. Modules the sidecar's order does not place are named in the import report.
