# Releases

A release is a "Release X.Y.Z" commit on `main`, its tag `vX.Y.Z` and a
GitHub Release of the same name on the
[Releases page](https://github.com/quazardous/helios/releases). The version
moves only when a release is made, and every release is published.

## Releases: asked for by a fragment

Every change reaches `next` through the tester's gate (docs/next.md). When
CI passes on a commit of `next`,
the promotion workflow (`.github/workflows/promote.yml`, running
`cargo xtask promote`) moves `main` forward:

- **It works on the exact commit CI validated**, the run's `head_sha`,
  never on whatever `next` points to when the promotion runs. A commit CI
  did not test never reaches `main`.
- When a fragment in `changelog.d/` asks for a release (a line
  `release: patch`, `minor` or `major` under its `kind:`), it commits
  "Release X.Y.Z" on top of that commit, as the bot: the CHANGELOG.md
  section of every pending fragment, the version (the strongest of what
  their kinds call for and what is asked; CONTRIBUTING.md, "Versions and
  releases"), the in-tree plugins' Helios range, the `AppStream` metadata
  and Cargo.lock. Fragments that ask for none wait: the commit alone is
  promoted.
- **The fallback:** a user-facing fragment that has waited 7 days (from
  the commit that added it to the commit promoted) is released anyway, by
  the next promotion, so a fix never waits for a feature.
- The release commit is refused unless it is the validated commit plus a
  version bump and nothing else; in Cargo.lock only the workspace's own
  versions may move. The survival scenarios are not run again: CI ran them
  on the validated commit, and the bump changes no code.
- `main` fast-forwards to the release commit. `next` does too, in the same
  atomic push, if it still points at the validated commit. If `next` has
  moved, `main` still gets the release (the validated tree plus the bump)
  and the release is merged into `next` by a merge commit, so nothing
  pushed is rewritten and both carry the same version. The promotion of the
  newer commit then starts from that merge.
- The workflow pushes with its own token, which starts no workflow: the
  release commit and the merge do not rerun CI (they change versions
  only); the next push to `next` tests them with whatever it brings.
- A branch started before a bump still carries the old version where it
  adds or moves a plugin: `cargo xtask check`, in CI on every pull
  request, fails when an in-tree plugin's Helios range is not the one the
  bump writes, or when Cargo.lock holds a workspace member at another
  version than its manifest. Its message says what to write; for the lock,
  `cargo update --workspace`. The branch's own CI fails, not `next`.

## Publishing: tagged by the promotion

The promotion that makes a release tags it `vX.Y.Z` and starts the release
workflow (`.github/workflows/release.yml`) with `publish` on: a tag pushed
with the workflow's token starts no workflow, a dispatch does. Nobody tags
by hand. Who decides is the coordinator, on the tester's advice: after a
green batch reaches `next`, the tester says what a user would notice since
the last release and recommends none, a patch or a minor one; a "yes" is a
one-line fragment `kind: internal` with `release: patch` (or `minor`)
through the same gate (docs/next.md). Authors put no `release:` line in
their own fragments.

The release workflow checks the tag against the workspace version, then:

1. builds `helios`, `helios-node`, `helios-mcp`, `helios-gui` and the
   programs of the plugins Helios ships (`helios-darktable-library`) in
   release, on Ubuntu 22.04, so they need glibc 2.35 or newer;
2. packages them (`packaging/linux/release.sh`): a tarball that runs in
   place, an AppImage, and `SHA256SUMS`. Both carry `COPYING` (the GPLv3)
   and `THIRD-PARTY-LICENSES.txt` (the crates built in, by license, from
   `cargo deny list`). The AppImage tools (appimagetool and its static
   runtime) come from their GitHub releases, pinned by version and
   SHA-256;
3. tests them (`packaging/linux/smoke.sh`): in a bare Ubuntu 22.04
   container, with no Vulkan loader and no GPU, the tarball's `helios`
   says its version, imports a picture into a new vault and exports it as
   a JPEG through a network on the CPU; the AppImage starts and says its
   version;
4. builds the same programs for Windows x86_64 (MSVC, `windows-latest`)
   and packages them (`packaging/windows/release.ps1`):
   `helios-X.Y.Z-windows-x86_64.zip`, a folder that runs where it is
   unpacked, with `COPYING`, `THIRD-PARTY-LICENSES.txt` and `README.md`,
   and `helios-X.Y.Z-windows-x86_64-setup.exe`, an installer of the same
   folder (see "The Windows installer" below);
5. tests the zip (`packaging/windows/smoke.ps1`) on a Windows runner,
   which has no GPU: `helios.exe` and `helios-gui.exe` say their version,
   `helios.exe` imports a picture into a new vault and exports it as a
   JPEG on the CPU; then the installer
   (`packaging/windows/smoke-installer.ps1`): a silent install into a
   temporary folder, the same tests from the installed folder, a silent
   uninstall, and a check that the files, the Start menu entry and the
   entry in Settings are gone;
6. builds the Flatpak (`packaging/flatpak/build.sh`) from the same
   source, as a single-file bundle
   `io.github.quazardous.Helios-X.Y.Z-x86_64.flatpak`, and tests it in
   the same job (`packaging/flatpak/smoke.sh`), with no GPU and no
   display: installed for the user, the command line in the sandbox says
   its version, imports a picture and exports it on the CPU, the window's
   program says its version (see "The Flatpak" below). The job runs in
   Flathub's builder image (`ghcr.io/flathub-infra/flatpak-github-actions`,
   the tag of the manifest's runtime), which brings flatpak-builder and
   everything the script needs, so it installs nothing from Ubuntu's
   archive;
7. creates the GitHub Release, titled "Helios X.Y.Z", with that version's
   CHANGELOG.md section as its notes, and attaches the files, with one
   `SHA256SUMS` for all of them;
8. opens the winget-pkgs pull request for the new installer (see "winget"
   below), when the `WINGET_TOKEN` secret is set.

Nothing else is needed at run time: the catalogues, templates, module
declarations and the shipped plugins' manifests are compiled into the
programs. The plugins' programs sit beside `helios-gui`, which writes their
folders into its cache on first use. Vulkan comes from the system: without
a loader or a driver, Helios processes on the CPU.

The workflow also runs by hand (Actions, "release", "Run workflow"), with
a tag and `publish`. With `publish` off, the default, it is a dry run: it
builds, packages and tests, and keeps the files as the run's artefacts,
without creating a Release. With `publish` on, it builds the tag given,
which must exist and match the workspace version. A dry run from the
command line, of the branch it is started on:

```bash
gh workflow run release.yml -R quazardous/helios --ref next -f tag=v0.0.0-dry
```

Each package's files, the Flatpak bundle among them (artefact
`helios-X.Y.Z-flatpak-x86_64`), are then the run's artefacts.

## When GitHub's CI is down

The bench box prepares a release instead (`cargo xtask bench-remote
release branch=next`): survival, the CHANGELOG section, the bump and
Cargo.lock, tested, as `out/release.patch`, which is committed as
"Release X.Y.Z" on a branch from `next` and merged into it. The release
workflow can also run by hand, as above, once Actions is back.

## The Windows installer

`helios-X.Y.Z-windows-x86_64-setup.exe` is built with
[Inno Setup](https://jrsoftware.org/isinfo.php) 6 from
`packaging/windows/helios.iss`. Inno Setup was chosen because it makes a
per-user installer (no administrator, no UAC prompt) with a Start menu
entry and an uninstaller from one short script, runs on GitHub's Windows
runners (installed with Chocolatey when the image lacks it) and needs no
.NET or WiX toolchain; it is a build tool only, nothing of it is linked
into Helios.

The installer:

- installs for the current user into `%LOCALAPPDATA%\Programs\Helios`
  (the folder can be changed), with the same files as the zip;
- adds a "Helios" Start menu entry, and a desktop one if asked;
- registers an uninstaller, listed in Settings, Apps; uninstalling
  removes the programs and the entries, and leaves the libraries,
  settings and caches in the user's folders alone;
- installs over an earlier version (the same `AppId`), closing a running
  Helios first.

It runs silently for scripted installs:

```powershell
.\helios-X.Y.Z-windows-x86_64-setup.exe /VERYSILENT /SUPPRESSMSGBOXES [/DIR="C:\path\Helios"]
& "$env:LOCALAPPDATA\Programs\Helios\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES
```

`helios-gui.exe` is a window program: started from the Start menu or
Explorer, no console opens beside it; started from a terminal, it prints
to that terminal (`--version`), though the prompt does not wait for it.
Its icon, version information and manifest (per-monitor DPI awareness,
paths longer than 260 characters) are embedded at build time
(`crates/helios-gui/build.rs`). Neither the installer nor the programs
are signed yet, so SmartScreen warns on first start.

## winget

Helios is published on the Windows Package Manager as `Quazardous.Helios`,
from the installer above (unsigned, decided 2026-11-12: no code signing
for now, winget only):

```powershell
winget install Quazardous.Helios
```

The manifests live in
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs), under
`manifests/q/Quazardous/Helios/X.Y.Z/`. `packaging/winget/` holds the
first set, for 0.77.0, in that same layout (schema 1.9.0): the version,
the default locale (en-US) and the installer manifest. The installer
manifest says what the Inno Setup script makes: `InstallerType: inno`,
`Scope: user` (per-user, no elevation), silent switches
`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-` (`/SILENT …` with
progress), the `ProductCode` `{833F5226-CF08-47A0-BF08-0B2A9389FFC3}_is1`
(the script's `AppId` plus `_is1`, the key Inno Setup registers the
uninstaller under), and the setup.exe URL and SHA-256 from the release's
`SHA256SUMS`.

Each tagged release then updates it on its own: the release workflow's
`winget` job, after `publish`, runs
[winget-releaser](https://github.com/vedantmgoyal9/winget-releaser)
(pinned by commit), which takes the manifests of the previous version in
winget-pkgs, swaps in the new setup.exe, its SHA-256 and the release notes
link, and opens the pull request on microsoft/winget-pkgs from the
`quazardous` fork. Microsoft's validation pipeline and moderators then
merge it, usually within a few days. Without the `WINGET_TOKEN` secret the
job leaves a notice in the run and does nothing; it never fails the
release (the Release is already published when it runs). Its version of
winget-releaser is bumped by hand, to a reviewed commit.

### Setting it up (once, by the owner)

1. Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)
   under the `quazardous` account (the job opens its pull requests from
   that fork).
2. Create a **classic** personal access token (fine-grained tokens are not
   supported by winget-releaser) with the `public_repo` scope. Adding the
   `workflow` scope lets it sync the fork when winget-pkgs changes its own
   workflows; without it, a sync that fails is done by hand from the
   fork's page ("Sync fork"), then the job rerun.
3. Store it as the repository secret `WINGET_TOKEN` (Settings, Secrets and
   variables, Actions).

### The first submission

winget-releaser updates a package; it cannot create one. The first version
is a manual pull request to microsoft/winget-pkgs adding
`packaging/winget/manifests/q/Quazardous/Helios/0.77.0/` at the same path,
opened by the coordinator once the owner agrees. Until it is merged, the
`winget` job of a new release fails (no previous version to start from);
that failure does not fail the release, and the job is rerun once 0.77.0
is in.

To check a manifest set on Windows before submitting it:

```powershell
winget validate --manifest packaging\winget\manifests\q\Quazardous\Helios\0.77.0
winget install --manifest packaging\winget\manifests\q\Quazardous\Helios\0.77.0
```

## The Flatpak

`io.github.quazardous.Helios-X.Y.Z-x86_64.flatpak` is built by
`packaging/flatpak/build.sh` from the manifest
`packaging/flatpak/io.github.quazardous.Helios.yml`:

- on the freedesktop runtime 26.08, with the SDK's Rust extension
  (`rust-stable`, a Rust at least as new as `rust-toolchain.toml`'s);
- from source, with no network during the build, as Flathub requires: the
  crates are listed with their checksums in `cargo-sources.json`, which
  the script generates from `Cargo.lock` with flatpak-builder-tools' cargo
  generator (pinned by commit and SHA-256) and does not commit, so it
  never drifts from the lock file. Repackaging the release tarball's
  programs would be quicker to build, but Flathub accepts prebuilt
  binaries only for programs whose source is not available, so the
  bundle is built the way Flathub will build it;
- with the desktop entry, the icons and the AppStream metadata
  (`packaging/linux/io.github.quazardous.Helios.metainfo.xml`, also in the
  tarball; `cargo xtask release` adds each release to it).

Install and run it:

```bash
flatpak install --user io.github.quazardous.Helios-X.Y.Z-x86_64.flatpak
flatpak run io.github.quazardous.Helios                     # the window
flatpak run --command=helios io.github.quazardous.Helios --help   # the command line
```

The runtime comes from Flathub (the bundle points to it); add Flathub
first if it is not there:
`flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo`.

What the sandbox lets Helios do, and why:

| Permission | Why |
|---|---|
| `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc` | the window, on Wayland or, without it, X11 |
| `--device=dri` | the GPU, for the window and Vulkan processing |
| `--share=network` | the AI model providers: Ollama on this machine or the network, online APIs. Helios works without them |
| `--filesystem=xdg-pictures`, `/run/media`, `/media` | the photos: the Pictures folder and removable media (cards, disks), read and written (imports, exports, backups) |
| `--filesystem=xdg-config/darktable:ro` and darktable's Flatpak settings, read only | importing a darktable library |
| `--talk-name=org.freedesktop.Notifications` | the desktop's notifications |

Helios picks folders with its own picker, not the desktop's file chooser
portal, so a folder outside these is not seen until it is granted:

```bash
flatpak override --user --filesystem=/path/to/photos io.github.quazardous.Helios
```

Known gaps of the sandbox: the darktable import cannot tell whether a
darktable outside the sandbox still runs (its process is not visible), so
close darktable before importing; a local program set as an AI provider
(segmentation) and the Ollama setup run inside the sandbox, where the
host's programs are not; a library or vault outside the folders above
needs an override. Helios is not on Flathub: Flathub's policy refuses
AI-assisted manifests and agent-made submissions, so the bundle on each
release is the Flatpak channel (see the ROADMAP's "Distribution").

## macOS

The workflow also builds Helios for macOS on Apple silicon (arm64), in
its `macos` job on GitHub's `macos-15` runner. A red `macos` job does not
hold the Release back: the Release then ships without the disk image.

- It builds the same programs as on Linux and Windows for
  `aarch64-apple-darwin`. Helios links no C library of darktable's (its
  processing is ported to Rust), and the C its crates carry (the bundled
  SQLite and the like) builds with the runner's Xcode command line tools:
  no Homebrew package is needed. The programs link only system libraries
  and frameworks; `release.sh` would copy any other library into
  `Contents/Frameworks` and repoint the programs at it. The GPU goes
  through wgpu's Metal backend; without a usable GPU Helios processes on
  the CPU.
- `packaging/macos/release.sh` builds `Helios.app`: `Info.plist`
  (`packaging/macos/Info.plist`, bundle id `io.github.quazardous.Helios`,
  the workspace version), an `.icns` icon made from the Linux icon set,
  every program side by side in `Contents/MacOS` (`helios-gui` is the
  app's executable; Helios finds its other programs and bundled plugins
  beside it), the licences in `Contents/Resources`. It wraps the app in
  `helios-X.Y.Z-macos-arm64.dmg` (with a link to Applications, `COPYING`,
  `THIRD-PARTY-LICENSES.txt` and `README.md`) and writes
  `SHA256SUMS-macos`, which `publish` adds to `SHA256SUMS`.
- `packaging/macos/smoke.sh` mounts the image read-only, checks the app's
  `Info.plist`, icon and signature, then runs from the mounted app, with
  no display session: both programs say their version, `helios` imports a
  picture into a new vault and exports it as a JPEG.

### Signing and notarisation

Without secrets the programs and the app are signed ad hoc only (Apple
silicon runs nothing unsigned): Gatekeeper refuses the first start until
the user confirms it (README.md, "Download"). The job signs with a
Developer ID and notarises when these repository secrets are set; each is
checked, and the step is skipped with a notice when it is missing:

| Secret | What |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | the Developer ID Application certificate and its private key, exported as `.p12`, base64-encoded |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12`'s password |
| `APPLE_NOTARY_KEY_P8` | an App Store Connect API key (`.p8`), base64-encoded, for `notarytool` |
| `APPLE_NOTARY_KEY_ID` | that key's id |
| `APPLE_NOTARY_ISSUER_ID` | that key's issuer id |
| `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID` | instead of the API key: an Apple ID, an app-specific password and the team id |

With the certificate, the code is signed with the hardened runtime and a
secure timestamp; with notary credentials too, the disk image is
submitted to Apple's notary service and the ticket stapled to it. That
path has not run yet (no Developer account): expect a first fix when the
secrets arrive.

The window behaviour follows macOS: the global menu bar, and the app
staying open with no window (docs/design/windows.md).

## Checking a download

Download `SHA256SUMS` beside the files, then:

```bash
sha256sum --ignore-missing -c SHA256SUMS
```

On Windows, in PowerShell, compare the line of `SHA256SUMS` for the zip
or the installer with:

```powershell
(Get-FileHash helios-X.Y.Z-windows-x86_64-setup.exe).Hash.ToLower()
```

Each file downloaded should say `OK` (or match): it is the file the release workflow
built and attached.
