#Requires -Version 5.1
<#
.SYNOPSIS
  End-to-end SMTC check for Perch's Windows media backend.

.DESCRIPTION
  By default starts `mock_smtc.exe` (crates/platform-windows/examples), which
  creates a genuine System Media Transport Controls session through the
  documented desktop API, then verifies through Perch's status file that:
    - Perch shows the track,
    - `perch play-pause` reaches the player and the pause comes back,
    - resuming works,
    - the player quitting is noticed.

  With -UseRunningPlayer it skips the mock and checks whatever player you have
  open (Spotify, a browser tab, the Media Player app): Perch must show *a*
  track and play-pause must toggle it.

.PARAMETER Exe
  Path to perch.exe (defaults to the release build).
.PARAMETER OutDir
  Where to write results and a screenshot of the expanded notch.
#>
[CmdletBinding()]
param([string]$Exe, [string]$OutDir, [switch]$UseRunningPlayer)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $repo 'target\release\perch.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'artifacts\windows' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Add-Type -AssemblyName System.Drawing

$mockProc = $null
$expected = $null
if (-not $UseRunningPlayer) {
    $mock = Join-Path $repo 'target\release\examples\mock_smtc.exe'
    if (-not (Test-Path $mock)) { throw "build it first: cargo build --release -p perch-platform-windows --example mock_smtc" }
    $mockProc = Start-Process -FilePath $mock -ArgumentList @('"Perch Smoke Song"', '"Perch Test Band"') -PassThru -WindowStyle Hidden
    $expected = 'Perch Test Band - Perch Smoke Song'
    Start-Sleep -Seconds 1
}

$status = Join-Path $env:TEMP 'perch-media-status.json'
Remove-Item $status -ErrorAction SilentlyContinue
$env:PERCH_STATUS_FILE = $status
$env:PERCH_DATA_DIR = Join-Path $env:TEMP 'perch-media-data'
$cfg = Join-Path $env:TEMP 'perch-media-config.toml'
Remove-Item $cfg -ErrorAction SilentlyContinue
Get-Process perch -ErrorAction SilentlyContinue | Stop-Process -Force
$proc = Start-Process -FilePath $Exe -ArgumentList @('--config', $cfg) -PassThru

$results = [ordered]@{}; $fail = @()
function Check($name, $ok, $detail) {
    $results[$name] = @{ ok = [bool]$ok; detail = $detail }
    Write-Host ("[{0}] {1}: {2}" -f ($(if ($ok) { 'PASS' } else { 'FAIL' })), $name, $detail)
    if (-not $ok) { $script:fail += $name }
}
function Status { try { Get-Content $status -Raw | ConvertFrom-Json } catch { $null } }
function WaitFor([scriptblock]$cond, [int]$ms = 6000) {
    $t = [Diagnostics.Stopwatch]::StartNew()
    while ($t.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}
function Perch([string[]]$a) { Start-Process -FilePath $Exe -ArgumentList $a -Wait -WindowStyle Hidden }

try {
    if ($expected) {
        $ok = WaitFor { (Status).track -eq $expected }
    } else {
        $ok = WaitFor { [bool](Status).track }
    }
    Check 'smtc_track' $ok "track='$((Status).track)'"
    $wasPlaying = (Status).playing
    Check 'smtc_playing_known' ($wasPlaying -is [bool]) "playing=$wasPlaying"

    Perch @('play-pause')
    $ok = WaitFor { (Status).playing -eq (-not $wasPlaying) }
    Check 'play_pause_round_trip' $ok "playing $wasPlaying -> $((Status).playing) (command out through SMTC, state back through SMTC)"
    Perch @('play-pause')
    $ok = WaitFor { (Status).playing -eq $wasPlaying }
    Check 'play_pause_again' $ok "playing -> $((Status).playing)"

    Perch @('toggle-notch')
    Start-Sleep -Milliseconds 700
    # Only the notch area: never capture the rest of the user's screen.
    Add-Type -AssemblyName System.Windows.Forms
    $sw = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width
    $bmp = New-Object System.Drawing.Bitmap 560, 220
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen([int]($sw / 2 - 280), 0, 0, 0, (New-Object System.Drawing.Size 560, 220))
    $bmp.Save((Join-Path $OutDir 'perch-media.png')); $g.Dispose(); $bmp.Dispose()
    Perch @('toggle-notch')

    if ($mockProc) {
        Stop-Process -Id $mockProc.Id -Force
        $ok = WaitFor { -not (Status).track } 8000
        Check 'player_quit_reported' $ok "track='$((Status).track)'"
    }
}
finally {
    if ($mockProc -and -not $mockProc.HasExited) { Stop-Process -Id $mockProc.Id -Force }
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}
($results | ConvertTo-Json -Depth 4) | Set-Content (Join-Path $OutDir 'media-results.json') -Encoding utf8
if ($fail.Count) { Write-Error "media smoke FAILED: $($fail -join ', ')"; exit 1 }
Write-Host 'media smoke PASSED'
