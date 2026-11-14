<p align="center"><img src="docs/brand/logo.svg" alt="Helios logo: an iris diaphragm whose opening is a sun" width="128"></p>

# Helios

![Status: alpha, under construction](https://img.shields.io/badge/status-alpha%20%E2%80%94%20under%20construction-orange)

> [!WARNING]
> 🚧 **Under construction: alpha.** Helios changes every day, its formats
> and settings may change without notice, and features are missing or
> unfinished. Try it on copies of your photos, keep your own backups, and
> expect bugs. Bug reports and ideas are welcome.

> A client/server, AI-first image processing framework. Its first
> application is photo editing: the quality of darktable without having to
> learn darktable.

Helios builds on [darktable](https://www.darktable.org/)'s image processing
and turns it into a set of normalized, self-describing layers: a catalog
exposed as an API (the vault), a kernel that schedules work on any node
(laptop, GPU workstation, NAS, cloud), and several surfaces on top of a
single engine, including one where an AI assembles a small, pre-calibrated
workshop from an artistic intention.

The desktop interface is built on
[egui-pop](https://github.com/quazardous/egui-pop), an application framework
for [egui](https://github.com/emilk/egui) developed alongside Helios.

## Download

Releases are published on the
[Releases page](https://github.com/quazardous/helios/releases), each with
its changelog. For Linux on x86_64:

- `Helios-<version>-x86_64.AppImage`: one file. Make it executable
  (`chmod +x`) and run it: it opens the window; `Helios-<version>-x86_64.AppImage
  helios ...` runs the command line.
- `helios-<version>-linux-x86_64.tar.gz`: a folder that runs where it is
  unpacked: `bin/helios-gui` (the window), `bin/helios` (the command line),
  with a desktop entry and icons under `share/`.
- `io.github.quazardous.Helios-<version>-x86_64.flatpak`: a Flatpak
  bundle, sandboxed: `flatpak install --user` it, then
  `flatpak run io.github.quazardous.Helios`. It sees the Pictures folder
  and removable media; see [docs/releases.md](./docs/releases.md#the-flatpak)
  for its permissions and how to grant another folder.
- `SHA256SUMS`: the checksums of every file (see [docs/releases.md](./docs/releases.md)).

Requirements: Linux x86_64 with glibc 2.35 or newer (Ubuntu 22.04, Debian
12, Fedora 36 and later), a Wayland or X11 desktop for the window. A GPU
with a Vulkan driver speeds the processing up; without one it runs on the
CPU.

For Windows on x86_64:

- `helios-<version>-windows-x86_64-setup.exe`: an installer, for the
  current user only (no administrator needed). It installs into
  `%LOCALAPPDATA%\Programs\Helios`, adds Helios to the Start menu, and
  uninstalls from Settings, Apps.
- `helios-<version>-windows-x86_64.zip`: the same programs as a folder that
  runs where it is unpacked: `helios-gui.exe` (the window), `helios.exe`
  (the command line).

It needs Windows 10 or 11 (64-bit); a GPU with a Vulkan or DirectX 12
driver speeds the processing up, without one it runs on the CPU. The
installer and the programs are not signed yet, so SmartScreen warns on
first start ("More info", then "Run anyway"): see
[docs/windows-port.md](./docs/windows-port.md) for what Windows still
lacks.

For macOS on Apple silicon (arm64):

- `helios-<version>-macos-arm64.dmg`: a disk image holding `Helios.app`.
  Open it and drag Helios onto Applications. The command line is inside
  the app: `/Applications/Helios.app/Contents/MacOS/helios`.

It needs macOS 11 or newer on an Apple silicon Mac; the GPU goes through
Metal. The app is not signed with a Developer ID nor notarised yet, so
macOS refuses the first start ("cannot be opened" or "damaged"): right-click
Helios in Applications, choose Open, then Open again (on macOS 15 and later,
open System Settings, Privacy & Security, and choose "Open Anyway"). Or
clear the quarantine flag in a terminal:

```bash
xattr -dr com.apple.quarantine /Applications/Helios.app
```

Building from source: see [CONTRIBUTING.md](./CONTRIBUTING.md).

## Learn more

- [ROADMAP.md](./ROADMAP.md): vision, concepts and phases.
- [docs/index.md](./docs/index.md): the design and research documents.
- [docs/crates.md](./docs/crates.md): how the code is split into crates and modules, and the tools.
- [CONTRIBUTING.md](./CONTRIBUTING.md): building from source, testing, contributing.

## License

GNU General Public License v3.0. See [LICENSE](./LICENSE). Helios links with
darktable, which is GPLv3.

The modules ported from [vkdt](https://github.com/hanatos/vkdt)
(`modules/vkdt`) are also under vkdt's BSD-2-Clause licence: its copyright
notice and licence are in [modules/vkdt/LICENSE-vkdt](./modules/vkdt/LICENSE-vkdt),
and each ported file names the vkdt sources it comes from.

The default themes, Helios Dark and Helios Light, take their colours and
sizes from the design tokens of the [Rerun](https://github.com/rerun-io/rerun)
viewer's `re_ui` crate (MIT OR Apache-2.0, Copyright (c) Rerun Technologies
AB), through the Rerun-like kit of egui-pop; Helios is not affiliated with
or endorsed by Rerun.
