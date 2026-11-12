# Smoke test of a Windows release installer, run by the release workflow on
# a Windows runner (no GPU):
#
#   pwsh packaging/windows/smoke-installer.ps1 helios-VERSION-windows-x86_64-setup.exe PICTURE.jpg
#
# Installs it silently, per user, into a temporary folder; checks the
# programs, the licences, the Start menu entry and the uninstaller's entry
# in Settings; runs packaging/windows/smoke.ps1 on the installed folder
# (versions, an import, an export on the CPU); then uninstalls silently and
# checks that the folder, the Start menu entry and the uninstaller's entry
# are gone.
param(
  [Parameter(Mandatory)] [string] $Setup,
  [Parameter(Mandatory)] [string] $Picture
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# The installer's AppId (packaging/windows/helios.iss): its uninstall key.
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{833F5226-CF08-47A0-BF08-0B2A9389FFC3}_is1'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Helios.lnk'

function Run([string] $exe, [string[]] $arguments) {
  $p = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru
  if ($p.ExitCode -ne 0) { throw "$exe exited with $($p.ExitCode)" }
}

$work = Join-Path ([IO.Path]::GetTempPath()) "helios-setup-smoke-$PID"
$dir = Join-Path $work 'Helios'
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
  Write-Output '== install'
  $log = Join-Path $work 'install.log'
  try {
    Run (Resolve-Path $Setup).Path @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`"")
  } catch {
    if (Test-Path $log) { Get-Content $log | Write-Output }
    throw
  }
  foreach ($f in 'helios.exe', 'helios-gui.exe', 'helios-node.exe', 'helios-mcp.exe',
    'helios-darktable-library.exe', 'helios-lightroom-catalog.exe', 'COPYING', 'THIRD-PARTY-LICENSES.txt', 'README.md', 'unins000.exe') {
    if (-not (Test-Path (Join-Path $dir $f))) { throw "not installed: $f" }
  }
  if (-not (Test-Path $shortcut)) { throw "no Start menu entry at $shortcut" }
  if (-not (Test-Path $uninstallKey)) { throw 'no uninstall entry' }
  $entry = Get-ItemProperty $uninstallKey
  Write-Output "installed: $($entry.DisplayName) in $dir"

  & (Join-Path $PSScriptRoot 'smoke.ps1') $dir $Picture

  Write-Output '== uninstall'
  Run (Join-Path $dir 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
  # The uninstaller hands over to a copy of itself in the temporary folder:
  # give it time to finish.
  $deadline = (Get-Date).AddSeconds(60)
  while ((Test-Path (Join-Path $dir 'helios.exe')) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
  if (Test-Path $dir) {
    $left = @(Get-ChildItem -Recurse $dir)
    if ($left.Count -gt 0) { throw "left after uninstall: $($left.Name -join ', ')" }
  }
  if (Test-Path $shortcut) { throw 'the Start menu entry is left after uninstall' }
  if (Test-Path $uninstallKey) { throw 'the uninstall entry is left after uninstall' }
  Write-Output 'installer smoke test passed'
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
