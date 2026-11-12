# Google OAuth verification for the Google Photos source

What Google asks for before Helios's Google Photos import can leave
"Testing" without the "unverified app" warning, and the material for it.
The Google Cloud steps are done by the project's owner from the Google
Cloud console; this page is the checklist and the texts to paste.

## Pages (this repository)

- Home page: <https://quazardous.github.io/helios/> (`site/index.html`).
- Privacy policy: <https://quazardous.github.io/helios/privacy.html>
  (`site/privacy.html`).

Both are published by `.github/workflows/pages.yml` from `main`.

## Checklist

1. **Prove the domain.** In Google Search Console, add a URL-prefix
   property for `https://quazardous.github.io/helios/` and choose the HTML
   file method. Put the file Google gives (`google<token>.html`) in `site/`,
   merge, wait for the Pages deployment, then press Verify. Google Auth
   Platform's authorized domain is `quazardous.github.io`; if it asks to
   verify the domain itself rather than the URL prefix, the same file has to
   be served from the root `https://quazardous.github.io/`, which needs a
   repository named `quazardous.github.io`.
2. **Branding** (Google Auth Platform → Branding): app name "Helios", the
   logo (`docs/brand/logo.svg` exported to a 120×120 PNG), the support
   e-mail, the home page and privacy policy links above, the authorized
   domain `quazardous.github.io`.
3. **Data access:** the only scope is
   `https://www.googleapis.com/auth/photospicker.mediaitems.readonly`.
4. **Audience:** Publish app (from "Testing" to "In production").
5. **Verification Center:** submit with the justification and the video
   below, then answer Google's e-mails.

## Scope justification (to paste)

> Helios is a free, open-source desktop photo editor. Its Google Photos
> source lets a user import photos from their Google Photos account into
> their local photo library. Helios uses the Google Photos Picker API only:
> the user picks photos in Google's own picking page, and Helios downloads
> exactly those photos to the user's computer, into the library folder they
> chose. The `photospicker.mediaitems.readonly` scope is the narrowest scope
> that allows this; Helios never reads the rest of the library, albums or
> account data. Nothing is sent to the developers or to third parties; the
> OAuth tokens are kept in the operating system's keyring on the user's
> computer.

## Demonstration video (script)

Record the screen, about two minutes, upload it to YouTube as unlisted,
and paste the link:

1. Start Helios, open File → Import, and show the "Add a source…" wizard
   choosing Google Photos.
2. Click to sign in: the browser opens Google's sign-in. Show the address
   bar with the OAuth client ID (`client_id=…` in the URL), pick the
   account, and show the consent screen naming Helios and the Google
   Photos permission. Accept.
3. Back in Helios, choose "Pick photos…": Google's picking page opens. Pick
   two or three photos and confirm.
4. Show the photos arriving in the Helios import window and in the library
   folder on disk.
5. Show Settings → Accounts with the connected account and the way to
   disconnect it, and mention revoking access from the Google account's
   third-party access page.
