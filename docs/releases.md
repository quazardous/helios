# Releases

Each release of Helios is a tag `vX.Y.Z` on `main` and a GitHub Release of
the same name on the
[Releases page](https://github.com/quazardous/helios/releases).

## Where they come from

A merge to `main` with user-facing changes is released right after it
lands (see CONTRIBUTING.md, "Versions and releases"): the bench box
prepares the version bump and the CHANGELOG.md section, the commit
"Release X.Y.Z" is tagged `vX.Y.Z` and pushed. The tag starts the release
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
   and zips them (`packaging/windows/release.ps1`):
   `helios-X.Y.Z-windows-x86_64.zip`, a folder that runs where it is
   unpacked, with `COPYING`, `THIRD-PARTY-LICENSES.txt` and `README.md`;
5. tests the zip (`packaging/windows/smoke.ps1`) on a Windows runner,
   which has no GPU: `helios.exe` and `helios-gui.exe` say their version,
   `helios.exe` imports a picture into a new vault and exports it as a
   JPEG on the CPU;
6. creates the GitHub Release, titled "Helios X.Y.Z", with that version's
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

## Checking a download

Download `SHA256SUMS` beside the files, then:

```bash
sha256sum --ignore-missing -c SHA256SUMS
```

On Windows, in PowerShell, compare the line of `SHA256SUMS` for the zip
with:

```powershell
(Get-FileHash helios-X.Y.Z-windows-x86_64.zip).Hash.ToLower()
```

Each file downloaded should say `OK` (or match): it is the file the release workflow
built and attached.
