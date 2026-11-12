# Releases

Each minor or major release of Helios is a tag `vX.Y.0` on `main` and a
GitHub Release of the same name on the
[Releases page](https://github.com/quazardous/helios/releases).

## Where they come from

A release is prepared on the bench box from `next`, merged into `next`
as the commit "Release X.Y.Z" through a pull request, and reaches `main`
when `promote.yml` promotes a green `next` (see CONTRIBUTING.md,
"Versions and releases"). Only a minor or major release is tagged: the
tag `vX.Y.0`, pushed on that commit on `main`, starts the release
workflow (`.github/workflows/release.yml`) on GitHub Actions, which:

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
6. builds the Flatpak (`packaging/flatpak/build.sh`, on Ubuntu 24.04)
   from the same source, as a single-file bundle
   `io.github.quazardous.Helios-X.Y.Z-x86_64.flatpak`, and tests it
   (`packaging/flatpak/smoke.sh`): installed for the user, the command line
   in the sandbox says its version, imports a picture and exports it on
   the CPU, the window's program says its version (see "The Flatpak"
   below);
7. creates the GitHub Release, titled "Helios X.Y.Z", with that version's
   CHANGELOG.md section as its notes, and attaches the files, with one
   `SHA256SUMS` for all of them.

Nothing else is needed at run time: the catalogues, templates, module
declarations and the shipped plugins' manifests are compiled into the
programs. The plugins' programs sit beside `helios-gui`, which writes their
folders into its cache on first use. Vulkan comes from the system: without
a loader or a driver, Helios processes on the CPU.

The workflow also runs by hand (Actions, "release", "Run workflow"), with
a tag and `publish`. With `publish` off, the default, it is a dry run: it
builds, packages and tests, and keeps the files as the run's artefacts,
without creating a Release. With `publish` on, it builds the tag given,
which must exist and match the workspace version.

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
needs an override. Helios is not on Flathub yet: see
[flathub.md](./flathub.md) for what submitting it takes.

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
