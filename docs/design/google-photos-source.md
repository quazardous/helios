# The Google Photos source: mock-ups for a redesign

Status: **mock-ups, waiting for the owner's choice** (2026-11-13). Nothing
here is built. The current behaviour is described in
[../google-photos.md](../google-photos.md) and in
[import-connectors.md](import-connectors.md), section 4.1; the import
window itself in [import-window.md](import-window.md).

## Why

The owner found the Google Photos source's buttons and flow
incomprehensible (2026-11-13). The screens below start from the code as
it is (`crates/helios-gui/src/shell/import_accounts.rs`,
`import_window.rs`, `import_browsers.rs`), then redraw them following
established apps that import from a service (Lightroom, Apple Photos):
one primary action per state, the account shown once, the chosen photos
in the ordinary grid, plain words.

## 1. The screens today

Drawn from the code. The import window keeps its three columns (Sources,
Contents, Choices) and its footer for every source.

### 1.1 Sources column, a connected account saved as a source

```
SOURCES
 Cards and cameras
 Recent
  ...
 ──────────────────────
 Saved
  My Google              ← right-click: Pick photos… · Edit… · Forget
 ──────────────────────
 Accounts
  Google Photos — me@example.com   connected
  [Pick photos…]                     ← a button under the row
                                     ← right-click: Reconnect · Sign out
 Add a source…
```

An old account (connected before Helios asked for the address), or one
Google stopped:

```
 Accounts
  Google Photos   needs attention    ← the reason only in a tooltip
  Reconnect to see the account
  [Reconnect]
```

### 1.2 Contents column, the account selected

```
CONTENTS
 ──────────────────────────────────────────────
 Google Photos — me@example.com
 Choose the photos in the service's own page: they are imported once
 picked.
 [Pick photos…]
```

The Choices column (Copy / Add where they are, Into, Name, Duplicates…)
and the footer's `[Import N]` stay on screen, though a picking ignores
most of them and the footer button never applies.

### 1.3 The picking dialog (modal over the window)

```
┌ Pick photos in Google Photos — me@example.com ──────┐
│ ◌ Pick your photos in your browser, press Done,     │
│   then come back here.                              │
│ Your browser must be signed in to the account       │
│ connected as Google Photos — me@example.com; if the │
│ page says it cannot open, sign in to that account   │
│ and start again.                                    │
│ https://photos.google.com/picker/…                  │
│ [Open again] [Copy link]                            │
│                                                     │
│ [Start again] [Cancel]                              │
└─────────────────────────────────────────────────────┘
```

Then, in the same dialog: "Importing 212 photos…", "198 photos
imported. 14 already in the library, left as they are."
`[Pick more photos…] [Close]`; or "Nothing was picked in time.", "The
picking session expired, start again.", "The source refused the sign-in:
reconnect the account.", "Not imported: …", and, when no library folder
is known, "Picked photos are copied into the library folder. Choose it
once: it is remembered." `[Choose the library folder…]`.

### 1.4 What confuses

1. **"Pick" everywhere, for two things.** "Pick photos…" is on the
   account row, in the saved source's menu, in Contents, then "Pick more
   photos…" and "Start again" in the dialog: five buttons for one
   action. And in the Library, "pick" is a flag (pick / reject).
