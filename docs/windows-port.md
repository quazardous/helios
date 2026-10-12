# Porting Helios to Windows: cost, blockers, and what to do now

Status: study, 2026-10-12. Nothing was built for Windows. Facts come from
reading the code at commit `a44135c` (Helios) and `35f9ffb` (egui-pop, the
pinned revision), from `cargo tree --target x86_64-pc-windows-msvc`
(metadata only), and from the sources cited inline. Claims marked
*[verify]* come from general knowledge and should be checked on a real
Windows machine before anyone relies on them.

Paths are relative to the Helios repository unless they start with
`egui-pop/` (the egui-pop repository) or `darktable/` (the darktable fork).

---

## 0. Summary

Porting is cheap today, and the price will grow. The Rust dependency tree
already builds on Windows. Every C dependency is compiled by `cc` and works
with MSVC. No crate is Unix-only on a Windows target. GPU, windowing, file
dialogs and accessibility all have Windows backends inside the crates
already locked.

Helios's own code holds about **25 Unix-only spots in 12 files**. They fall
into five themes:

1. **Local IPC.** Unix sockets, `0600`/`0700` modes, `process_group`,
   `ps`/`kill`, `signal-hook`.
2. **Directories.** `HOME` and `XDG_*` are read by hand in 7 places.
3. **Child processes.** No console suppression, `sh -c` in tests,
   `python3` in plugin manifests.
4. **The GUI shell.** No `windows_subsystem`, an undecorated custom frame,
   egui-pop's `reg query`.
5. **Tests.** 10 test files are `#![cfg(unix)]`.

Three things block a usable Windows build:

- **`helios batch` does not work on Windows at all.** The batch endpoint
  refuses TCP (`crates/helios-node/src/main.rs:557`), and the sandbox it
  starts creates Unix sockets only (`crates/helios-node/src/batch.rs:234`).
- **Sandboxes rely on `ps`, `kill` and Unix socket paths**
  (`crates/helios-node/src/sandbox.rs`).
- **The GUI is a console-subsystem executable.** Child processes would
  flash console windows, including egui-pop's focus-mode probe
  (`egui-pop/crates/egui-pop-core/src/focus_mode.rs:155`).

The cheapest durable fix:

- add one small platform crate;
- add a Windows CI job that is allowed to fail at first;
- add a few `clippy.toml` bans so new code cannot read `HOME` or spawn
  processes on its own.

---

## 1. Inventory of platform-specific code

### 1.1 Local IPC: endpoints, tokens, private files

| Location | What it does | Windows equivalent | Portable crate (licence) |
|---|---|---|---|
| `crates/helios-inspect/src/transport.rs:124-150` | `Listen::Unix` binds a `UnixListener`. On non-Unix it returns `Unsupported` and points to TCP. | Use the loopback TCP + session token transport that already exists. Later, use named pipes with a per-user security descriptor. | `interprocess` local sockets: named pipes on Windows, UDS on Unix (MIT OR Apache-2.0 *[verify]*). `uds_windows` (MIT, already in the lock via zbus) gives AF_UNIX on Windows 10 1803+. |
| `transport.rs:222-232` | `request_at` with `Address::Unix` | as above | as above |
| `transport.rs:104-113` | `write_token` opens the token file with mode `0600`; Windows gets the default ACL. | A file under `%LOCALAPPDATA%` inherits the profile's per-user ACL, which is private in practice. For a hard guarantee, set an explicit DACL (owner only). | `windows-sys` (MIT/Apache) behind one audited `unsafe`, or write the file only into a per-user directory. |
| `crates/helios-inspect/src/lib.rs:164-197` | `serve`, `serve_lines` and `request` are `#[cfg(unix)]` shortcuts | Callers should use `serve_at`/`request_at` with `Listen`/`Address` | n/a |
| `crates/helios-node/src/main.rs:552-578` | The **batch endpoint** accepts only `Listen::Unix`, then `chmod 0600`. | Accept TCP + token, as the inspect and control endpoints do. | n/a |
| `crates/helios-node/src/sandbox.rs:20, 51-56, 76-91` | The runtime dir comes from `XDG_RUNTIME_DIR` (else temp). The 100-byte Unix path limit is checked. | Use `%LOCALAPPDATA%\Helios\run\<hash>` (or a pipe name `\\.\pipe\helios-<user>-<hash>`). Windows has no 108-byte limit for pipes. | `directories` (MIT/Apache) |
| `sandbox.rs:134-144` | `private_dir` sets `0700`, which also proves ownership | Directory DACL; ownership check through `GetSecurityInfo` | `windows-sys`, or skip and rely on the per-user profile dir |
| `sandbox.rs:108-118` | `is_our_node` runs `ps -p PID -o args=` | Better on every OS: ask the node itself. Connect to its endpoint and read an identity, such as `describe()` or a nonce written to `sandbox.json`. | `sysinfo` (MIT) as a fallback |
| `sandbox.rs:175-178` | `process_group(0)`, so the node ignores the terminal's Ctrl-C | `CommandExt::creation_flags(CREATE_NEW_PROCESS_GROUP \| CREATE_NO_WINDOW)` (a safe std API). Add `DETACHED_PROCESS` if the node must outlive its console. | std only |
| `sandbox.rs:216-219` | `stop` runs `kill PID` (SIGTERM, graceful) | No SIGTERM exists. Send a `shutdown` request over the endpoint (portable and graceful); `TerminateProcess` stays the last resort. | n/a |
| `crates/helios-node/src/bin/helios.rs:387-393` | `signal-hook` SIGINT/SIGTERM sets the `interrupted` flag (batch cancels, exit 130). On Windows nothing is registered, so Ctrl-C kills `helios` outright and leaves the node and its temp files behind. | `SetConsoleCtrlHandler` (CTRL_C, CTRL_BREAK, CTRL_CLOSE) | `ctrlc` with the `termination` feature (MIT/Apache) covers SIGINT, SIGTERM and SIGHUP plus the Windows console events in one API, and could replace `signal-hook`. |
| `crates/helios-node/Cargo.toml` | `signal-hook` under `[target.'cfg(unix)'.dependencies]` | Already gated correctly; it does not appear in the Windows tree | n/a |
| `crates/helios-node/src/batch.rs:219-240` | `own_node` builds `Address::Unix` for its sandbox | Pick the transport per OS (see §7) | n/a |

The transport layer is already split well. `Listen` and `Address` are
enums, and TCP with a token works on every OS (the TCP transport and its
test, `crates/helios-node/tests/tcp.rs`). What is missing is that the
callers (sandbox, batch, batch endpoint) still hard-code `Unix`.

**Process lifetime.** A node started by `helios batch`, or a GUI-owned
node in the future, must die with its parent if the parent crashes. On
Linux this is not handled either (no `PR_SET_PDEATHSIG`). On Windows the
tool is a **Job Object** with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
`win32job` (MIT/Apache *[verify]*) wraps it; otherwise use `windows-sys`
with one `unsafe` block. Plugins (`crates/helios-plugin/src/rpc.rs:79`) need
the same, so that a plugin's own children (exiftool, gphoto2) die with it.

