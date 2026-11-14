#Requires -Version 5.1
<#
.SYNOPSIS
  Build Perch's Windows release artifacts into dist/:
    perch-<version>-windows-x64-setup.exe     per-user NSIS installer
    perch-<version>-windows-x64-portable.zip  perch.exe + README + licenses
  and print their SHA-256.

.PARAMETER Makensis
  Path to makensis.exe (default: ~\tools\nsis\makensis.exe, then PATH).
.PARAMETER NoBuild
  Reuse target\release\perch.exe instead of building.
#>
[CmdletBinding()]
param([string]$Makensis, [switch]$NoBuild)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Perch $version"

if (-not $NoBuild) {
    cargo build --release --locked -p perch
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
$exe = Join-Path $repo 'target\release\perch.exe'
if (-not (Test-Path $exe)) { throw "missing $exe" }

$dist = Join-Path $repo 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null

# --- installer -------------------------------------------------------------
if (-not $Makensis) {
    $candidate = Join-Path $env:USERPROFILE 'tools\nsis\makensis.exe'
    $Makensis = if (Test-Path $candidate) { $candidate } else { (Get-Command makensis -ErrorAction Stop).Source }
}
$setup = Join-Path $dist "perch-$version-windows-x64-setup.exe"
Push-Location (Join-Path $repo 'packaging\windows')
try {
    & $Makensis /V2 "/DVERSION=$version" "/DSRC=$(Split-Path $exe)" "/DOUTFILE=$setup" perch.nsi
    if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
}
finally { Pop-Location }

# --- portable zip ----------------------------------------------------------
$stage = Join-Path $env:TEMP "perch-portable-$version"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item $exe, README.md, LICENSE-MIT, LICENSE-APACHE -Destination $stage
@"
Perch $version (portable)

Run perch.exe. Nothing is installed: settings live in %APPDATA%\perch.
"Start at login" in the tray menu points at this perch.exe, so keep it where
it is (or switch it off before moving the folder).

Windows may show a SmartScreen warning because the binary is not code-signed;
choose "More info" -> "Run anyway". See README.md.
"@ | Set-Content -Path (Join-Path $stage 'README-portable.txt') -Encoding utf8
$zip = Join-Path $dist "perch-$version-windows-x64-portable.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Remove-Item $stage -Recurse -Force

Write-Host ""
foreach ($f in @($setup, $zip)) {
    $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
    "{0}  {1:N2} MB  {2}" -f $h, ((Get-Item $f).Length / 1MB), (Split-Path $f -Leaf)
}
