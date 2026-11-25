# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.

command-activity-resume =
    .title = Resume a held task now (`id`: a promise of `promises.list` held on a service's quota or a refused sign-in)

command-activity-show =
    .title = Activity… (`id`: the job selected, a promise of `promises.list`)

command-calibrate-open =
    .title = Calibrate camera colour… (`path`: a chart shot; `corners`: its four corner patches' centres; `camera`: the profiles listed)

command-compare-active =
    .title = Compare: the photos compared and the active one (`image`: make that compared photo active; `surface`: another Compare, helios.compare#<n>)

command-compare-add =
    .title = Compare: add photos to the comparison open (`images`: ids, else the selection; `at`: before the photo at that place, else last; sixteen at most; `surface`: another Compare, helios.compare#<n>)

command-compare-clear =
    .title = Compare: empty the comparison and leave Compare (`surface`: another Compare, helios.compare#<n>, emptied)

command-compare-leave =
    .title = Leave Compare: back to the grid, or wherever it was entered from (Escape)

command-compare-next =
    .title = Compare: make the next photo active (`surface`: a Compare, helios.compare#<n>; else the focused one, else the one focused last in the window)

command-compare-previous =
    .title = Compare: make the previous photo active (`surface`: a Compare, helios.compare#<n>; else the focused one, else the one focused last in the window)

command-compare-remove =
    .title = Compare: take photos out of the comparison (`images`: ids, else the active photo; the last one out leaves Compare; `surface`: another Compare, helios.compare#<n>)

command-compare-set =
    .title = Compare: compare exactly these photos (`images`: one to sixteen ids, in that order; `surface`: another Compare, helios.compare#<n>), entering Compare

command-compare-sync =
    .title = Compare: zoom and pan synchronised across the photos (`on`; toggled when absent; `surface`: a Compare, helios.compare#<n>; else the focused one, else the one focused last in the window)

command-controls-catalogue =
    .title = List the registered controls

command-controls-context =
    .title = Read the controls' context keys, box, density and hand; set any of them (`task`, `detail`, `image`, `tool`, `device`, `target`, `density`, `box`, `layouts`, `gesture_open`, `pointer_in_panel`, `handled`)

command-controls-describe =
    .title = Describe a control (`id`): purpose, connector, gestures, detents, variants, slot policies, orientations, integration and fallback per surface, warnings, settings schema; with `node`, its placement on that node's card and why

command-controls-find =
    .title = Search a node's controls (`node`, `query`), shown or folded, with their placement and reason

command-controls-nudge =
    .title = Step a control on a node (`node`, `control`, `steps`; `fine`, `coarse`)

command-controls-reset =
    .title = Reset a control on a node to its default (`node`, `control`)

command-controls-reveal =
    .title = Show a folded part of a node's card for the moment (`node`, `key`)

command-controls-reveal-all =
    .title = Show every part of a node's card for the moment (`node`)

command-controls-set =
    .title = Set a control on a node (`node`, `control`, `value`; `force`)

command-decision-accept =
    .title = Accept a decision's proposal (`node`; `option`, 0 for the model's best)

command-decision-acceptAbove =
    .title = Accept every proposal the model is at least this sure of (`confidence`, 0 to 1)

command-decision-decide =
    .title = Answer a decision, or correct its proposal (`node`; `values`: the answer's parameters, as plain values)

command-decision-list =
    .title = List the open image's decisions waiting for an answer: what each asks, the model's proposal, how sure it is and why

command-dialog-answer =
    .title = Answer the dialog (`answer`: primary or cancel; `values` for a form; `choice` for a picker)

command-dialog-assumeYes =
    .title = Answer confirmations yes by policy (`on`)

command-dialog-current =
    .title = Describe the dialog waiting for an answer

command-edit-resetAll =
    .title = Reset all: every module's parameters to their defaults and the modules bypassed brought back, the photo as it was before it was edited; one undo step

command-gui-record-start =
    .title = Start recording a video

command-gui-record-stop =
    .title = Stop recording

command-gui-snapshot =
    .title = Take a picture of a panel or a widget

command-health-checkNow =
    .title = Run the health checks now (Help > Health check)

command-health-fix =
    .title = Run a health finding's fix, then the checks again (`id`)

command-health-ignore =
    .title = Ignore a health finding, or show it again (`id`, `on`: true by default)

command-helios-app-reimport =
    .title = Add the photos of the folders a reset kept back to the library, where they are (nothing copied or moved)

command-helios-app-reimport-folders =
    .title = The photo folders a reset kept, offered for a re-import in place

command-helios-compare-detach =
    .title = Move Compare to a new window

command-helios-compare-dock =
    .title = Dock Compare back in its area

command-helios-compare-float =
    .title = Float Compare over the interface (`pos`: [x, y])

command-helios-compare-moveTo =
    .title = Move Compare to an area (`area`)

command-helios-decisions-detach =
    .title = Move Decisions to a new window

command-helios-decisions-dock =
    .title = Dock Decisions back in its area

command-helios-decisions-float =
    .title = Float Decisions over the interface (`pos`: [x, y])

command-helios-decisions-moveTo =
    .title = Move Decisions to an area (`area`)

command-helios-duplicates-detach =
    .title = Move Duplicates to a new window

command-helios-duplicates-dock =
    .title = Dock Duplicates back in its area

command-helios-duplicates-float =
    .title = Float Duplicates over the interface (`pos`: [x, y])

command-helios-duplicates-moveTo =
    .title = Move Duplicates to an area (`area`)

command-helios-filmstrip-detach =
    .title = Move Filmstrip to a new window

command-helios-filmstrip-dock =
    .title = Dock Filmstrip back in its area

command-helios-filmstrip-float =
    .title = Float Filmstrip over the interface (`pos`: [x, y])

command-helios-filmstrip-moveTo =
    .title = Move Filmstrip to an area (`area`)

command-helios-filters-detach =
    .title = Move Filters to a new window

command-helios-filters-dock =
    .title = Dock Filters back in its area

command-helios-filters-float =
    .title = Float Filters over the interface (`pos`: [x, y])

command-helios-filters-moveTo =
    .title = Move Filters to an area (`area`)

command-helios-graph-detach =
    .title = Move Graph to a new window

command-helios-graph-dock =
    .title = Dock Graph back in its area

command-helios-graph-float =
    .title = Float Graph over the interface (`pos`: [x, y])

command-helios-graph-moveTo =
    .title = Move Graph to an area (`area`)

command-helios-log-detach =
    .title = Move Log to a new window

command-helios-log-dock =
    .title = Dock Log back in its area

command-helios-log-float =
    .title = Float Log over the interface (`pos`: [x, y])

command-helios-log-moveTo =
    .title = Move Log to an area (`area`)

command-helios-masks-detach =
    .title = Move Masks to a new window

command-helios-masks-dock =
    .title = Dock Masks back in its area

command-helios-masks-float =
    .title = Float Masks over the interface (`pos`: [x, y])

command-helios-masks-moveTo =
    .title = Move Masks to an area (`area`)

command-helios-modules-detach =
    .title = Move Modules to a new window

command-helios-modules-dock =
    .title = Dock Modules back in its area

command-helios-modules-float =
    .title = Float Modules over the interface (`pos`: [x, y])

command-helios-modules-moveTo =
    .title = Move Modules to an area (`area`)

command-helios-navigator-detach =
    .title = Move Navigator to a new window

command-helios-navigator-dock =
    .title = Dock Navigator back in its area

command-helios-navigator-float =
    .title = Float Navigator over the interface (`pos`: [x, y])

command-helios-navigator-moveTo =
    .title = Move Navigator to an area (`area`)

command-helios-params-detach =
    .title = Move Parameters to a new window

command-helios-params-dock =
    .title = Dock Parameters back in its area

command-helios-params-float =
    .title = Float Parameters over the interface (`pos`: [x, y])

command-helios-params-moveTo =
    .title = Move Parameters to an area (`area`)

command-helios-queue-detach =
    .title = Move Queue to a new window

command-helios-queue-dock =
    .title = Dock Queue back in its area

command-helios-queue-float =
    .title = Float Queue over the interface (`pos`: [x, y])

command-helios-queue-moveTo =
    .title = Move Queue to an area (`area`)

command-helios-quit-confirmed =
    .title = Quit Helios, cancelling the running jobs

command-helios-quit-unsaved =
    .title = Before quitting, an edit not saved: retry saving or quit without saving (`value`)

command-helios-scopes-detach =
    .title = Move Scopes to a new window

command-helios-scopes-dock =
    .title = Dock Scopes back in its area

command-helios-scopes-float =
    .title = Float Scopes over the interface (`pos`: [x, y])

command-helios-scopes-moveTo =
    .title = Move Scopes to an area (`area`)

command-helios-setUp =
    .title = Set up Helios…

command-helios-stacks-detach =
    .title = Move Stacks to a new window

command-helios-stacks-dock =
    .title = Dock Stacks back in its area

command-helios-stacks-float =
    .title = Float Stacks over the interface (`pos`: [x, y])

command-helios-stacks-moveTo =
    .title = Move Stacks to an area (`area`)

command-helios-vault-detach =
    .title = Move Library to a new window

command-helios-vault-dock =
    .title = Dock Library back in its area

command-helios-vault-float =
    .title = Float Library over the interface (`pos`: [x, y])

command-helios-vault-moveTo =
    .title = Move Library to an area (`area`)

command-helios-viewport-detach =
    .title = Move Image to a new window

command-helios-viewport-dock =
    .title = Dock Image back in its area

command-helios-viewport-float =
    .title = Float Image over the interface (`pos`: [x, y])

command-helios-viewport-moveTo =
    .title = Move Image to an area (`area`)

command-helios-window-main =
    .title = Main window

command-helios-workshops-detach =
    .title = Move Workshops to a new window

command-helios-workshops-dock =
    .title = Dock Workshops back in its area

command-helios-workshops-float =
    .title = Float Workshops over the interface (`pos`: [x, y])

command-helios-workshops-moveTo =
    .title = Move Workshops to an area (`area`)

command-help-health =
    .title = Health check…

command-import-addSource =
    .title = Add a source that needs connecting: opens the import window and runs the connector's connection steps (`connector`)

command-import-disconnect =
    .title = Disconnect an import account (`account`): its token revoked where the service allows, its secrets forgotten; the saved sources using it are kept with it, not connected (`sources`: "keep") or forgotten with it ("forget"); an account no source uses is forgotten

command-import-history-again =
    .title = Import again (`id`: an import of `import.history.list`): the same sources with the same choices; photos the library has are skipped as duplicates

command-import-history-list =
    .title = List the import history, newest first (`limit`): each import's sources, choices, counts, when and who asked

command-import-reconnect =
    .title = Connect an import account again, from the step that failed (`account`)

command-import-signOut =
    .title = Sign an import account out: its keys and tokens are forgotten, it stays listed (`account`); a choice of photos under way on it is cancelled first

command-import-sources-add =
    .title = Add an import source, as the import window's "Add…" wizard does: `connector` (its id) and what its setup asks (`path`: a folder, or a volume's mount point; `volume`: a mounted volume's id or label; `account`: an account of the connector), then `name` (else the folder's, volume's or account's), `profile` (an import profile's id; else the connector's default, else `everyday`), `auto` (true: the automatic import the connector offers) and `params` (the connector's own parameters); selected in the import window when it is open

command-init-apply =
    .title = Write the setup's settings (`library.folder`: the library folder)

command-init-chooseFolder =
    .title = Choose the library folder for the setup

command-init-libraryFolder =
    .title = Choose the library folder in the setup (`path`: set at once)

command-keymap-package =
    .title = Add a keymap package's keys over the defaults (`name`: darktable-classic)

command-keymap-package-remove =
    .title = Take a keymap package's keys away (`name`)

command-library-chooseFolder =
    .title = Choose the library folder imports copy photos into (`path`; asked through the folder chooser when absent)

command-library-collection =
    .title = Scope the library to an album (`id`; null: the whole library); kept with the workspace. Without `id`, says the album

command-library-collection-add =
    .title = Add photos to an album (`id`; `images`: ids, else the selection)

command-library-collection-create =
    .title = New album (`name`; `images`: ids, else the selection), or with `kind: smart` a smart album (`rules`: a tree of groups (`match`: all, any or none; `rules`) and conditions (`field`, `op`, `value`), or the form's keys), or with `kind: set` an album set (`set`: the set it goes in); without `name`, asks it. Answers its `id`

command-library-collection-delete =
    .title = Delete an album or a set (`id`); its photos stay in the library. A set's albums move up into the set it was in, or with `members: true` are deleted too

command-library-collection-list =
    .title = List the albums: `id`, `name`, `kind` (album, smart or set), the `set` it is in, how many photos each holds (a set: its albums' together), a smart album's `rules`; and the quick collection's `quick` count

command-library-collection-move =
    .title = Move an album, a smart album or a set (`id`) into a set (`set`; null: the top); a set never goes inside itself. Without `set`, asks it

command-library-collection-remove =
    .title = Remove photos from an album (`id`; `images`: ids, else the selection); they stay in the library

command-library-collection-rename =
    .title = Rename an album (`id`, `name`); without `name`, asks it

command-library-collection-rules =
    .title = Set a smart album's rules (`id`; `rules`, a tree, or the form's keys); without them, opens the rule editor on them. Unknown fields and invalid rules are refused with the reason

command-library-collection-rules-edit =
    .title = Edit the rules in the open rule editor, one undoable edit at a time: `edit` (add_condition, add_group, remove, move with `to`, match with `match`: all, any or none, field with `field`, op with `op`, value with `value`, `high` (between's second bound, or within's count) and `unit` (days, weeks or months), undo, redo, apply, cancel) at `at`, a path of indexes from the top group ([]). A capture date takes a day (YYYY-MM-DD) or, with `op: within`, today, this_year or last. Answers the draft, each condition's fault and the rules when valid

command-library-compare =
    .title = Compare photos side by side (`images`: ids, else the selection; two to four)

command-library-copyFiles =
    .title = Copy Files

command-library-deleteRejected =
    .title = Delete Rejected…: the rejected photos shown to the system trash (`images`: of them, those named), asked first unless `confirmed`

command-library-duplicate =
    .title = Duplicate: a new version of each photo from its current edit, beside the others (`images`: ids, else the selection); the grid counts the versions

command-library-duplicateOriginal =
    .title = Duplicate Original: a new version of each photo with no edit, beside the others (`images`: ids, else the selection)

command-library-export =
    .title = Export… (`images`: ids, else `scope`: `selected` or `shown`, the selection by default, else every image shown; `format`: jpeg, png or tiff; `quality`, `bits`, `long_edge`, `space`, `folder`, `pattern`, `clash`: number, overwrite or skip; `remember`; `again`: from the last export's settings, never asked; asked when `folder` is absent or `ask` is true)

command-library-files-pair =
    .title = Pair RAW+JPEG Files: each raw and JPEG of one shot among the photos (`images`: ids, else the selection; `all`: the whole Library) made one photo, the JPEG its camera file, the raw's marks kept where both have one, labels, tags and albums merged; `exact`: the two photos paired as they are; Ctrl+Z takes it back

command-library-files-unpair =
    .title = Unpair Files: each file of the photos (`images`: ids, else the selection) a photo of its own again, with a copy of the photo's marks and its own versions; Ctrl+Z pairs them again

command-library-filter-apply =
    .title = Apply a filter to the library (`query`, typed as in the search field, or `filter` as listed); it becomes the first recent filter

command-library-filter-pin =
    .title = Pin a filter (`query` or `filter`, else the filter in force), or unpin it with `pinned: false`

command-library-filter-recent =
    .title = List the library's `pinned` and `recent` filters (newest first) and its `smart` albums, each with its `query` and `filter`

command-library-filter-switch =
    .title = Show the recent and pinned filters and the smart albums, to apply one to the library

command-library-fitCameraLook =
    .title = Fit a camera look from the selection: one look from photos of one camera, fitted to their embedded JPEGs and written into each (`images`: ids, else the selection; `default`: keep it as the camera's default too)

command-library-folder =
    .title = Scope the library to a folder and those below it (`path`, absolute; null: the whole library); kept with the workspace. Without `path`, says the folder

command-library-importAutomatic-pause =
    .title = Pause automatic imports, or resume them (`on`: true pauses, false resumes; without it, toggles)

command-library-importFiles =
    .title = Import files and folders (`paths`), placed as the workspace's import source says (`copy`: into the library folder or not, `into`, `layout`, `rename`; `duplicates`; `ask`: asked without an import source; `darktable`; `camera_look`: false leaves the cameras' default looks off the raws)

command-library-importSession =
    .title = Show an import session in the Library (`id`: an import of `import.history.list`; null: the whole library), its photos all selected with `select: true`; it works with the other filters, nothing is copied or moved. Without `id`, says the session shown

command-library-importSource =
    .title = Import photos… (`path`: a card or folder, asked when absent; `copy`, `darktable`, `remember` when the workspace has no import source)

command-library-importWindow =
    .title = Import photos… (`source`: the card or folder the import window opens on)

command-library-importWith =
    .title = Import with an importer plugin (`plugin`, `importer`; `source`: the file or folder it reads, asked when absent; `copy`, `darktable`)

command-library-keyword =
    .title = Scope the library to a keyword branch (`path`, levels joined with `|`: the photos with that keyword or one below it; null: none), with the other filters. Without `path`, says the branch

command-library-keyword-add =
    .title = Tag photos with a keyword (`keyword`, levels joined with `|`; `images`: ids, else the selection), a mark as `library.tag` makes it

command-library-keyword-list =
    .title = List the keyword branches (`path`, levels joined with `|`) with how many photos have a keyword of each (`count`)

command-library-keyword-merge =
    .title = Merge a keyword branch into another photos have, on every photo (`from`, `into`); without `into`, asks it; Ctrl+Z takes it back

command-library-keyword-remove =
    .title = Take a keyword off photos, with those below it (`keyword`; `images`: ids, else the selection), a mark as `library.tag` makes it

command-library-keyword-rename =
    .title = Rename a keyword branch on every photo, the keywords below it along (`from`, `to`: a keyword no photo has yet); without `to`, asks it; Ctrl+Z takes it back

command-library-marks-copy =
    .title = Copy a photo's marks: rating, colour labels, tags and title (`image`: an id, else the photo under the cursor). Answers them

command-library-marks-paste =
    .title = Paste the copied marks (`images`: ids, else the selection; `marks`: some of rating, labels, tags, title, all by default; `mode`: merge, the default, adds labels and tags, overwrite makes each mark the copy's)

command-library-metadata-get =
    .title = Read the selection's metadata: each field's shared value, or `mixed`; the labels and keywords all or some photos have

command-library-metadata-set =
    .title = Set metadata on photos as a group (`fields`: title, description, creator, copyright, rating, flag; `keywords` and `labels`: `add`, `remove`; `images`: ids, else the selection); fields not named keep each photo's value; Ctrl+Z takes it back

command-library-metadata-template-apply =
    .title = Apply a metadata template (`name`) to photos (`images`: ids, else the selection); Ctrl+Z takes it back

command-library-metadata-template-delete =
    .title = Delete a metadata template (`name`)

command-library-metadata-template-list =
    .title = List the metadata templates, with their fields and keywords

command-library-metadata-template-save =
    .title = Save a metadata template (`name`), from `fields` and `keywords` when given, else from what the selected photos share

command-library-openFolder =
    .title = Open a folder in the file manager (`path`)

command-library-pasteFiles =
    .title = Paste Files or Image (`dry_run`: only say what would be imported)

command-library-quick-add =
    .title = Add photos to a quick collection (`id`; none: the target; `images`: ids, else the selection)

command-library-quick-clear =
    .title = Empty a quick collection (`id`; none: the target); its photos stay in the library

command-library-quick-create =
    .title = New quick collection (`name`; `images`: ids, none by default; `target: true` makes it the target); without `name`, asks it. Answers its `id`

command-library-quick-delete =
    .title = Delete a quick collection (`id`), not the first (`quick`); its photos stay in the library. When it was the target, the first one is

command-library-quick-list =
    .title = List the quick collections, the first one first: `id`, `name` (none for the first until renamed), `count`, `target` (true for the target of B)

command-library-quick-move =
    .title = Move photos to a quick collection (`id`; none: the target) out of another (`from`): `images`, ids, else the selection; one step. A drag between two quick collections does it, Ctrl copies

command-library-quick-open =
    .title = Open a quick collection (`id`; none: the target) in a panel of its own (`place`: last, here or an area): its photos, photos dropped on it go in. Answers the panel's id (`view`)

command-library-quick-remove =
    .title = Remove photos from a quick collection (`id`; none: the target; `images`: ids, else the selection); they stay in the library

command-library-quick-rename =
    .title = Rename a quick collection (`id`, `name`); without `name`, asks it

command-library-quick-saveAs =
    .title = Save a quick collection (`id`; none: the target) as an album (`name`; `clear`: empty it too); without `name`, asks it. Answers the album's `id`

command-library-quick-target =
    .title = Make a quick collection (`id`) the target: B and drops on it put photos there

command-library-quick-toggle =
    .title = B: put photos in the target quick collection (`id`: another one; `images`: ids, else the selection), or take them out when it holds them all; scope the library to one with `library.collection` and its `id` (the first is `quick`)

command-library-remove =
    .title = Remove from Library: take the photos out (`images`: ids, else the selection), their files untouched; Ctrl+Z puts them back

command-library-removeVersion =
    .title = Remove Version…: remove a copy of a photo with the edits only it holds (`image`: an id, else the photo under the cursor; `version`: a head's id, else its current one), asked first unless `confirmed`; a photo keeps one version; Ctrl+Z brings it back

command-library-resetRotation =
    .title = Reset Rotation: turn the photos back to their file's orientation (`images`: ids, else the selection)

command-library-rotateLeft =
    .title = Rotate Left: turn the photos a quarter counter-clockwise (`images`: ids, else the selection); Ctrl+Z turns them back

command-library-rotateRight =
    .title = Rotate Right: turn the photos a quarter clockwise (`images`: ids, else the selection); Ctrl+Z turns them back

command-library-runWorkshop =
    .title = Run Workshop on Selection…

command-library-scanFolder =
    .title = Scan a folder and those below it for new or changed files (`path`), as the image map scans a volume

command-library-select-invert =
    .title = Invert the selection: the photos shown that were not selected

command-library-select-none =
    .title = Select no photo, the cursor gone too

command-library-select-similar =
    .title = Select the photos shown like the selected ones (`by`: day, taken the same day, the default; or stack, in the same stack)

command-library-show =
    .title = Show exactly these photos in the Library (`ids`: image ids, e.g. a job's), the filter cleared and all of them selected; the next filter set shows by it again. Without `ids`, says how many the set shown holds

command-library-sort =
    .title = Sort the library (`by`: captured, imported, name, rating, edited, size or focus; `descending`); kept with the workspace. Without `by`, says how it is sorted

command-library-stack-group =
    .title = Group into Stack: the photos in one stack (`images`: ids, else the selection; `top`: the photo on top, else the one under the cursor), the stacks they are in merged; Ctrl+Z takes it back

command-library-stack-top =
    .title = Move to Top of Stack: the photo shown for its stack (`image`: an id, else the photo under the cursor); Ctrl+Z puts the former top back

command-library-stack-ungroup =
    .title = Unstack: ungroup the stacks the photos are in (`images`: ids, else the selection), the photos untouched; Ctrl+Z groups them again

command-library-storage =
    .title = Storage… (`volume`: the volume to mark and scroll to)

command-library-trash =
    .title = Move to Trash…: the photos' files to the system trash (`images`: ids, else the selection), asked first unless `confirmed`; refused on cards and backup volumes, the photos taken out of the Library; the system trash undoes it, not Ctrl+Z

command-library-view-close =
    .title = Close a Library view (`view`: its id); the other views keep their filter and cursor

command-library-view-open =
    .title = Open another Library view (`place`: last, here or an area): its own filter and cursor, kept with the layout. Answers its id (`view`)

command-library-view-selection =
    .title = Say a Library view's selection and cursor (`view`: its id or main; none: every view), as kept with the layout

command-library-window =
    .title = Library

command-macro-dissolve =
    .title = Give a macro way to its nodes, as ordinary ones (`node`; `dry_run`)

command-macro-fold =
    .title = Fold an opened macro back into one node (`node`; `changes`: `version`, `expose`, `discard` or `keep`, asked when it changed while open; `dry_run`)

command-macro-interface =
    .title = Choose the parameters a macro shows, saved as a new version (`node`, the selected one by default; `exposed`; `dry_run`)

command-macro-make =
    .title = Make a macro of nodes (`nodes`, the selected ones by default; `name`, `expose`, `dry_run`)

command-macro-open =
    .title = Open a macro in place, its nodes shown (`node`, the selected one by default; `dry_run`)

command-macro-publish =
    .title = Publish a macro to your library, for every workspace; not undone (`node`, the selected one by default, or `id`; `dry_run`)

command-macro-toggle =
    .title = Open the selected macro, or fold the one the selected node is in

command-macro-upgrade =
    .title = Move a macro to a newer version of its file (`node`, `version`; `dry_run`)

command-mask-add-circle =
    .title = Masks: add a circle at the view's centre

command-mask-add-ellipse =
    .title = Masks: add an ellipse at the view's centre

command-mask-add-gradient =
    .title = Masks: add a gradient across the view's centre

command-mask-add-parametric =
    .title = Masks: add a parametric member, reading the masked module's input

command-mask-add-path =
    .title = Masks: draw a path (click its points; with `corners`, made at once)

command-mask-cancel =
    .title = Masks: stop drawing the path

command-mask-handle-move =
    .title = Masks: move a shape's handle (`handle`: centre, radius, feather, corner.2...) to `x`, `y`, one undo step

command-mask-member-delete =
    .title = Masks: take a member (`member`, the selected one by default) out of the mask; rewires the network, no undo step

command-mask-member-invert =
    .title = Masks: invert a member (`member`, the selected one by default), or back

command-mask-member-lock =
    .title = Masks: lock a member (`member`, the selected one by default) against moves and edits, or unlock it; `locked` sets it

command-mask-member-move =
    .title = Masks: move a member (`member`, the selected one by default) to `index` of `group` (its own, last, by default); rewires the network, no undo step

command-mask-member-opacity =
    .title = Masks: set a member's (`member`, the selected one by default) `opacity`, 0 to 1; without it, off or back to full

command-mask-member-rename =
    .title = Masks: name a member (`member`, the selected one by default) `label`; without it, rename in the Masks panel

command-mask-paste =
    .title = Masks: paste copied shapes into the mask edited, scaled to the image (`clip`: a `helios.mask` payload)

command-mask-path-close =
    .title = Masks: close the path being drawn

command-mask-point-add =
    .title = Masks: add a point to the selected path, on its edge nearest `x`, `y`

command-mask-point-remove =
    .title = Masks: remove the selected path point (or `point`)

command-mask-point-smooth =
    .title = Masks: make the selected path point (or `point`) smooth, or sharp with `smooth: false`

command-mask-select =
    .title = Masks: select a mask, a shape of it, a path point (`mask`, `shape`, `point`)

command-network-add =
    .title = Add nodes to the network (`nodes`: each `module`, `composite` or `copy_of`, `at`: [x, y])

command-network-connect =
    .title = Connect an output to an input (`from`, `port`, `to`, `input`, `replaces`)

command-network-delegate =
    .title = Delegate nodes of the network to a new sub-workspace (`name`, `nodes`, `dry_run`)

command-network-disconnect =
    .title = Remove wires of the network (`connections`: each `from`, `port`, `to`, `input`)

command-network-move =
    .title = Move nodes in the graph view (`moves`: each `node`, `to`: [x, y])

command-network-paste =
    .title = Paste copied nodes into the network (`clip`: a `helios.nodes` payload; `parts`), renamed on a clash

command-network-remove =
    .title = Remove nodes from the network, with their wires (`nodes`)

command-node-bypass =
    .title = Bypass a module or bring it back (`node`: the selected one by default; `on`: toggled by default)

command-node-reset =
    .title = Reset a module's parameters to their defaults, its bypass kept (`node`: the selected one by default); one undo step

command-notice-details =
    .title = Show a failure's details (`notice`: its index; the last failure by default)

command-param-copyPath =
    .title = Copy a value's path (`path`)

command-param-copyValue =
    .title = Copy a value (`path`)

command-param-get =
    .title = Read a value (`path`)

command-param-list =
    .title = List the values under a path (`prefix`)

command-param-paste =
    .title = Paste onto a value (`path`; `dry_run` says what it would set)

command-param-redo =
    .title = Redo the last undone change of values

command-param-reset =
    .title = Reset a value to its default (`path`)

command-param-set =
    .title = Set a value (`path`; `value`, `step`, `coarse`, `reset`, `top` or `bottom`; `force`, `if_seq`)

command-param-undo =
    .title = Undo the last change of values

command-param-undoAgent =
    .title = Undo an agent's last changes alone (`agent`, `group`)

command-photo-show =
    .title = Show a photo on a display surface (`photo`: its id; `surface`: helios.viewport, helios.compare or another Compare helios.compare#<n>, new for a new Compare, new_window for one in a window of its own, else the double-click strategy chooses; `from`: the panel asking)

command-profile-export =
    .title = Export the profile (all parts, or `parts`; `this_machine` flattens its variants)

command-profile-import =
    .title = Read a profile and show what it would change

command-profile-import-apply =
    .title = Apply the profile just read

command-profile-resolved =
    .title = Describe the profile in use as resolved for this machine: which values came from which variant

command-profile-use =
    .title = Use a saved profile by name

command-scopes-histogram =
    .title = Scopes: histogram

command-scopes-linear =
    .title = Scopes: linear

command-scopes-parade =
    .title = Scopes: RGB parade

command-scopes-stops =
    .title = Scopes: in stops (log)

command-scopes-waveform =
    .title = Scopes: waveform

command-subworkspace-delete =
    .title = Delete a sub-workspace, its nodes kept in the network (`id`, `dry_run`)

command-subworkspace-enter =
    .title = Enter a sub-workspace: its nodes only, shown and edited (`id`; without it, the one holding the selected node)

command-theme-exportMerged =
    .title = Export the theme in use, overrides merged

command-theme-helios-classic-dark =
    .title = Theme: Classic Dark

command-theme-helios-classic-light =
    .title = Theme: Classic Light

command-theme-helios-dark =
    .title = Theme: Dark

command-theme-helios-high-contrast =
    .title = Theme: High Contrast

command-theme-helios-light =
    .title = Theme: Light

command-theme-select =
    .title = Use a theme by name

command-vault-autoAdvance =
    .title = Culling: move to the next image after each mark, in the Library view in use (`on`; toggled when absent)

command-vault-flag-none =
    .title = Take the pick or reject off the selected images

command-vault-flag-pick =
    .title = Pick the selected images

command-vault-flag-reject =
    .title = Reject the selected images

command-vault-label-blue =
    .title = Toggle the blue label on the selected image

command-vault-label-green =
    .title = Toggle the green label on the selected image

command-vault-label-purple =
    .title = Toggle the purple label on the selected image

command-vault-label-red =
    .title = Toggle the red label on the selected image

command-vault-label-yellow =
    .title = Toggle the yellow label on the selected image

command-vault-next =
    .title = Select the next image

command-vault-previous =
    .title = Select the previous image

command-vault-rate =
    .title = Rate the selected image (`stars`, 0 to 5)

command-vault-rate-0 =
    .title = Rate the selected image 0 stars

command-vault-rate-1 =
    .title = Rate the selected image 1 stars

command-vault-rate-2 =
    .title = Rate the selected image 2 stars

command-vault-rate-3 =
    .title = Rate the selected image 3 stars

command-vault-rate-4 =
    .title = Rate the selected image 4 stars

command-vault-rate-5 =
    .title = Rate the selected image 5 stars

command-vault-reject =
    .title = Reject the selected image, or take it back

command-vault-stack-toggle =
    .title = Expand or collapse the stack of the selected image

command-view-culling =
    .title = Culling: flag, rate and label from anywhere, the image first (`on`; toggled when absent)

command-view-library =
    .title = Go to the library: its photos to browse, rate and cull

command-view-network =
    .title = Go to the workspace's network, to edit it

command-viewport-compare =
    .title = Image: before and after side by side

command-viewport-copyImage =
    .title = Copy Image as PNG

command-viewport-mask-display =
    .title = Image: show the mask (cycles overlay, grey, off; or `mode`)

command-viewport-mask-display-off =
    .title = Image: stop showing the mask

command-viewport-zoom-100 =
    .title = Image: actual pixels, 100 %

command-viewport-zoom-fill =
    .title = Image: fill the view (cropping the longer side)

command-viewport-zoom-fit =
    .title = Image: fit to the view

command-viewport-zoom-in =
    .title = Image: zoom in

command-viewport-zoom-out =
    .title = Image: zoom out

command-window-maximize =
    .title = Maximize or restore the window

command-window-minimize =
    .title = Minimize the window

command-workbench-keybindings =
    .title = Keyboard shortcuts

command-workbench-quickOpen =
    .title = Go to a module, a sub-workspace, an image or a workspace

command-workbench-quickOpen-go =
    .title = Go to what quick open offers (`item`: module:<node>, subworkspace:<id>, image:<id> or workspace:<path>)

command-workbench-quit =
    .title = Quit Helios

command-workbench-run =
    .title = Run a command by id (`command`)

command-workbench-systemMenu =
    .title = Show the Helios menu

command-workshop-aim =
    .title = Experimental: aim an applied workshop's targets again on the open image, one value step (`node`, the selected one by default; `dry_run`)

command-workshop-answer =
    .title = Experimental: answer a proposed value: accept it or set your own (one undo step each), or reject it; recorded in the session (`of`: the proposal's step; `param`; `action`: accept, override or reject; `value` for override); yours alone

command-workshop-apply =
    .title = Experimental: apply a workshop after a node, aimed on the open image (`workshop`: id@version; `after`: the node the image is shown from by default; `aim`: true by default; `dry_run`)

command-workshop-context =
    .title = Experimental: what an agent reads to design a workshop: the words, the image's measures, the modules, the bricks; a proxy only when allowed (`intent`, the session's by default; `proxy`; `modules`)

command-workshop-describe =
    .title = Experimental: start designing a workshop from your words, answered by your own agent over MCP (`intent`; `cancel`)

command-workshop-design =
    .title = Experimental: check a workshop draft on the open image and apply it as one undo step (`draft`; `intent`, the session's by default; `apply`: true by default; `replace`; `dry_run`)

command-workshop-generalise =
    .title = Experimental: make the workshop piloted automatic: one row per slider (a target a measure explains, else a fixed value, or kept as AI) and per AI zone (a fitted parametric mask, or kept as AI), with evidence, and J, A (the AI left, calls per photo) (`rows`: choices by name; `confirm`: write the next version, yours alone; `session`; `dry_run`)

command-workshop-propose =
    .title = Experimental: propose values for the workshop being piloted, shown as ghost values the user accepts, overrides or rejects (`values`: by slider name; `aim`: the words it answers; `session`)

command-workshop-review =
    .title = Experimental: review a workshop run, flagged photos first, with their flags, provenance and values before and after; or act on one photo (`run`, the last one by default; `flag`: only photos raising it; `action`: accept or revert (yours alone), fix (`values`), decide a junction left to its fallback (`junction`, `value`, recorded in the decision book), rerun the held and decided photos (yours alone); `image`)

command-workshop-run =
    .title = Experimental: run the applied workshop on a set of photos, a job: each photo aimed, its junctions answered, checked against the limits, written or held; one undo step (`node`, the selected one by default; `set`: selection or view; `images`: ids instead; `on_breach`: flag, hold or stop; `dry_run`: the photos, J, A and what the decision book already answers)

command-workshop-save =
    .title = Experimental: save a macro as a workshop, its next version with an intent, keywords and targets (`node`, the selected one by default; `intent`, `keywords`, `targets`; `dry_run`); without `intent`, a form asks

command-workshop-send-proxy =
    .title = Experimental: let an agent see a small proxy of the open image, for this session (`on`); the user's switch only

command-workshop-session =
    .title = Experimental: the workshop session's record: each step piloting the workshop, its author, aim and zone measures (`action`: read, keep (yours alone: a session not kept is dropped when it ends) or close)

command-workshop-suggest =
    .title = Experimental: find workshops by what you want, ranked by name, keywords, then intent (`intent`)

command-workspace-edit =
    .title = Edit the workspace (`name`, `description`, `icon`, `changes`)

command-workspace-importParts =
    .title = Add parts from a workspace or template (`source`, `parts`, `picks`, `dry_run`)

command-workspace-importParts-apply =
    .title = Add the parts reviewed, each conflict's side picked

command-workspace-importParts-review =
    .title = Review the parts to add: what changes, and the side to keep for each conflict

command-workspace-library =
    .title = Library settings of the workspace

command-workspace-new =
    .title = New workspace from a template (`template`, `path`)

command-workspace-open =
    .title = Open a workspace in a new window (`path`)

command-workspace-parts =
    .title = List the parts of the workspace, or of a workspace or template (`source`)

command-workspace-reset =
    .title = Reset the workspace to its template (`dry_run`)

command-workspace-saveAsTemplate =
    .title = Save the workspace as a template (`id`, `name`, `description`, `icon`, `dry_run`)