### 1.2 Directories (`HOME`, `XDG_*`)

The configuration directory is computed by hand in **seven places**, all
with the same snippet:

| Location | Use |
|---|---|
| `crates/helios-gui/src/main.rs:17-19` | per-workspace layout file |
| `crates/helios-gui/src/shell.rs:903-906` | `theme.export` default path |
| `crates/helios-gui/src/shell.rs:2437-2442` | `config_dir()` (recent workspaces) |
| `crates/helios-gui/src/shell.rs:2376, 2389` | `default_path`: `$HOME/Pictures/Helios` for snapshots and recordings |
| `crates/helios-gui/src/dialogs.rs:342-356` | `~` expansion (`/` only) and `home()`, which falls back to `/` |
| `crates/helios-compute/src/settings.rs:16-22` | machine settings directory |
| `crates/helios-plugin/src/broker.rs:63-75` | plugins directory |

`HOME` is usually unset on Windows outside MSYS/Git Bash. The code then
falls back to `.` (the current directory), so configuration would silently
land wherever the program was started. Replace all seven with one
function in a platform crate built on `directories::ProjectDirs` (MIT/Apache):

- `config_dir()` maps to `%APPDATA%\Helios` (roaming), which suits
  user-level settings and themes;
- machine settings and caches map to `%LOCALAPPDATA%\Helios`, which is
  **never roaming** and matches the "never synced" rule for `machine.toml`
  in `docs/gui.md`;
- the runtime directory maps to `%LOCALAPPDATA%\Helios\run`;
- pictures map to `UserDirs::picture_dir()` (the known folder
  FOLDERID_Pictures). It is often redirected to OneDrive, so do not build
  it from `HOME`.

