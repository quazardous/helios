# Google Photos

Helios imports the photos you **pick** in Google Photos. Since March 2025
Google lets an application read your library only through its Photos
Picker: you choose the photos in Google Photos' own page, Helios lists
and downloads what you chose. Helios never sees the rest of your
library, and Google keeps nothing of Helios's between two pickings.

What to know before you start:

- **Copies, not originals.** Google serves the photos with their Exif
  but **without their location**, and its bytes may differ from the
  file you uploaded (photos kept in "Storage saver" are re-encoded in any
  case). Helios imports them as copies in your Library folder and marks
  their source; for the originals and the location, use a Google
  Takeout export.
- **At most 2,000 photos** a picking; pick again for more.
- **No automatic import.** Each import starts with a picking.

## Setting up access

Google requires every desktop application reading Google Photos to sign
in with an OAuth client. Helios looks for one in this order:

1. **Your own**, set in **Settings > Accounts > Use my own OAuth
   client**: the client ID and secret of a desktop application you
   registered (below). Both stay in your system keyring. Sign in again
   afterwards.
2. **The environment**, for scripts and tests: the variables
   `HELIOS_GOOGLE_PHOTOS_CLIENT_ID` and
   `HELIOS_GOOGLE_PHOTOS_CLIENT_SECRET`, read when Helios starts.
3. **Helios's own**, built into the release packages. A build made
   without it (a build from source, a development build) has none: use
   your own.

The client ID and the secret always come from the same place. For a
desktop application Google does not treat the secret as confidential:
it ends up in the program anyway, and what protects your sign-in is
PKCE and the answer coming back to your own machine only.

The same section of Settings says which client is used, and **Use
Helios's client** forgets yours.

### Creating your own client

It is free, and done once, in your own Google account.

1. Open the [Google Cloud console](https://console.cloud.google.com/)
   and create a project (any name, such as "Helios photos").
2. In **APIs & Services > Library**, find **Photos Picker API** and
   enable it. (Not the "Photos Library API": it no longer reads your
   library.)
3. In **APIs & Services > OAuth consent screen** (Google Auth Platform):
   - user type **External**, an app name, your e-mail address as the
     support and developer contact;
   - under **Data access** (scopes), add
     `https://www.googleapis.com/auth/photospicker.mediaitems.readonly`,
     `openid` and `email` (the last two show which account is
     connected);
   - under **Audience**, leave the app in **Testing** and add your own
     Google account as a **test user**.
4. In **APIs & Services > Credentials**, **Create credentials > OAuth
   client ID**, application type **Desktop app**. Google shows a client
   ID and a client secret.
5. Type them in **Settings > Accounts > Use my own OAuth client**, on
   the Google Photos line.

### Google's consent screen modes

A Google Cloud project's consent screen is in one of these states:

- **Testing**: only the test users listed in the console can sign in,
  at most 100, and Google ends their sign-in after seven days (Helios
  then shows the account as needing attention: Reconnect).
- **In production, unverified**: anyone can sign in, after clicking
  through Google's "this app isn't verified" warning, up to a capped
  number of users.
- **In production, verified**: no warning. The Photos Picker scope is
  sensitive, so going to production for everyone needs Google's
  verification of the app (its home page, privacy policy, and a video
  of the sign-in and of what the scope is used for).

### For maintainers: the built-in client

The release workflow (`.github/workflows/release.yml`) passes two
repository secrets to the build as environment variables, and
`helios-connect` compiles them in (`helios_connect::oauth::built_in`):

- `HELIOS_GOOGLE_PHOTOS_CLIENT_ID`
- `HELIOS_GOOGLE_PHOTOS_CLIENT_SECRET`

Without them the packages have no built-in client and every user sets
their own. Their values never go into the repository. A local build
made with these variables set compiles them in the same way. The
Flatpak is built in its own sandbox, which sees no job environment: the
release workflow writes them to `packaging/flatpak/build.env` (ignored by
git, removed after the build), which the manifest's build command reads
before compiling.

## Signing in

In the import window, **Add a source… > Google Photos**. Helios opens
your browser on Google's sign-in page; choose your account and allow
"See photos you select" and your e-mail address. Google may warn that the app is not verified:
it is your own app, in testing, so continue. The browser then says to
go back to Helios.

Helios keeps the sign-in in your system keyring, never in the Library.
The account is shown with its address, "Google Photos — name@example.com",
in the import window, the source editor and **Settings > Accounts**, so
several accounts stay apart. An account connected before Helios asked
for the address says **Reconnect to see the account**: one reconnection
(one consent) shows it. **Settings > Accounts** shows the account, the
sources using it and when it was last used; rename it there, or
**Disconnect** it: Helios revokes its access at Google and forgets it,
and asks whether the sources using it are kept (until you reconnect) or
forgotten. When Google refuses the access later (you revoked it in your
Google account, or a testing app's sign-in aged out: Google ends them
after seven days), the account shows **Access refused** and **Reconnect**
signs in again.

## Picking and importing

In the import window, a connected Google Photos account shows **Pick
photos…**. Helios creates a picking session and opens Google Photos' picker in
your browser. Select the photos, press **Done**; the tab closes and
Helios lists what you picked, downloads each photo and imports it into
your Library folder, then closes the session at Google. **Cancel** stops waiting; a picking
waits 30 minutes at most. A photo you
already imported from Google Photos is recognised by its Google id and
not downloaded again.

Pick again whenever you want: **Pick photos…** stays on the account, in
its source's menu and in the Contents column, and a finished picking
offers **Pick more photos…**. Each is a new picking session; its photos
are added to the same source, and those already in the Library are said
so and left as they are. Google's download addresses last an hour: one
that expired during an import is renewed by listing the picking again,
never shown as an error.

## For developers

The plugin lives in `plugins/sources/google-photos`: a Rust program
speaking the `import.source` service over JSON-RPC, and a library whose
`GooglePhotos` type is a `helios_connect::Connector` over one picking
session. Its tests run it against a fake Picker API on a local port
(`tests/fake_picker`); they never reach Google. Helios drives the
program through its plugin broker as an import connector
(`helios_plugin::source`): each call carries the account's access token,
the plugin downloads each photo into a staging folder Helios gives, and
its error -32010 marks the account "needs attention". The plugin runs
only once its `network` and `folder.write` capabilities are granted
(`plugin.grant`). The design is in
[design/import-connectors.md](design/import-connectors.md), section 4.1.
