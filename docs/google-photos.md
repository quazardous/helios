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

Google requires every desktop application reading Google Photos to have
an OAuth client of its own. Helios does not ship one: you create yours
once, in your own Google account. It is free.

1. Open the [Google Cloud console](https://console.cloud.google.com/)
   and create a project (any name, such as "Helios photos").
2. In **APIs & Services > Library**, find **Photos Picker API** and
   enable it. (Not the "Photos Library API": it no longer reads your
   library.)
3. In **APIs & Services > OAuth consent screen** (Google Auth Platform):
   - user type **External**, an app name, your e-mail address as the
     support and developer contact;
   - under **Data access** (scopes), add
     `https://www.googleapis.com/auth/photospicker.mediaitems.readonly`;
   - under **Audience**, leave the app in **Testing** and add your own
     Google account as a **test user**. Only test users can sign in
     while the app is in testing, which is what you want.
4. In **APIs & Services > Credentials**, **Create credentials > OAuth
   client ID**, application type **Desktop app**. Google shows a client
   ID and a client secret. For a desktop app Google does not treat the
   secret as confidential, but keep both to yourself.
5. Give them to Helios through the environment, before it starts:

   ```sh
   export HELIOS_GOOGLE_PHOTOS_CLIENT_ID="1234-abc.apps.googleusercontent.com"
   export HELIOS_GOOGLE_PHOTOS_CLIENT_SECRET="GOCSPX-…"
   ```

   (On Windows, set them as user environment variables.)

## Signing in

In the import window, **Add a source… > Google Photos**. Helios opens
your browser on Google's sign-in page; choose your account and allow
"See photos you select". Google may warn that the app is not verified:
it is your own app, in testing, so continue. The browser then says to
go back to Helios.

Helios keeps the sign-in in your system keyring, never in the Library.
**Settings > Accounts** shows the account; **Sign out** there revokes
Helios's access at Google and forgets it. When Google refuses the access
later (you revoked it in your Google account, or a testing app's
sign-in aged out: Google ends them after seven days), the account shows
**needs attention** and **Reconnect** signs in again.

## Picking and importing

In the import window, a connected Google Photos account shows **Pick
photos…**. Helios creates a picking session and opens Google Photos' picker in
your browser. Select the photos, press **Done**; the tab closes and
Helios lists what you picked, downloads each photo and imports it into
your Library folder, then closes the session at Google. **Cancel** stops waiting; a picking
waits 30 minutes at most. A photo you
already imported from Google Photos is recognised by its Google id and
not downloaded again.

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