Keep honouring `XDG_CONFIG_HOME` on Linux so tests and sandboxes still work.
In `dialogs.rs`, `~` expansion must accept `~\` and `~/` on Windows. The
root fallback should become the drive list (or `current_dir`), not `/`.

### 1.3 Paths

- **`canonicalize()` returns verbatim paths on Windows** (`\\?\C:\...`).
  It is used in `crates/helios-node/src/sandbox.rs:75,148,209,239`,
  `crates/helios-node/src/main.rs:666`, `crates/helios-gui/src/main.rs:223`,
  `crates/helios-plugin/src/broker.rs:630,750,802,810` and
  `crates/helios-i18n/build.rs:32`. Verbatim paths:
  - break comparisons with non-canonical paths (`broker.rs:812` compares a
    parent with a canonical staging dir; both sides are canonical, so this
    one is safe);
  - leak into JSON answers, the vault and `sandbox.json`;
  - are rejected by many external tools (exiftool, ffmpeg, cmd).

  Fix: use `dunce::canonicalize` everywhere (CC0-1.0 OR MIT-0 OR
  Apache-2.0, accepted by `deny.toml`). It strips `\\?\` when that is safe.
  Ban `std::fs::canonicalize` and `Path::canonicalize` in `clippy.toml`
  (§7.1).
- **Paths as text.**
  - `crates/helios-vault-sqlite/src/lib.rs:596,631` stores
    `path.to_string_lossy()`.
  - `crates/helios-node/src/sandbox.rs:54` hashes the lossy string.

  Windows paths are UTF-16 and may hold unpaired surrogates (rare). More
  important, they are **case-insensitive and accept both separators**. A
  vault that looks a location up by exact string can miss
  `C:\Photos\a.CR2` against `c:/photos/A.cr2`. Define one normalised key
  (separator, drive-letter case, no verbatim prefix) for location lookup
  now. Content hash identity is unaffected.
- **Hard-coded separators.**
  - `crates/helios-photo/src/cfa.rs:76` (`join("/")`) is a CFA pattern
    label, not a path: fine.
  - Tests use `/nonexistent/helios.sock` (`crates/helios-node/tests/system.rs:153`)
    and `/tmp/...` in docs only.
- **Non-ASCII and long paths.**
  - Release 3.4 of darktable crashed for profiles such as
    `C:\Users\Börje` (darktable-org/darktable#8782, cited in
    `docs/darktable-pitfalls/ui-platforms.md` §12).
  - Rust uses wide APIs and `\\?\` internally for long paths, so std is
    safe. External tools are not.
  - Declare `longPathAware` in the application manifest, and run CI with a
    non-ASCII user name and a space in the path (§6).
- **Atomic replace.** `write_atomic` (`crates/helios-export/src/lib.rs:141`),
  `crates/helios-vault-sqlite/src/snapshot.rs:90`,
  `crates/helios-compute/src/settings.rs:148` and `broker.rs:821` all rely
  on `rename` over an existing file. On Windows, `std::fs::rename` uses
  `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`. That works, but it **fails with
  a sharing violation** when another process holds the target open (an
  image viewer, Explorer's thumbnailer, antivirus, the search indexer).
  Add a short bounded retry (for example 5 times, 20-100 ms) on
  `ERROR_ACCESS_DENIED` and `ERROR_SHARING_VIOLATION`, inside one helper.
  `snapshot.rs:91-95` already tolerates the directory-fsync gap on Windows.
- **Deleting open files.** `sandbox::clean` calls `remove_dir_all` on the
  runtime and sandbox directories. On Windows this fails if the node still
  holds `node.log` or a socket. That is one more reason to stop the node
  through the endpoint and wait for it before cleaning.
- **Line endings.** Helios has **no `.gitattributes`** (egui-pop only marks
  `*.png binary`). With Git for Windows' default `core.autocrlf=true`, text
  fixtures (JSON networks, TOML, XMP, Fluent catalogues, WGSL through
  `include_str!` in 10 files) would check out with CRLF. Byte-exact
  goldens and content hashes of text inputs would then differ. Add
  `* text=auto eol=lf` plus `*.png binary` now: it costs nothing.

### 1.4 Child processes

| Location | Program | Windows issue |
|---|---|---|
| `crates/helios-gui/src/shell.rs:1773-1779` | `current_exe()` re-launched for "open workspace" (one window per workspace) | Fine once the GUI is a windows-subsystem binary. Pass `CREATE_NO_WINDOW` anyway. |
| `crates/helios-gui/src/snapshot.rs:93` | `ffmpeg` for recordings | Without `CREATE_NO_WINDOW`, a GUI process that starts a console program flashes a console window and may steal focus (darktable-org/darktable#17193). |
| `egui-pop/crates/egui-pop-core/src/focus_mode.rs:155-165, 204-212` | `reg query HKCU\Control Panel\Desktop /v UserPreferencesMask` at start | Same flash, at every launch. Read the value in-process instead: `SystemParametersInfoW(SPI_GETACTIVEWINDOWTRACKING)` through `windows-sys`. egui-pop has `unsafe_code = "forbid"`, so `windows-registry` (MIT/Apache, safe API) can read `UserPreferencesMask` directly. At minimum, set `creation_flags(CREATE_NO_WINDOW)`. |
| `crates/helios-plugin/src/rpc.rs:79` | plugins (any executable, JSON-RPC over stdio) | `CREATE_NO_WINDOW`, a job object, no shell. |
| `plugins/examples/exif-exiftool/helios-plugin.toml:5`, `crates/helios-plugin/tests/fixtures/testee/helios-plugin.toml:5` | `command = ["python3", ...]` | On Windows `python3` often resolves to the Microsoft Store "app execution alias" stub, which opens the Store or exits with code 9009. Allow per-OS commands in the manifest (`[command.windows] = ["py", "-3", ...]`), or resolve `python` / `py -3`. |
| `plugins/examples/gphoto2-camera` | `gphoto2` | libgphoto2 has no native Windows build (darktable uses the MSYS2 build plus a WinUSB driver through Zadig; see `darktable/packaging/windows/README.md`). Tethering on Windows needs another backend (vendor SDKs, WPD/WIA). Mark the plugin `platforms = ["linux", "macos"]`. |
| `crates/helios-parity/src/lib.rs:188,261`, `crates/helios-parity/src/vkdt.rs:56,87` | `darktable-cli`, `vkdt-cli` | Parity stays a Linux CI job (§4); fine. |
| `crates/helios-node/src/lib.rs:110` | `resolver.command` (user-configured argv) | Fine (no shell). Document that Windows users must give an `.exe` or a `py` launcher. |
| `crates/helios-node/src/sandbox.rs:109,217` | `ps`, `kill` | See §1.1 |
| `xtask/src/bench.rs:35-38` | `target/release/examples/stress_bench` | Add `std::env::consts::EXE_SUFFIX`, as `crates/helios-node/src/bin/helios.rs:302` already does. |

**Rule:** every `Command::new` in non-test code goes through one helper,
`platform::command(program)`. It sets `CREATE_NO_WINDOW` on Windows,
clears inherited handles that should not leak, and optionally attaches the
child to the caller's job object. This is the recommendation of
`docs/darktable-pitfalls/ui-platforms.md` §12, made enforceable.

### 1.5 Memory measurement

`modules/masks/examples/stress_bench.rs:78-83` reads `VmHWM` from
`/proc/self/status`. On other platforms it returns `None` ("`null` where it
is not available"), so it degrades without breaking. The Windows equivalent
is `GetProcessMemoryInfo` → `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize`.
`memory-stats` (MIT/Apache *[verify]*) and `sysinfo` both cover Linux,
macOS and Windows; macOS has no simple peak value. Low priority, but use a
`platform::peak_rss()` so the bench is comparable across OSes.

### 1.6 Focus mode and desktop settings

`egui-pop/crates/egui-pop-core/src/focus_mode.rs` already has a Windows
branch (`UserPreferencesMask`, lowest bit of the first byte = active window
tracking), with a pure parser that is tested. Only the subprocess needs to
change (§1.4). `XDG_CURRENT_DESKTOP` is read unconditionally, but that is
harmless on Windows (empty).

Other "system says" readers will follow: theme, accent colour, reduced
motion, double-click time, caret blink. The Windows sources for them are
`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`,
`UISettings`, `SystemParametersInfo`, `GetDoubleClickTime`. Keep the egui-pop
pattern: a pure parser plus an injectable reader, with an environment
variable override.

### 1.7 Headless GUI testing (wbox)

- wbox (`egui-pop/tools/wbox`, `proto/wbox`) runs the app inside a nested
  headless **Wayland** compositor (labwc). It is Linux-only by construction.
  Its scripts are bash (`app.sh`, `bench.sh`) and Python (`wbox_ctl.py`).
- `egui_kittest` with the wgpu renderer (used by `helios-gui` tests and
  `Shell::headless`, `crates/helios-gui/src/shell.rs:2507-2520`) is
  **portable**. On a Windows runner without a GPU, wgpu finds **WARP**, the
  DX12 software rasteriser, through the DX12 backend. Lavapipe is not
  available there unless Mesa for Windows is installed.
  `egui-pop/kittest.toml` already has a `[windows]` section with looser
  thresholds (1.0 / 2000 pixels).
- On Windows, therefore, kittest snapshots and the DX12/WARP adapter replace
  wbox. Interactive "drive the real window" runs (frame, snap, DPI change,
  IME) have no headless equivalent. They need either a Windows VM or
  runner session with UI automation (UI Automation through AccessKit's
  Windows adapter, which egui already ships: `accesskit_windows` is in the
  lock), or manual checks per release.
- Policy: keep snapshots authoritative on Linux/lavapipe. On Windows/WARP,
  run the same tests with platform thresholds, or skip pixel comparisons
  and assert structure (labels, AccessKit tree).

### 1.8 Shell scripts and tooling

The following are shebang scripts:

- `tools/*.sh` (darktable oracles; they also use `-ldl`, `LD_LIBRARY_PATH`
  and `.so` lookups);
- `tools/pre-commit`;
- `packaging/linux/install.sh`;
- `proto/wbox/*.sh`;
- `plugins/examples/gphoto2-camera/tests/fake-gphoto2.sh`.

These are developer tools tied to Linux workflows; keep them so. The rule
is that **nothing a user or `cargo test` needs is a shell script**.
`xtask` is already the portable entry point.

Tests that use `sh -c` and should get a Rust stub binary or a
`cfg(windows)` variant:

- `crates/helios-node/tests/control.rs:206`;
- `crates/helios-resolve/tests/resolve.rs:281-299`;
- the gphoto2 fake.

### 1.9 Tests gated on Unix

These test files are `#![cfg(unix)]`:

- `crates/helios-inspect/tests/protocol.rs`;
- `crates/helios-node/tests/{system,sandbox,batch,batch_endpoint}.rs`;
- `crates/helios-node/tests/{control,mcp}.rs`, which are
  `cfg(all(unix, debug_assertions))`;
- `plugins/examples/gphoto2-camera/tests/fake_camera.rs`.

These individual tests are gated too:

- `crates/helios-compute/tests/compute.rs:588`;
- `crates/helios-plugin/src/broker.rs:857` (symlinks need a privilege or
  Developer Mode on Windows: keep it Unix-only).

On Windows the whole node system suite would silently not run. Most of it
is transport-agnostic. Parameterise the harness over `Listen` (Unix where
available, TCP + token everywhere) so the suite runs on Windows through
TCP. Keep only the `0600`/`0700` mode assertions Unix-only.

---

## 2. Dependencies

### 2.1 What the Windows target pulls in

`cargo tree --offline --target x86_64-pc-windows-msvc` on `helios-gui`,
`helios-node` and `helios-vault-sqlite`:

| Kind | Crates | Notes |
|---|---|---|
| C via `cc` | `libsqlite3-sys` 0.38 (rusqlite `bundled`), `ring` 0.17 (via rustls / ureq), `blake3` (SIMD C/asm) | All build with MSVC. `vcpkg`/`pkg-config` are ignored when `bundled` is on. `ring` ships pre-assembled objects for x86_64 Windows. aarch64-pc-windows-msvc needs **clang** for ring *[verify]*. |
| Windows system bindings | `windows` 0.62, `windows-sys` 0.52/0.60/0.61, `accesskit_windows`, `glutin_wgl_sys` (from wgpu-hal GLES), `renderdoc-sys` (bindings only) | No link-time surprises beyond the Windows SDK |
| GPU | `wgpu` / `wgpu-hal` 30.0.1 with DX12 (`windows`, `gpu-allocator`), Vulkan (`ash`), GLES/WGL; all compiled in by default | See §3 |
| Windowing | `winit` 0.30.13, `egui-winit`, `accesskit_winit`, `rfd` 0.17 (Win32 `IFileDialog`; the `xdg-portal` feature only matters on Linux) | Native |
| Not present on Windows | `signal-hook`, wayland-*, x11*, zbus/atspi, `libc` (only through portable crates), `egui_glow` | Correctly target-gated |

Pure Rust and portable:

- `rawler` (LGPL-2.1-only; `memmap2` works on Windows);
- `image`, `jxl-oxide`, `moxcms`, `fluent-*`;
- `sys-locale` (Windows: `GetUserDefaultLocaleName`);
- `quick-xml`, `flate2` (via `miniz_oxide`/`zlib-rs`);
- `naga`.

**No Unix-only crate and no system library is required.** That is the
main reason the port is cheap.

### 2.2 Toolchain: MSVC, not GNU

- Use `x86_64-pc-windows-msvc`. It is Rust's tier-1 Windows target, wgpu
  and winit test against it, and the Windows SDK's DX12/DXGI headers and
  import libraries come with it.
- `x86_64-pc-windows-gnu` (MinGW) works for pure Rust but mixes badly with
  MSVC-built system pieces (DXC, debuggers, PDBs, signing tools). Its
  unwinding and C runtime differ from what Windows users' other software
  uses.
- The only reason to pick GNU would be **linking darktable's C code
  in-process**, because darktable builds with MSYS2/UCRT64 GCC (§4).
  Helios does not do that today; keep it that way (§4).
- `rust-toolchain.toml` pins 1.95, which works for both targets.
- Licences of the proposed new crates are all on `deny.toml`'s allow list:

  | Crate | Licence |
  |---|---|
  | `directories` | MIT/Apache |
  | `dunce` | CC0/MIT-0/Apache |
  | `ctrlc` | MIT/Apache |
  | `windows-sys` | MIT/Apache |
  | `windows-registry` | MIT/Apache |
  | `interprocess` | MIT/Apache *[verify]* |
  | `sysinfo` | MIT |
  | `winresource` / `embed-resource` | MIT |
  | `velopack` | MIT |

---

## 3. GPU on Windows

### 3.1 Backend: Vulkan or DX12

ROADMAP §2.1 says "Vulkan on Linux and Windows, Metal on macOS, raw Vulkan
access for vkdt interop". The code does not force Vulkan.
`crates/helios-compute/src/device.rs:140-143` enumerates `Backends::PRIMARY`
(Vulkan, Metal, DX12, WebGPU) then `SECONDARY`. `crates/helios-gpu/src/lib.rs:40`
and egui-wgpu use `InstanceDescriptor::..._from_env` (`WGPU_BACKEND`). On
Windows both Vulkan and DX12 adapters will be listed for the same GPU.

The arguments for keeping **both** and choosing at runtime:

- **CI and VMs have no Vulkan.** GitHub `windows-latest` runners, RDP
  sessions and many VMs only offer WARP (DX12). Vulkan there would need
  Mesa for Windows (lavapipe) or SwiftShader, installed by hand.
- **Windows on ARM.** Qualcomm's Vulkan driver lags behind its DX12 driver
  *[verify current state]*. DX12 is the safe default there.
- **HDR.** DXGI scRGB through a DX12 swapchain is the most travelled Windows
  HDR path. On Vulkan it depends on the vendor exposing
  `VK_COLOR_SPACE_EXTENDED_SRGB_LINEAR_EXT` (NVIDIA and AMD do; Intel
  varies *[verify]*).
- **Same pixels.** Both backends compile WGSL through naga. Compute results
  must match the CPU reference on both: the existing GPU-vs-CPU tests
  already express that.

Two things to watch:

- **Device identity includes the backend** (`"gpu:vulkan:10de:2206"` in
  `crates/helios-compute/tests/compute.rs:584`). A user whose machine
  switches backend (driver update, setting) would lose per-device settings.
  Decide now whether the key is `backend:vendor:device` or
  `vendor:device` with backend as an attribute.
- **Zero-copy GUI ↔ engine** (the ROADMAP's hard requirement) works only if
  the GUI and the engine share **one `wgpu::Device`**. That holds within
  one backend on any OS; nothing about it is Vulkan-specific.

**"Raw Vulkan for vkdt interop" is moot on Windows.** vkdt does not
support Windows: its README lists Linux and macOS (MoltenVK) and an
unfinished Android port, with no Windows build
(https://github.com/hanatos/vkdt). The ROADMAP also decided on 2026-10-11
to **port** vkdt modules to WGSL rather than embed libvkdt. So any
`as_hal::<Vulkan>` code is optional, must stay behind a feature or `cfg`,
and must have a portable fallback (copy). Parity against `vkdt-cli` stays a
Linux-only CI job, which is what `.github/workflows/parity.yml` already does
(Fedora container, lavapipe).

**Proposed ROADMAP amendment (not decided):** "Vulkan or DX12 on Windows,
chosen at runtime (Vulkan preferred when the adapter supports it, DX12 for
WARP, ARM and HDR-only paths). No Vulkan-only feature without a fallback."

### 3.2 GPU watchdog (TDR)

Windows resets a GPU that does not answer within about 2 s (TDR). The
dispatch is lost and the device is often lost too.
`docs/darktable-pitfalls/gpu-perf.md` §12 already flags that `helios-gpu`
waits with `PollType::wait_indefinitely()` and registers no device-lost
callback. On Windows this turns into a frozen or blank viewport after a
long dispatch. Before any Windows beta:

- split long dispatches into tiles (≤ ~100 ms each);
- poll with a timeout;
- register `set_device_lost_callback`;
- recreate the device and retry on the CPU.

### 3.3 HDR and wide gamut: the Windows counterpart of `docs/wayland-colour.md`

`docs/wayland-colour.md` already chose path (a): one wgpu surface in a
declared colour space. It starts with scRGB (`Rgba16Float` +
`SurfaceColorSpace::ExtendedSrgbLinear`, wgpu 30, gfx-rs/wgpu#9658) and
notes "the same code also gives scRGB on Windows/DX12 and EDR on macOS".
On Windows:

- **Surface.**
  - On DX12, `ExtendedSrgbLinear` maps to
    `DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709` with an `R16G16B16A16_FLOAT`
    flip-model swapchain. DWM composites it in scRGB: 1.0 = SDR white,
    values above 1.0 are HDR headroom.
  - `Bt2100Pq` maps to `DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020`
    (HDR10).
  - On Vulkan-on-Windows the same variants map through
    `VK_EXT_swapchain_colorspace`, when the driver exposes them.
- **Display information.** `Surface::display_hdr_info()` (gfx-rs/wgpu#9739)
  returns values on Windows. `docs/wayland-colour.md` notes it returns
  nothing for Vulkan *outside* Windows, which implies DXGI output data on
  Windows. Check that it reports max luminance and the **SDR white level**.
  Otherwise read `DISPLAYCONFIG_SDR_WHITE_LEVEL` through
  `DisplayConfigGetDeviceInfo`, as `docs/darktable-pitfalls/colour.md` §13
  recommends.
- **The SDR-in-HDR trap.** With Windows HDR on, an 8-bit sRGB swapchain is
  composited by DWM with the HDR Calibration / "ICC compatibility" pipeline.
  darktable then showed a blue-green cast (darktable-org/darktable#15967,
  #3619). When HDR is on, Helios should use the scRGB float surface so that
  Helios, not the OS, does the mapping. With HDR off, an sRGB surface plus
  Helios's own display transform is right.
- **Display ICC profile (SDR wide gamut).** Windows does not tag swapchains
  with the monitor's ICC profile; colour-managed apps apply it themselves.
  The per-monitor profile comes from `WcsGetDefaultColorProfile` (or the
  older `GetICMProfileW` on the monitor's DC). It must be re-read when the
  window moves to another monitor (`WM_DISPLAYCHANGE`, winit `Moved` /
  `ScaleFactorChanged`).
  - darktable used the **primary** display's profile for every window on
    Windows (`docs/darktable-pitfalls/colour.md` §2). Helios's auxiliary
    windows on other screens make the per-window profile mandatory.
  - The profile then goes through `moxcms`, as `docs/icc.md` already
    decided.
- **Advanced Color / Auto Color Management (Windows 11 22H2+).** On
  supported SDR wide-gamut displays the OS can colour-manage
  sRGB-or-scRGB swapchains itself (ACM). With ACM on, an scRGB surface is
  correct everywhere and Helios must not apply the ICC profile twice
  *[verify behaviour per build]*.
- **Diagnostics.** The per-window colour diagnostics from `colour.md` §14
  (platform path, surface format, colour space, profile, SDR white) should
  report "Windows: DXGI colour space X, HDR on/off, SDR white N nits,
  ACM on/off, ICC profile P".

Short version: the Wayland work is reusable as is. Windows adds a
per-monitor ICC reader, an HDR/SDR-white probe, and a test matrix of
HDR on/off × ACM on/off × Vulkan/DX12.

---

## 4. The darktable side

**Today, Helios does not link with darktable at all.**

- No `build.rs` links C (the only build script is `crates/helios-i18n/build.rs`).
- There is no `extern "C"`, no `links =`, no `cc::Build`.
- darktable is used:
  - as a **subprocess** for parity (`darktable-cli`, `crates/helios-parity/src/lib.rs:177-195`);
  - by **developer tools** that compile small C programs against a darktable
    tree and `dlopen` its plugins (`tools/dt_param_layouts.sh`,
    `tools/dt_legacy_oracle.sh`: bash, `cc`, `pkg-config glib-2.0`, `-ldl`,
    `libexposure.so`, `LD_LIBRARY_PATH`). Their output (params layouts
    JSON) is committed in `crates/helios-parity/layouts/`. Windows never
    needs to rerun them.

So on Windows, darktable is a **test-time oracle only**, and parity can stay
on Linux CI. The GPLv3 obligation (ROADMAP §1, "mandatory since Helios
links with darktable") is about future plans and code derived from
darktable. It does not by itself impose a C toolchain.

**Phase 0 (the darktable fork, `darktable/doc/helios/PHASE0.md`)** is
purely inside darktable. It is an in-process C catalog SDK replacing direct
SQLite calls, "designed for remote, implemented locally". Later phases plug
a remote backend (the Helios vault) behind it. On Windows:

- **darktable builds only with MSYS2 / UCRT64 GCC** (MinGW-w64, CMake, Ninja,
  OpenMP from `mingw-w64-ucrt-x86_64-omp`, GTK3 and about 40 MSYS2
  packages; installer through NSIS: `darktable/packaging/windows/README.md`).
  There is no MSVC build.
- **If darktable becomes a client of the Helios node** (the "two GUIs on one
  engine" decision), the clean boundary is **IPC, not linking**. The C
  remote backend speaks the node's JSON-lines protocol over loopback TCP +
  token (or a named pipe) from inside darktable's MinGW build. Helios stays
  MSVC; darktable stays MSYS2; no ABI crosses the toolchain boundary.
  Keeping the wire protocol transport-agnostic and documented (it already
  is: `crates/helios-inspect/src/transport.rs`, `docs/cli.md`) is what
  makes this possible.
- **If Helios ever links darktable code in-process** (calling darktable's
  IOPs through a C ABI for exact parity), Windows forces a choice:
  - build Helios with `x86_64-pc-windows-gnu` against MSYS2's darktable
    libraries (MinGW runtime, OpenMP, GLib, and every darktable dependency
    shipped as DLLs); or
  - build darktable's computation core with clang-cl/MSVC, which nobody
    maintains today.

  Both are expensive. Decide on IPC, or a subprocess (`darktable-cli`),
  for anything darktable-side on Windows. *(Proposed, not decided.)*
- **Windows bugs in the fork are mostly not Helios's problem.** darktable's
  own Windows issues (console flashes from Lua, focus stealing, non-ASCII
  profile crashes, OpenCL kernel cache on Windows) are listed in
  `docs/darktable-pitfalls/ui-platforms.md` §12 and `gpu-perf.md` §21 as
  lessons, not dependencies.

---

## 5. GUI and UX on Windows

| Topic | State in Helios | What Windows needs |
|---|---|---|
| **Subsystem** | `crates/helios-gui/src/main.rs` has no `windows_subsystem` attribute, so launching from Explorer opens a console window behind the app | `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`. `helios-gui --snapshot` prints to stdout: in a windows-subsystem build, call `AttachConsole(ATTACH_PARENT_PROCESS)` when started from a terminal, or move headless snapshotting to the `helios` CLI. |
| **DPI** | `pixels_per_point` is read per window (`shell.rs:2060`), with a `hidpi` class at 1.5 | winit enables per-monitor DPI awareness *[verify v2 on winit 0.30.13]*. Also declare `PerMonitorV2` in the application manifest so the process starts DPI-aware before winit runs. Test dragging an auxiliary window between a 100 % and a 150 % monitor: the scale changes mid-drag, and egui layouts must not jump. "100 % = one image pixel per physical pixel" (`ui-platforms.md` §10) already handles the viewport. |
| **Custom frame** | `with_decorations(false)` in `main.rs:268`; `frame.rs` draws the title bar, buttons and resize grips, and uses `ViewportCommand::StartDrag` / `BeginResize`; double-click maximises | **Aero Snap** (drag to an edge or top, Win+arrows) works with winit's drag, which posts `WM_NCLBUTTONDOWN(HTCAPTION)` *[verify]*. Three gaps: (1) the **Windows 11 Snap Layouts flyout** appears only when `WM_NCHITTEST` returns `HTMAXBUTTON` over the maximise button, and winit gives no hook for that; (2) the drop shadow and rounded corners of undecorated windows depend on `DwmExtendFrameIntoClientArea` (winit's `with_undecorated_shadow`); (3) right-clicking the title bar does not open the system menu, and **Alt+Space is both Windows' system-menu key and Helios's `system` menu** (`docs/gui.md` "Menus are declared"). Options, in order of cost: (a) on Windows, default `draw_frame(false)` (the shell already supports it: `shell.rs:594`) and accept the native title bar; (b) subclass the window procedure to answer `WM_NCHITTEST` (unsafe, through `raw-window-handle`), the way Tauri and Electron's "window controls overlay" do; (c) upstream a winit hit-test hook (track in `docs/upstream.md`, per the egui-pop upstream rule). |
| **Taskbar identity** | `with_app_id("helios-gui")` (Wayland only) | Set an AppUserModelID (`SetCurrentProcessExplicitAppUserModelID`, or through the installer shortcut) so all workspace windows group under one icon and jump lists ("recent workspaces") work. Embed the icon and version through a `build.rs` resource (`winresource`). |
| **Console flashes / focus stealing** | see §1.4 | One spawn helper with `CREATE_NO_WINDOW`; never call `SetForegroundWindow` or `ViewportCommand::Focus` from background events (`ui-platforms.md` §12; darktable-org/darktable#20442). Notices never take focus (already decided in `docs/gui.md`). Windows also refuses foreground changes from background processes, which shows as a flashing taskbar button: launching another workspace window from the shell is fine because it follows user input. |
| **Non-ASCII paths** | `PathBuf` end to end in most code; `to_string_lossy` in vault and display | Add a CI user and paths with `é`, `Ö`, CJK and spaces (§6). |
| **Fonts** | egui's embedded fonts (`default_fonts`), no system lookup | Same on every OS, which avoids darktable's Windows font bugs (`ui-platforms.md` §13). CJK, Arabic and other scripts are still missing everywhere. On Windows, a runtime fallback can load Segoe UI, Yu Gothic UI, Microsoft YaHei UI or Malgun Gothic from `%WINDIR%\Fonts` by locale. Load them at runtime; never redistribute them. Make it a theme-declared fallback chain, as the pitfall doc suggests. |
| **File dialogs** | `rfd` 0.17, `xdg-portal` feature, run on its own thread; `HELIOS_NO_NATIVE_DIALOGS` fallback | On Windows `rfd` uses `IFileOpenDialog` (COM). The dialog must be **parented** to the requesting window (`set_parent` with the raw window handle) or it can open behind the app. COM initialisation on the worker thread is handled by rfd *[verify]*. |
| **Shortcuts** | `Mod` = Ctrl on Linux and Windows, Cmd on macOS (`docs/gui.md`); `keys.rs:22` maps `meta` to `mac_cmd` | Reserved on Windows: Alt+F4, Alt+Space, Alt+Tab, Win+anything, Ctrl+Alt+Del, F10 (menu-bar activation), plus Ctrl+Shift+Esc and AltGr (= Ctrl+Alt on many European layouts, so **Ctrl+Alt+key shortcuts collide with AltGr characters** on French and German layouts). darktable recorded three-modifier combos wrongly on Windows (`ui-platforms.md` §6). Add a keymap lint that rejects Ctrl+Alt bindings that produce a character on common layouts. |
| **IME** | egui 0.36 IME through winit (TSF/IMM32 on Windows) | Test Japanese, Chinese and Korean input in text fields and the palette. The candidate window must follow the caret (`egui::Output::ime`). |
| **Mouse wheel and precision touchpads** | `docs/ux-research.md`: the wheel scrolls panels; sliders take it when focused | Windows precision touchpads send pixel deltas, mice send line deltas (120 per notch). Normalise both in egui-pop's input layer (`ui-platforms.md` §7 asks for recorded event replays: add a Windows trace). |
| **Clipboard, drag and drop** | `arboard` in the tree | OLE drag and drop of files from Explorer works in winit (`DroppedFile`). Dragging out to Explorer is not supported by winit. |

---

## 6. CI and release

### 6.1 A Windows CI job

Add a `windows-latest` job to `.github/workflows/ci.yml` (and egui-pop's
`ci.yml`). Start with `continue-on-error: true` and make it required once
it is green.

```yaml
windows:
  runs-on: windows-latest
  continue-on-error: true   # until green, then required
  steps:
    - run: git config --global core.autocrlf false
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@1.95
      with: { components: clippy }
    - uses: Swatinem/rust-cache@v2
    - uses: taiki-e/install-action@v2
      with: { tool: cargo-nextest }
    - run: cargo clippy --workspace --all-targets -- -D warnings
    - run: cargo nextest run --workspace --profile ci
      env: { WGPU_BACKEND: dx12 }   # WARP: no Vulkan on hosted runners
```

What will fail first, in likely order:

1. **Clippy on `cfg(windows)` code**: unused imports and variables in the
   `#[cfg(not(unix))]` arms, dead code behind `cfg(unix)`, such as
   `Listen::Unix` handling.
2. **CRLF**, unless `.gitattributes` lands first (§1.3).
3. **Tests that hard-code Unix behaviour without a gate**:
   - `/nonexistent/...` paths;
   - `sh -c` resolvers (`crates/helios-resolve/tests/resolve.rs` is not
     gated);
   - `python3` plugin fixtures (`crates/helios-plugin/tests/broker.rs:37`
     probes `python3 --version`; on a runner it may resolve to the Store
     stub);
   - `exiftool` probes.
4. **GPU tests on WARP.**
   - Snapshot thresholds (egui-pop's `kittest.toml` `[windows]`).
   - Float differences between WARP/DX12 and the CPU reference. WARP
     is IEEE-conformant but differs in denormals and transcendental
     functions *[verify per test]*.
   - The `helios-gpu` "GPU adapter" step expects the Vulkan wording.
5. **Timing.** Windows process spawn and file I/O are slower (Defender
   scanning `target/`). nextest's 120 s cut-off may trip on raw-decoding
   tests in debug builds. Exclude the workspace from Defender in CI only
   if needed.

Cache: `Swatinem/rust-cache` works on Windows. Expect a first full build
of 15-25 min (wgpu + eframe + rawler) and 5-8 min when warm *[estimate]*.
Disk on hosted runners is enough (about 14 GB free on `D:`).

**Tests that need wbox or a display.** None of the `cargo test` suites
needs wbox (wbox is a manual or agent tool). `egui_kittest` runs offscreen
on WARP. Real-window behaviour (frame, snap, DPI, IME, focus) moves to a
manual release checklist, or to a self-hosted Windows runner with a GPU and
UI Automation later.

**Locale and path stress.** A second, nightly job can run under a user
whose name holds non-ASCII characters (create one with `net user` and
`runas`, or set `USERPROFILE`/`TEMP` to `C:\Users\Börje Ü\...`), plus
`chcp 65001` off. This covers darktable#7489 and #8782.

**Cross-checking from Linux** (`cargo clippy --target x86_64-pc-windows-msvc`
through `cargo-xwin`) works but needs the MSVC CRT and SDK downloaded and a
C compiler for the bundled SQLite. A real `windows-latest` job is simpler.

### 6.2 Packaging

| Format | Tool | Notes |
|---|---|---|
| **MSI** | `cargo-wix` (WiX 3/4), or `cargo-packager` (MIT/Apache; WiX MSI and NSIS from one config, also `.deb`, AppImage and `.dmg`) | Per-user install under `%LOCALAPPDATA%\Programs\Helios` (no admin needed), or per-machine. Ships `helios-gui.exe`, `helios.exe`, `helios-node.exe`, `helios-mcp.exe`, catalogues and icons. |
| **MSIX** | `makeappx` + `signtool`, or cargo-packager *[verify MSIX support]* | Needed for the Microsoft Store. Clean install and uninstall, but packaged apps get a **virtualised `%APPDATA%`** and restricted spawning of out-of-package executables (plugins). Test plugin launching under MSIX before choosing it. |
| **Portable zip** | xtask | Cheapest first deliverable; good for testers. |
| **winget** | a manifest PR to `microsoft/winget-pkgs` (`wingetcreate`, `komac`) pointing to a signed MSI or zip on GitHub Releases | Free. Automatable from the release workflow. |

The cross-platform `cargo-packager` (one config for all three OSes) fits
the "Linux, Windows, macOS" requirement better than three separate tools.

### 6.3 Code signing

Without a signature, SmartScreen shows "Windows protected your PC" until
the binary builds reputation. Since 2024, EV certificates no longer bypass
it instantly *[verify]*. Options:

- **Azure Artifact Signing** (formerly Trusted Signing): **$9.99/month** for
  5,000 signatures (Basic), $99.99 for Premium
  (https://azure.microsoft.com/en-gb/products/artifact-signing). Certificates
  are short-lived and rotate automatically; signing runs from GitHub Actions.
  Eligibility is limited: **individuals only in the USA and Canada,
  organisations in the EU/UK** among others
  (https://devclass.com/2026/01/14/code-signing-windows-apps-may-be-easier-and-more-secure-with-new-azure-artifact-service/).
  A French individual would therefore need a legal entity, such as an
  association or a company with three years of history *[verify]*.
- **SignPath Foundation**: free code signing for qualifying open-source
  projects, from GitHub Actions *[verify eligibility: OSI licence, public
  builds]*. GPLv3 qualifies.
- **Certum "Open Source Code Signing"**: a cheap OV certificate for
  individuals (on the order of tens of euros a year plus a smart card)
  *[verify price]*. It needs a hardware token, which makes CI signing
  awkward (cloud HSM option *[verify]*).
- **Microsoft Store (MSIX)**: the Store signs the package. Registration for
  individual developers became free *[verify]*. This is the cheapest path
  to a trusted install, at the price of MSIX constraints.

The pitfall doc's lesson from macOS (`ui-platforms.md` §16: one volunteer
and an unsigned app got Homebrew to drop darktable) applies here too. Sign
from CI with a **project-owned** identity, never a contributor's personal
certificate.

### 6.4 Auto-update

- `velopack` (MIT; Rust SDK; Windows, macOS and Linux; delta updates;
  per-user install without UAC) is the closest to "Squirrel done right".
- `cargo-packager`'s updater plugin, or plain winget upgrades, are the
  lighter options.
- Updates must not replace `helios-node.exe` while a node or batch is
  running. Version the node and check `compat` at connect.

---

## 7. Plan

### 7.1 Do now (cheap, stops Unix-only code accumulating)

| # | Action | Effort |
|---|---|---|
| 1 | **Add `.gitattributes`**: `* text=auto eol=lf` and `*.png binary` (Helios; egui-pop already marks PNGs, so add the `eol` line there). | 10 min |
| 2 | **Create a `helios-platform` crate** (the only crate allowed `#![allow(unsafe_code)]`, audited) exposing: `config_dir()`, `machine_dir()`, `cache_dir()`, `runtime_dir()`, `pictures_dir()`, `home_dir()` (via `directories`, honouring `XDG_*` on Linux); `canonicalize()` (via `dunce`); `private_dir()` / `write_private()` (0600/0700 on Unix, per-user directory and DACL on Windows later); `command(program)` (`CREATE_NO_WINDOW` on Windows); `detach_group(&mut Command)` (`process_group(0)` or `CREATE_NEW_PROCESS_GROUP`); `on_interrupt(flag)` (`ctrlc` with `termination`); `replace_file(tmp, dst)` (rename with a bounded retry on Windows); `peak_rss()`. Move the seven `HOME`/`XDG` snippets, the `0600`/`0700` blocks and `process_group` into it. Everything outside it becomes `cfg`-free. | ½–1 day |
| 3 | **Ban the escape hatches in `clippy.toml`** (`disallowed-methods`: `std::env::home_dir`, `std::fs::canonicalize`, `std::path::Path::canonicalize`, `std::os::unix::process::CommandExt::process_group`, `std::process::Command::new` with "use helios_platform::command"; allow with a reason in tests and xtask). Add an `xtask check` grep that fails on the literals `"HOME"`, `"XDG_`, `/proc/` and `std::os::unix` outside `helios-platform` and tests. Mirror the focus-mode subprocess rule in egui-pop. | 1–2 h |
| 4 | **Make the node's local transport choose itself**: `Listen::local(name)` returns a Unix socket on Unix and TCP + token (token file in `runtime_dir`) on Windows, until named pipes exist. Use it in `sandbox`, `batch::own_node` and the batch endpoint (accept TCP + token in `serve_batch_control`). This unblocks `helios batch` on Windows. | ½ day |
| 5 | **Stop the node through the protocol**: add a `shutdown` request (batch and control scopes, token-protected). Replace `ps` and `kill` in `sandbox.rs` with an endpoint probe (identity plus nonce in `sandbox.json`) and `shutdown`. This is better on Linux too: no pid reuse race, no `procps` dependency. | ½ day |
| 6 | **Parameterise the node system tests over the transport** so the `#![cfg(unix)]` suites run over TCP everywhere. Keep only the mode checks Unix-only. Replace `sh -c` resolver fixtures with a tiny Rust test binary (`[[bin]]` under `helios-testkit`, or `CARGO_BIN_EXE_*`). | 1 day |
| 7 | **Add a Windows CI job** (§6.1) with `continue-on-error`, in Helios and egui-pop; make it required once green. | 1–2 h, then fixes |
| 8 | **egui-pop focus mode**: read `UserPreferencesMask` with `windows-registry` (safe API) instead of `reg query`, or at least `CREATE_NO_WINDOW`. Note it in egui-pop's changelog. | 1 h |
| 9 | **`windows_subsystem` attribute** on `helios-gui` (release only), plus an application manifest (PerMonitorV2, `longPathAware`, `activeCodePage` UTF-8) and an icon through a `build.rs` (`winresource`). | 2 h |
| 10 | **Plugin manifests** gain an optional per-OS command and a `platforms` list. Validation refuses `python3` alone on Windows with a hint (`py -3`). | 2 h |
| 11 | **ROADMAP**: record the proposed decisions as "not decided": (a) Windows GPU backend chosen at runtime, Vulkan or DX12, with no Vulkan-only feature lacking a fallback; (b) darktable is reached over IPC or a subprocess on Windows, never linked; (c) MSVC toolchain on Windows; (d) device identity key with or without the backend. | 30 min |

Items 1-3 and 7 are the "stop the bleeding" set: about a day in total.
Items 4-6 make the node portable; they are also improvements on Linux.

### 7.2 Port milestones, in order

| M | Milestone | Content | Exit criterion | Rough effort |
|---|---|---|---|---|
| **W0** | Builds and core tests on Windows | §7.1 items 1-7; Windows CI green for every non-GPU crate (core, values, vault-sqlite, xmp, load, export, modules on CPU) | `cargo nextest run` green on `windows-latest`, GPU tests skipped or on WARP | 3–5 days |
| **W1** | Node and CLI on Windows | Local transport (TCP + token), sandbox, batch, MCP over TCP, Ctrl-C (`ctrlc`), job object so nodes die with `helios batch`, `replace_file` retries, `dunce` paths, normalised vault location keys | `helios batch` on Windows exports a folder of raws; interrupting leaves no partial file; node system tests green over TCP | 4–6 days |
| **W2** | GPU compute on Windows | Vulkan and DX12 adapters both tested against the CPU reference (WARP in CI, a real NVIDIA/AMD/Intel machine by hand); TDR-safe dispatch splitting, poll timeout, device-lost recovery (pitfall gpu-perf §12; also benefits Linux); device-key decision | GPU-vs-CPU tests green on DX12/WARP and on one real GPU per vendor | 4–8 days |
| **W3** | GUI usable on Windows | `windows_subsystem`, manifest, AppUserModelID, icon; frame decision (native title bar by default on Windows, or `WM_NCHITTEST` subclassing for Snap Layouts); rfd parenting; Alt+Space and AltGr shortcut audit; IME check; per-monitor DPI moves; spawn helper everywhere; kittest snapshots on WARP with thresholds | Manual checklist passes on Windows 11 at 100 % + 150 % dual monitors | 5–10 days |
| **W4** | Colour on Windows | Per-monitor ICC profile reader (`WcsGetDefaultColorProfile`), HDR/SDR-white probe (`display_hdr_info` or DisplayConfig), scRGB surface when HDR is on (shared with the Wayland path (a) work in egui-wgpu/eframe), diagnostics panel; test matrix HDR on/off × ACM on/off | Neutral grey stays neutral with HDR on; the window on a P3 monitor uses that monitor's profile | 5–8 days (after the Wayland surface work lands) |
| **W5** | Packaging, signing, update | cargo-packager MSI + portable zip in the release workflow; signing (Artifact Signing through an entity, or SignPath); winget manifest; velopack (or winget-only) updates; CI job with a non-ASCII user and paths | A signed MSI installs without admin and passes SmartScreen; winget installs it; an update applies | 4–7 days, plus calendar time for signing eligibility |
| **W6** | Plugins and devices | Per-OS plugin commands, job objects for plugin trees, exiftool on Windows, tethering backend decision (gphoto2 is not native on Windows) | The exiftool plugin works from an MSI install | 2–4 days (tethering excluded) |

Total: about **6–9 weeks of focused work** for a signed, colour-correct
Windows beta. Roughly half of it (W1 partly, W2's TDR work, W4's surface
work) also pays off on Linux. If §7.1 is done now, W0 and W1 shrink to
about a week.

### 7.3 Risks and unknowns to test early (spikes)

1. **egui-wgpu on DX12 with an `Rgba16Float` scRGB surface.** The same
   eframe change as the Wayland plan. Check that DX12 lists
   `ExtendedSrgbLinear` for `Rgba16Float` in `format_capabilities` and that
   egui's linear output path renders correctly. Run it on WARP in CI.
   *Half a day spike, once the eframe patch exists.*
2. **One `wgpu::Device` shared between the GUI and engine compute on
   DX12.** Zero-copy textures and buffers across render and compute
   queues; check synchronisation and performance against Vulkan on the
   same GPU. *One day.*
3. **WARP versus the CPU reference.** Do module tolerances hold on WARP?
   If not, CI either relaxes per backend or skips GPU-vs-CPU on Windows
   CI. *Half a day, comes with W0.*
4. **Snap Layouts with the custom frame.** Prototype `WM_NCHITTEST`
   subclassing through the raw window handle with winit 0.30, or decide on
   the native title bar for Windows. *One day.*
5. **MSIX constraints on plugins** (spawning executables outside the
   package, virtualised AppData). Decides MSI versus MSIX. *Half a day.*
6. **Signing eligibility** for the project (entity, country). It has the
   longest calendar lead; start the paperwork early.
7. **Vulkan on Windows on ARM** (Snapdragon X): does the adapter work with
   wgpu 30 compute shaders, or do we force DX12 there? *Needs hardware.*
8. **Antivirus and indexer interference** with atomic renames and SQLite
   WAL in the vault directory. Stress-test exports into a folder Explorer
   is showing.
9. **Performance of process-per-workspace** on Windows: process start is
   slower, and each workspace re-initialises wgpu, using 1-2 s of DX12 or
   Vulkan setup *[verify]*. If it is too slow, consider one process with
   several windows (winit supports it) while keeping "one window = one
   workspace" as the user-facing rule.

---

## Sources

- Helios code and docs at commit `a44135c`: `ROADMAP.md` §1-2,
  `docs/architecture.md`, `docs/cli.md`, `docs/gui.md`,
  `docs/persistence.md`, `docs/wayland-colour.md`, `docs/icc.md`,
  `docs/darktable-pitfalls/{ui-platforms,gpu-perf,colour}.md`,
  `.github/workflows/{ci,parity}.yml`, `Cargo.toml`, `Cargo.lock`,
  `deny.toml`, and the files cited inline.
- egui-pop at `35f9ffb`: `crates/egui-pop-core/src/focus_mode.rs`,
  `kittest.toml`, `tools/wbox/README.md`, `.github/workflows/ci.yml`.
- darktable fork: `doc/helios/PHASE0.md`, `packaging/windows/README.md`
  (MSYS2/UCRT64 build).
- vkdt platforms: https://github.com/hanatos/vkdt (README: Linux and macOS,
  Android unfinished, no Windows).
- wgpu surface colour spaces: https://github.com/gfx-rs/wgpu/pull/9658,
  https://github.com/gfx-rs/wgpu/pull/9739 (as cited in
  `docs/wayland-colour.md`).
- darktable Windows issues (cited in the pitfall docs): #17193 (console
  flashes), #20442 (focus stealing), #7489 and #8782 (non-ASCII profile
  paths), #15967 and #3619 (HDR colour cast), #13084 (fonts), #3384
  (OpenCL kernel cache).
- Azure Artifact Signing pricing and eligibility:
  https://azure.microsoft.com/en-gb/products/artifact-signing,
  https://devclass.com/2026/01/14/code-signing-windows-apps-may-be-easier-and-more-secure-with-new-azure-artifact-service/