2. **The account twice**, under two names: once under Saved ("My
   Google"), once under Accounts ("Google Photos — me@example.com").
3. **Account actions mixed with importing**: Reconnect and Sign out sit
   in the Sources column next to "Pick photos…"; Settings > Accounts says
   "Disconnect" for the same act.
4. **The photos are imported the moment Google's page says Done**: no
   grid, nothing to review, the Choices column and the footer's Import
   button do nothing for this source.
5. **A modal blocks the window** while Google's page is open in the
   browser, and its "Start again" next to "Cancel" is ambiguous.
6. **Jargon**: "picking session", "the source refused the sign-in",
   "Pick in the service", "needs attention" with its reason in a
   tooltip only.
7. **A question mid-flow**: the library folder is asked inside the
   picking, not where the destination is chosen (Choices > Into).

## 2. The redesign (option A, recommended)

Rules:

- **One primary action per state**, the button of the state, in the
  middle of the Contents column; nothing else looks like it.
- **"Choose", never "pick"** ("pick" stays the Library's flag).
- **The account once**: one row in Sources, one header line in Contents
  with its menu. Managing accounts lives in Settings > Accounts; the
  menu links there.
- **The chosen photos are an ordinary source**: they fill the grid with
  the "in your Library" badges, the Choices column applies, the footer's
  Import imports them, like a card.
- **No modal**: the wait for Google's page is a status line in Contents,
  with Cancel; the rest of the window stays usable.
- **No technical words**: no session, token, picker, OAuth, scope.

### 2.1 Sources column

```
SOURCES
 Cards and cameras
  ...
 ──────────────────────
 Services
  ● Google Photos             ← one row per account; a saved source's
    me@example.com               name replaces "Google Photos"
  ▲ Google Photos             ← needs reconnecting: amber mark, a word
    me@other.org · reconnect     on the row itself, not in a tooltip
 ──────────────────────
 Add a source…
```

No buttons in this column. Right-click on an account row: **Choose
photos…**, **Reconnect**, **Manage accounts…** (Settings > Accounts),
then the source's own items (Edit…, Forget) when it is saved.

### 2.2 Not connected (Add a source… > Google Photos)

```
CONTENTS
 Google Photos
 ──────────────────────────────────────────────────────────

            Import photos you choose in Google Photos.

                  [ Connect Google Photos ]

     Your browser opens Google's sign-in page. Helios only sees
     the photos you choose, never the rest of your library.
     What Google sends: copies, without their location. More…
```

Footer: `[Cancel]  [Import]` disabled, with "Connect first" beside it.

### 2.3 Connecting

```
CONTENTS
 Google Photos
 ──────────────────────────────────────────────────────────
 ◌ Sign in to Google in your browser, then come back here.
   The page did not open?  Open it again · Copy the link
                                                    [Cancel]
```

### 2.4 Connected, nothing chosen yet

```
CONTENTS
 Google Photos · me@example.com                          [⋯]
 ──────────────────────────────────────────────────────────

              [ Choose photos in Google Photos… ]

     Google Photos opens in your browser. Choose up to 2,000
     photos, press Done there, then come back here.
```

`[⋯]` is the account menu: **Reconnect**, **Use another account…**
(connects one more account, then selects it), **Manage accounts…**.

### 2.5 Choosing (Google's page open in the browser)

```
CONTENTS
 Google Photos · me@example.com                          [⋯]
 ──────────────────────────────────────────────────────────
 ◌ Choose your photos in the browser, press Done there, then
   come back here.
   Google shows the photos of the account your browser is
   signed in to: me@example.com.
   The page did not open?  Open it again · Copy the link
                                                    [Cancel]
```

The window stays usable. After 30 minutes without an answer the line
becomes the "nothing chosen" state (2.8).

### 2.6 Photos chosen: the grid

```
┌ Import photos ───────────────────────────────────────────────────────────────────────────────────┐
│ SOURCES            │ Google Photos · me@example.com                 [⋯] │ CHOICES                   │
│ Cards and cameras  │ 212 chosen · 198 new · 1.4 GB  [Choose more…]      │ Copied into the Library   │
│ ──────────────     │ [New only ✓]  [Select: all · none · day]           │  (photos from Google are  │
│ Services           │ ── 2026-10-30 · 40 ─────────────────────── [✓]     │   always copied)          │
│ ▣ Google Photos    │ ┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐                    │ Into Pictures/2026/… [▸]  │
│   me@example.com   │ │✓ ││✓ ││░░││✓ ││✓ ││✓ ││✓ ││✓ │ ░░ = in your       │ Name {stem}          [▾]  │
│ ──────────────     │ └──┘└──┘└──┘└──┘└──┘└──┘└──┘└──┘      Library      │ Duplicates skip      [▾]  │
│ Add a source…      │ ── 2026-11-02 · 172 ────────────────────── [✓]     │                           │
│                    │ ┌──┐┌──┐┌──┐ …                                     │                           │
├────────────────────┴────────────────────────────────────────────────────┴───────────────────────────┤
│ Preset: Default ▾                                       198 photos, 1.3 GB    [Cancel]  [Import 198] │
└──────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- The thumbnails come from Google's small previews; nothing full size is
  downloaded before Import.
- A photo already imported from Google (by its Google id) or already in
  the Library (by its hash, once downloaded) is dimmed with "in your
  Library", and unticked by "New only".
- **Choose more…** opens Google's page again; the photos chosen are
  added to the grid.
- Import runs as for a card: the window's progress line, the toast, the
  import history. A link that expired (Google's one hour) is renewed
  without a word.
- No library folder yet: the Choices column's **Into** asks for it, as
  for any copy, and Import waits for it.

### 2.7 The account needs reconnecting

```
CONTENTS
 Google Photos · me@example.com                          [⋯]
 ──────────────────────────────────────────────────────────
 ▲ Google no longer lets Helios read this account.
   This happens when access was removed in your Google account,
   or after 7 days when the app is in Google's testing mode.

                  [ Reconnect me@example.com ]
```

An account connected before Helios asked for the address:

```
 Google Photos                                           [⋯]
 ──────────────────────────────────────────────────────────
 Reconnect once to show which Google account this is.

                       [ Reconnect ]
```

### 2.8 Nothing came back, and errors

Each says what happened, plainly, and offers the one action that helps:

| Situation | Text | Primary |
|---|---|---|
| Google's page left without Done, 30 minutes | No photos were chosen. | Choose photos in Google Photos… |
| Google's page closed or ran out | Google's page has closed. Open it again to choose your photos. | Choose photos in Google Photos… |
| The browser is on another account | Google's page did not open: your browser may be signed in to another account. Sign in as me@example.com, then try again. | Try again |
| Network | Could not reach Google Photos: <reason>. | Try again |
| Access refused during the import | as 2.7 | Reconnect me@example.com |

## 3. The alternatives

- **A (recommended): sections 2.1 to 2.8.** The waiting is a status line
  in Contents; the chosen photos fill the grid; the footer's Import
  imports them, with the Choices column. Google behaves like a card.
- **B: as A, but the wait stays a dialog.** Sections 2.3 and 2.5 are
  drawn in a modal over the window (as today, with the new words); the
  rest as A. Simpler to build; the window is blocked while Google's page
  is open.
- **C: as A, but Done imports at once.** No grid and no Import press:
  Google's page already was the choice. Contents shows the result
  ("198 photos imported, 14 were already in your Library") and
  **Choose more photos…**. Fewer clicks; the Choices column applies but
  nothing can be unticked or reviewed before the download.

## 4. Building it (after the choice)

One pull request: the words (`gui-pick-*`, `gui-account-pick`,
`gui-import-browse-pick*` replaced), the account's single row and header
menu, the Contents states, the chosen photos as a listing source (A, B),
and a kittest check per state against the fake Google server the plugin's
tests already use.
