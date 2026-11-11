# Smoke test of a Windows release zip, run by the release workflow on a
# Windows runner (no GPU):
#
#   pwsh packaging/windows/smoke.ps1 helios-VERSION-windows-x86_64.zip PICTURE.jpg
#   pwsh packaging/windows/smoke.ps1 INSTALLED_FOLDER PICTURE.jpg
#
# Unpacks the zip (or takes the folder an installer filled), then runs the command line: its version, an import of the
# picture into a new vault, and a batch that exports it as a JPEG through
# a network with one module (the processing runs on the CPU). The window's
# program says its version. Mirrors packaging/linux/smoke.sh.
param(
  [Parameter(Mandatory)] [string] $Zip,
  [Parameter(Mandatory)] [string] $Picture
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Check([string] $what) {
  if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

$work = Join-Path ([IO.Path]::GetTempPath()) "helios-smoke-$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
  if (Test-Path -PathType Container $Zip) {
    $bin = (Resolve-Path $Zip).Path
  } else {
    Expand-Archive -Path $Zip -DestinationPath $work
    $bin = (Get-ChildItem -Directory (Join-Path $work 'helios-*')).FullName
  }
  $helios = Join-Path $bin 'helios.exe'

  Write-Output '== files'
  Get-ChildItem $bin | ForEach-Object { "$($_.Name) $($_.Length)" }

  Write-Output '== version'
  $v = & $helios --version; Check 'helios --version'
  Write-Output $v
  if ($v -notmatch '^helios [0-9]') { throw "unexpected version: $v" }
  $v = & (Join-Path $bin 'helios-gui.exe') --version; Check 'helios-gui --version'
  Write-Output $v
  if ($v -notmatch '^helios-gui [0-9]') { throw "unexpected version: $v" }

  Write-Output '== import'
  $photos = Join-Path $work 'photos'
  New-Item -ItemType Directory -Path $photos | Out-Null
  Copy-Item $Picture $photos
  $import = & $helios import --vault (Join-Path $work 'library.helios') $photos; Check 'helios import'
  $import | Write-Output
  if (-not ($import -match '"report"')) { throw 'the import gave no report' }

  Write-Output '== export'
  $network = Join-Path $work 'network.json'
  [IO.File]::WriteAllText($network, @'
{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}
'@)
  $leaf = Split-Path -Leaf $Picture
  $outDir = Join-Path $work 'out'
  & $helios batch --network $network --out $outDir --format jpeg (Join-Path $photos $leaf) | Write-Output
  Check 'helios batch'
  $out = Join-Path $outDir ([IO.Path]::GetFileNameWithoutExtension($leaf) + '.jpg')
  if (-not (Test-Path $out) -or (Get-Item $out).Length -eq 0) { throw "no export at $out" }
  # A JPEG starts with FF D8 FF.
  $magic = [IO.File]::ReadAllBytes($out)[0..2]
  if ($magic[0] -ne 0xFF -or $magic[1] -ne 0xD8 -or $magic[2] -ne 0xFF) { throw "$out is not a JPEG" }
  Write-Output "exported $((Get-Item $out).Length) bytes to $(Split-Path -Leaf $out)"
  Write-Output 'smoke test passed'
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
