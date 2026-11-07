# Packages a release build of Helios for Windows x86_64, as the release
# workflow does (.github/workflows/release.yml):
#
#   pwsh packaging/windows/release.ps1 VERSION BUILD_DIR OUT_DIR
#
# BUILD_DIR holds the programs built with `cargo build --release`. OUT_DIR
# receives helios-VERSION-windows-x86_64.zip, a folder that runs in place
# (helios-gui.exe, helios.exe...) with COPYING (the GPLv3),
# THIRD-PARTY-LICENSES.txt (the crates built in and their licenses, from
# `cargo deny list`, which must be installed) and README.md, and
# SHA256SUMS-windows, its checksum in `sha256sum` format.
param(
  [Parameter(Mandatory)] [string] $Version,
  [Parameter(Mandatory)] [string] $Build,
  [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$programs = 'helios', 'helios-node', 'helios-mcp', 'helios-gui', 'helios-darktable-library'

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$work = Join-Path ([IO.Path]::GetTempPath()) "helios-package-$PID"
$name = "helios-$Version-windows-x86_64"
$tree = Join-Path $work $name
New-Item -ItemType Directory -Force -Path $tree | Out-Null
try {
  foreach ($p in $programs) {
    Copy-Item (Join-Path $Build "$p.exe") (Join-Path $tree "$p.exe")
  }
  Copy-Item (Join-Path $root 'LICENSE') (Join-Path $tree 'COPYING')
  Copy-Item (Join-Path $root 'README.md') (Join-Path $tree 'README.md')
  Push-Location $root
  try {
    $crates = cargo deny --exclude xtask list --layout license --format human
    if ($LASTEXITCODE -ne 0) { throw "cargo deny list failed" }
  } finally {
    Pop-Location
  }
  $licenses = @(
    "Helios $Version is free software under the GNU General Public License,"
    'version 3 or later (COPYING). It is built with the Rust crates below,'
    'listed by license (from `cargo deny list`); each crate''s own license'
    'text comes with its sources on crates.io.'
    ''
  ) + $crates
  # LF line ends, as on Linux.
  [IO.File]::WriteAllText((Join-Path $tree 'THIRD-PARTY-LICENSES.txt'), (($licenses -join "`n") + "`n"))

  $zip = Join-Path $Out "$name.zip"
  if (Test-Path $zip) { Remove-Item $zip }
  Compress-Archive -Path $tree -DestinationPath $zip
  $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
  [IO.File]::WriteAllText((Join-Path $Out 'SHA256SUMS-windows'), "$hash  $name.zip`n")
  Get-Content (Join-Path $Out 'SHA256SUMS-windows')
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
