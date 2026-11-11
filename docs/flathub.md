# Submitting Helios to Flathub

Helios is not on Flathub yet. Its Flatpak is built and tested by the
release workflow and attached to each release as a bundle
([releases.md](./releases.md), "The Flatpak"). This page lists what
submitting it takes, following Flathub's
[requirements](https://docs.flathub.org/docs/for-app-authors/requirements)
and [submission](https://docs.flathub.org/docs/for-app-authors/submission)
guides. Nothing here has been submitted.

## Already in place

- The app ID `io.github.quazardous.Helios`: a `io.github.<user>` ID is
  verified by Flathub through the GitHub account that owns the repository.
- A manifest that builds from source with no network
  (`packaging/flatpak/io.github.quazardous.Helios.yml`), on a current
  freedesktop runtime, with the crates from a generated
  `cargo-sources.json`.
- A desktop entry, icons up to 512 px and an SVG, all named by the app ID.
- AppStream metadata (`packaging/linux/io.github.quazardous.Helios.metainfo.xml`):
  a summary, a description, the license (GPL-3.0-or-later; the metadata
  CC0-1.0), the developer, links, an OARS content rating, branding colours,
  and a `<release>` for each version, added by `cargo xtask release`. It
  passes `appstreamcli validate`.
- The license installed in `/app/share/licenses/io.github.quazardous.Helios`.

## Still to do

1. **Screenshots.** Flathub requires at least one, of the whole window, in
   the metainfo's `<screenshots>`, with a caption, at a stable URL (a file
   in this repository on a tag, or Flathub's screenshot mirror). Take
   them from a release build in a headless session, not from a
   development build.
2. **A stable source in the manifest.** Flathub's copy replaces the
   `type: dir` source with the release tag:

   ```yaml
   - type: git
     url: https://github.com/quazardous/helios.git
     tag: vX.Y.Z
     commit: <the tag's commit>
   ```

   and commits the `cargo-sources.json` generated from that tag's
   `Cargo.lock` beside the manifest (`packaging/flatpak/build.sh` shows
   how). Each release then updates the tag, the commit and that file in
   the Flathub repository (by hand, or with Flathub's
   `x-checker-data` and a script that regenerates the crates).
3. **The linter.** Run Flathub's linter on the manifest, the build and
   the repository, and fix or justify each finding:

   ```bash
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest io.github.quazardous.Helios.yml
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo repo
   ```

   Expect questions on `--share=network` (the AI model providers),
   `--filesystem=/media` and `/run/media` (photo cards) and the darktable
   folders (the library import): the reasons are in releases.md.
4. **The folder picker.** Flathub reviewers prefer the file chooser portal
   to wide filesystem permissions. Helios uses its own picker; offering
   the desktop's chooser inside the sandbox (egui-pop's `native-dialogs`,
   through the portal) would let a user pick any folder without an
   override.
5. **The pull request.** Fork `flathub/flathub`, branch from its
   `new-pr` branch, add the manifest and `cargo-sources.json`, open the
   pull request against `new-pr`, and answer the review. Once merged,
   Flathub creates `flathub/io.github.quazardous.Helios`, where later
   releases are made.
6. **After acceptance.** Verify the app on Flathub's developer portal
   (the GitHub account), and add the Flathub badge to README.md.
