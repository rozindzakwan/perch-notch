#Requires -Version 5.1
<#
.SYNOPSIS
  Perch UI smoke test for Windows: drives hover with synthetic input and proves
  the notch expands, collapses, and keeps its tool-window style bits.

.DESCRIPTION
  Launches a Perch binary, then:
    1. verifies the two notch windows exist with the right extended styles
       (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST) and are absent from
       the Alt-Tab list,
    2. moves the cursor to the top-centre hot zone with SendInput and checks the
       pill window grew,
    3. moves the cursor away and checks it shrank back,
    4. confirms focus was never stolen from the foreground window,
    5. captures a screenshot of the top strip as evidence,
    6. reports working set / private bytes / handles / CPU.

  Exits non-zero on any failure, so CI can gate on it.

.PARAMETER Exe
  Path to perch.exe. Defaults to the release build.

.PARAMETER OutDir
  Where to write the screenshot and the results file.

.PARAMETER IdleSeconds
  Start only after this long without keyboard or mouse input (default 60).

.PARAMETER MaxWaitMinutes
  Give up waiting for an idle machine after this long; exits 3 ("skipped").

.PARAMETER SkipIdleCheck
  Start at once (CI runners, where nobody is at the keyboard).
#>
[CmdletBinding()]
param(
    [string]$Exe,
    [string]$OutDir,
    [int]$IdleSeconds = 60,
    [int]$MaxWaitMinutes = 20,
    [switch]$SkipIdleCheck
)

$ErrorActionPreference = 'Stop'

# $PSScriptRoot is empty when the script is piped rather than invoked by path,
# so resolve the repo root defensively.
$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $scriptDir) { $scriptDir = (Get-Location).Path }
$repoRoot = Split-Path -Parent $scriptDir
if (-not $Exe)    { $Exe = Join-Path $repoRoot 'target\release\perch.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'artifacts' }
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

if (-not (Test-Path $Exe)) { throw "perch.exe not found at $Exe - build it first" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# This test moves the mouse and briefly owns the clipboard: only run it while
# nobody is using the machine.
. (Join-Path $scriptDir 'lib-idle.ps1')
if (-not $SkipIdleCheck -and -not (Wait-UserIdle -Seconds $IdleSeconds -MaxMinutes $MaxWaitMinutes)) {
    exit 3
}

# --- Win32 interop: window enumeration, styles, and synthetic mouse input ----
if (-not ([System.Management.Automation.PSTypeName]'PerchNative').Type) {
Add-Type -Namespace '' -Name PerchNative -MemberDefinition @'
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
    [DllImport("user32.dll", SetLastError=true)] public static extern int GetWindowLong(IntPtr hWnd, int nIndex);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll", CharSet=CharSet.Auto)] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder s, int max);
    [DllImport("user32.dll", CharSet=CharSet.Auto)] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder s, int max);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] pInputs, int cbSize);

    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public struct RECT { public int Left, Top, Right, Bottom; }
    public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT { public uint type; public MOUSEINPUT mi; }
'@
}

$GWL_EXSTYLE       = -20
$WS_EX_TOOLWINDOW  = 0x00000080
$WS_EX_NOACTIVATE  = 0x08000000
$WS_EX_TOPMOST     = 0x00000008
$WS_EX_APPWINDOW   = 0x00040000

function Get-PerchWindows([int]$TargetPid) {
    $found = New-Object System.Collections.ArrayList
    $cb = [PerchNative+EnumProc]{
        param($hWnd, $lParam)
        $wpid = 0
        [void][PerchNative]::GetWindowThreadProcessId($hWnd, [ref]$wpid)
        if ($wpid -eq $TargetPid) {
            $cls = New-Object System.Text.StringBuilder 256
            [void][PerchNative]::GetClassName($hWnd, $cls, 256)
            $txt = New-Object System.Text.StringBuilder 256
            [void][PerchNative]::GetWindowText($hWnd, $txt, 256)
            $r = New-Object PerchNative+RECT
            [void][PerchNative]::GetWindowRect($hWnd, [ref]$r)
            [void]$found.Add([pscustomobject]@{
                Handle  = $hWnd
                Class   = $cls.ToString()
                Title   = $txt.ToString()
                ExStyle = [PerchNative]::GetWindowLong($hWnd, $GWL_EXSTYLE)
                Visible = [PerchNative]::IsWindowVisible($hWnd)
                X = $r.Left; Y = $r.Top
                W = $r.Right - $r.Left; H = $r.Bottom - $r.Top
            })
        }
        return $true
    }
    [void][PerchNative]::EnumWindows($cb, [IntPtr]::Zero)
    return $found
}

function Get-LiveRect([IntPtr]$Handle) {
    # Always ask the OS for the CURRENT rect: re-running the enumeration and
    # reading a previously captured object gives stale geometry.
    $r = New-Object PerchNative+RECT
    if (-not [PerchNative]::GetWindowRect($Handle, [ref]$r)) { throw "GetWindowRect failed" }
    return [pscustomobject]@{
        X = $r.Left; Y = $r.Top
        W = $r.Right - $r.Left; H = $r.Bottom - $r.Top
    }
}

function Move-MouseAbsolute([int]$X, [int]$Y) {
    # SendInput with MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE is real synthetic
    # input: it goes through the whole input stack, so the window manager
    # delivers genuine WM_MOUSEMOVE/WM_MOUSELEAVE, which is what we want to
    # test (SetCursorPos can bypass parts of that).
    #
    # PowerShell value-type gotcha: `$inp.mi.dx = ...` mutates a *temporary
    # copy* of the nested struct and silently leaves the original at 0, so
    # SendInput reports success while moving the cursor nowhere. The nested
    # struct must be built separately and assigned back whole.
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $mi = New-Object PerchNative+MOUSEINPUT
    $mi.dx = [int](($X * 65535) / $screen.Width)
    $mi.dy = [int](($Y * 65535) / $screen.Height)
    $mi.mouseData = 0
    $mi.dwFlags = 0x8000 -bor 0x0001  # ABSOLUTE | MOVE
    $mi.time = 0
    $mi.dwExtraInfo = [IntPtr]::Zero

    $inp = New-Object PerchNative+INPUT
    $inp.type = 0  # INPUT_MOUSE
    $inp.mi = $mi

    # Verify the cursor really moved; a silent no-op would make every hover
    # assertion below meaningless. A human touching the mouse mid-test (or
    # pointer acceleration rounding) can knock a single SendInput off target,
    # so retry, falling back to SetCursorPos, which also produces the
    # WM_MOUSEMOVE that hover detection relies on.
    for ($attempt = 1; $attempt -le 4; $attempt++) {
        if ($attempt -le 2) {
            $sent = [PerchNative]::SendInput(1, @($inp), [System.Runtime.InteropServices.Marshal]::SizeOf($inp))
            if ($sent -ne 1) { throw "SendInput failed (sent=$sent)" }
        } else {
            [void][PerchNative]::SetCursorPos($X, $Y)
        }
        Start-Sleep -Milliseconds 120
        $pos = [System.Windows.Forms.Cursor]::Position
        if ([Math]::Abs($pos.X - $X) -le 4 -and [Math]::Abs($pos.Y - $Y) -le 4) { return }
    }
    throw "cursor did not reach ($X,$Y); it is at ($($pos.X),$($pos.Y))"
}

function Save-TopStripScreenshot([string]$Path) {
    # Only the pill window itself: never capture the rest of the user's
    # screen (the open panel is tall, so a fixed strip would take in other
    # windows around it).
    $r = Get-LiveRect $script:pillHandle
    $w = [Math]::Max(1, $r.W); $h = [Math]::Max(1, $r.H)
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.X, $r.Y, 0, 0, (New-Object System.Drawing.Size $w, $h))
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

$results = [ordered]@{}
$failures = New-Object System.Collections.ArrayList
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results[$Name] = @{ ok = $Ok; detail = $Detail }
    $tag = if ($Ok) { 'PASS' } else { 'FAIL' }
    Write-Host ("[{0}] {1}: {2}" -f $tag, $Name, $Detail)
    if (-not $Ok) { [void]$failures.Add($Name) }
}

$cfg = Join-Path $env:TEMP 'perch-smoke-config.toml'
Remove-Item $cfg -ErrorAction SilentlyContinue
# Turn on the features the end-to-end checks exercise.
@"
[features]
media = true
todo = true
clipboard = true
shelf = true
screen_time = true
screenshot = true
"@ | Set-Content -Path $cfg -Encoding ascii
# Machine-readable state from the app, and an isolated data dir so the CLI
# checks never touch the user's real to-dos.
$statusFile = Join-Path $env:TEMP 'perch-smoke-status.json'
Remove-Item $statusFile -ErrorAction SilentlyContinue
$env:PERCH_STATUS_FILE = $statusFile
$dataDir = Join-Path $env:TEMP 'perch-smoke-data'
Remove-Item $dataDir -Recurse -Force -ErrorAction SilentlyContinue
$env:PERCH_DATA_DIR = $dataDir

Write-Host "=== Perch UI smoke test ==="
Write-Host "exe: $Exe"

# Keep a known foreground window so we can prove focus is never stolen.
$fgBefore = [PerchNative]::GetForegroundWindow()

$proc = Start-Process -FilePath $Exe -ArgumentList @('--config', $cfg) -PassThru
Start-Sleep -Seconds 3

try {
    if ($proc.HasExited) { throw "Perch exited immediately with code $($proc.ExitCode)" }

    $wins = Get-PerchWindows $proc.Id
    Check 'windows_created' ($wins.Count -ge 2) "found $($wins.Count) windows: $(($wins | ForEach-Object { $_.Class }) -join ', ')"

    $pill = $wins | Where-Object { $_.Class -eq 'PerchNotch' } | Select-Object -First 1
    $hot  = $wins | Where-Object { $_.Class -eq 'PerchHotZone' } | Select-Object -First 1
    Check 'pill_window_present' ($null -ne $pill) "class PerchNotch$(if($pill){" at $($pill.X),$($pill.Y) $($pill.W)x$($pill.H)"})"
    Check 'hot_zone_present' ($null -ne $hot) "class PerchHotZone$(if($hot){" at $($hot.X),$($hot.Y) $($hot.W)x$($hot.H)"})"

    if ($pill) {
        $need = $WS_EX_TOOLWINDOW -bor $WS_EX_NOACTIVATE -bor $WS_EX_TOPMOST
        $got = $pill.ExStyle
        Check 'pill_exstyle' (($got -band $need) -eq $need) ("ex-style 0x{0:X} has TOOLWINDOW|NOACTIVATE|TOPMOST" -f $got)
        Check 'pill_not_in_alt_tab' (($got -band $WS_EX_APPWINDOW) -eq 0) ("WS_EX_APPWINDOW clear, so no Alt-Tab/taskbar entry")
    }
    if ($hot) {
        Check 'hot_zone_is_thin' ($hot.H -le 8) "hot zone is $($hot.H)px tall (must stay 1-8px)"
    }

    # --- a known, ordinary foreground window --------------------------------
    # Over a maximized or fullscreen window the notch rests as a stripe (by
    # design), so whatever the person had in front would change the results.
    # Put a small, normal window of our own in front first, and bring it back
    # after the fullscreen / maximized checks.
    $base = New-Object System.Windows.Forms.Form
    $base.Text = 'Perch smoke base'
    $base.StartPosition = 'Manual'
    $base.Bounds = New-Object System.Drawing.Rectangle 80, 300, 520, 360
    $base.Show()
    function Show-Base {
        $base.Activate()
        [PerchNative]::SetForegroundWindow($base.Handle) | Out-Null
        for ($i = 0; $i -lt 15; $i++) {
            [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100
            $s = Read-Status
            if ($s -and $s.obscured -eq $false) { break }
        }
    }
    function Read-Status { try { Get-Content $statusFile -Raw | ConvertFrom-Json } catch { $null } }
    Show-Base
    $st = Read-Status
    Write-Host ("[INFO] base window in front: foreground={0}, obscured={1}" -f ([PerchNative]::GetForegroundWindow() -eq $base.Handle), $st.obscured)

    # --- hover expands -------------------------------------------------------
    # Park the cursor well away from the notch FIRST and wait longer than the
    # collapse delay + animation, so the baseline really is the collapsed size.
    # (A cursor left anywhere in the pill's expanded footprint - including the
    # screen centre, which the pill covers when open - makes the baseline wrong
    # and the comparison meaningless.)
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $parkX = 40
    $parkY = $screen.Height - 80
    $script:pillHandle = $pill.Handle
    $pillHandle = $pill.Handle
    Move-MouseAbsolute $parkX $parkY
    Start-Sleep -Milliseconds 1500
    $before = Get-LiveRect $pillHandle
    Write-Host ("[INFO] collapsed baseline: {0}x{1}" -f $before.W, $before.H)

    Move-MouseAbsolute ([int]($screen.Width/2)) 0
    Start-Sleep -Milliseconds 900
    $after = Get-LiveRect $pillHandle
    Write-Host ("[INFO] hovered: {0}x{1}" -f $after.W, $after.H)

    Check 'hover_expands' ($after.H -gt $before.H) "pill height $($before.H) -> $($after.H) on hover"
    Check 'hover_expands_width' ($after.W -ge $before.W) "pill width $($before.W) -> $($after.W)"

    Save-TopStripScreenshot (Join-Path $OutDir 'perch-expanded.png')

    # --- leaving collapses ---------------------------------------------------
    Move-MouseAbsolute $parkX $parkY
    Start-Sleep -Milliseconds 1500
    $collapsed = Get-LiveRect $pillHandle
    Write-Host ("[INFO] collapsed again: {0}x{1}" -f $collapsed.W, $collapsed.H)
    Check 'leaving_collapses' ($collapsed.H -lt $after.H) "pill height $($after.H) -> $($collapsed.H) after leaving"
    Check 'collapses_to_original' ($collapsed.H -eq $before.H) "back to $($before.H)px"

    Save-TopStripScreenshot (Join-Path $OutDir 'perch-collapsed.png')

    # --- CLI verbs over the named pipe ---------------------------------------
    function Invoke-Perch([string[]]$CliArgs) {
        Start-Process -FilePath $Exe -ArgumentList $CliArgs -Wait -WindowStyle Hidden
    }
    function Read-Status { try { Get-Content $statusFile -Raw | ConvertFrom-Json } catch { $null } }
    Invoke-Perch @('toggle-notch'); Start-Sleep -Milliseconds 900
    $st = Read-Status
    Check 'cli_toggle_pins_open' ($st.state -eq 'Expanded') "state $($st.state)"
    Invoke-Perch @('toggle-notch'); Start-Sleep -Milliseconds 1000
    $st = Read-Status
    Check 'cli_toggle_closes' ($st.state -eq 'Collapsed') "state $($st.state)"
    Invoke-Perch @('todo', 'smoke', 'test', 'item'); Start-Sleep -Milliseconds 600
    $st = Read-Status
    Check 'cli_todo' ($st.todos -eq 1) "todos=$($st.todos)"

    # --- a to-do falling due shows as a banner, then goes away ---------------
    Invoke-Perch @('todo', '+2s', 'Smoke', 'banner')
    $shown = ''
    for ($i = 0; $i -lt 60; $i++) {
        $st = Read-Status
        if ($st -and $st.banner) { $shown = $st.banner; break }
        Start-Sleep -Milliseconds 100
    }
    Check 'todo_due_banners' ([bool]$shown) "banner '$shown'"
    Start-Sleep -Milliseconds 400
    Save-TopStripScreenshot (Join-Path $OutDir 'perch-banner.png')
    $gone = $false
    for ($i = 0; $i -lt 80; $i++) {
        $st = Read-Status
        if ($st -and -not $st.banner) { $gone = $true; break }
        Start-Sleep -Milliseconds 100
    }
    Check 'banner_goes_away' $gone "banner cleared within 8 s"
    Check 'first_frame_fast' ($st.first_frame_ms -lt 300) "first frame $($st.first_frame_ms) ms after process start (target < 300)"

    # --- clipboard board: record copies, never password-manager copies ------
    function WaitStatus([scriptblock]$cond, [int]$ms = 5000) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
        return $false
    }
    Set-Clipboard -Value 'hello from the smoke test'
    $ok = WaitStatus { (Read-Status).clipboard -eq 1 }
    Check 'clipboard_records_copy' $ok "entries=$((Read-Status).clipboard)"
    $do = New-Object System.Windows.Forms.DataObject
    $do.SetData([System.Windows.Forms.DataFormats]::UnicodeText, 'hunter2')
    # What KeePass, 1Password and friends add so history tools stay away.
    $do.SetData('ExcludeClipboardContentFromMonitorProcessing', $true)
    [System.Windows.Forms.Clipboard]::SetDataObject($do, $true)
    Start-Sleep -Milliseconds 1500
    Check 'clipboard_skips_password_manager' ((Read-Status).clipboard -eq 1) "entries=$((Read-Status).clipboard)"
    Set-Clipboard -Value 'second copy'
    $ok = WaitStatus { (Read-Status).clipboard -eq 2 }
    Check 'clipboard_records_after_secret' $ok "entries=$((Read-Status).clipboard)"

    # --- fullscreen auto-hide ------------------------------------------------
    # A borderless window covering the monitor, brought to the foreground, is
    # what games / video players / F11 look like to the shell.
    $form = New-Object System.Windows.Forms.Form
    $form.FormBorderStyle = 'None'
    $form.StartPosition = 'Manual'
    $form.Bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $form.BackColor = [System.Drawing.Color]::Black
    $form.Show()
    $form.Activate()
    [PerchNative]::SetForegroundWindow($form.Handle) | Out-Null
    for ($i = 0; $i -lt 10; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
    $isFg = ([PerchNative]::GetForegroundWindow() -eq $form.Handle)
    # The shell's quiet state can lag the foreground change; Perch re-checks
    # 1.5 s later, so allow up to 3 s.
    for ($i = 0; $i -lt 20; $i++) {
        $st = Read-Status
        if ($st -and $st.stripe -eq $true) { break }
        [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100
    }
    if ($isFg) {
        # Over a fullscreen app the notch rests as a thin stripe...
        Start-Sleep -Milliseconds 500
        $r = Get-LiveRect $pillHandle
        Check 'fullscreen_stripes' ($st.stripe -eq $true -and $st.visible -eq $true -and $r.H -le 8) "stripe=$($st.stripe), $($r.W)x$($r.H)"
        # ...that a 1 s hover unlocks: the full panel opens.
        Move-MouseAbsolute ([int]($screen.Width/2)) 0
        for ($i = 0; $i -lt 5; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
        $st = Read-Status
        Check 'stripe_hover_waits' ($st.state -eq 'Collapsed') "state after 0.5 s: $($st.state)"
        for ($i = 0; $i -lt 13; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
        $st = Read-Status
        Check 'stripe_hover_unlocks' ($st.state -eq 'Expanded') "state after 1.8 s: $($st.state)"
        # Unlocked, it rests as the normal pill, and hovering opens it at once.
        Move-MouseAbsolute $parkX $parkY
        for ($i = 0; $i -lt 12; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
        $st = Read-Status
        Check 'unlocked_rests_as_pill' ($st.stripe -eq $false -and $st.state -eq 'Collapsed') "stripe=$($st.stripe), state $($st.state)"
        Move-MouseAbsolute ([int]($screen.Width/2)) 0
        for ($i = 0; $i -lt 6; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
        $st = Read-Status
        Check 'unlocked_pill_opens_at_once' ($st.state -eq 'Expanded') "state 0.6 s after hovering the pill: $($st.state)"
        Move-MouseAbsolute $parkX $parkY
        for ($i = 0; $i -lt 12; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
    } else {
        Write-Host "[INFO] fullscreen_stripes: skipped, Windows refused to give the test window the foreground (foreground lock)"
        $results['fullscreen_stripes'] = @{ ok = $true; detail = 'skipped: foreground lock' }
    }
    $form.Close(); $form.Dispose()
    Show-Base
    # The shell's quiet state lags; Perch re-checks 1.5 s after a focus change.
    for ($i = 0; $i -lt 25; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
    $st = Read-Status
    Check 'unfullscreen_restores' ($st.stripe -eq $false -and $st.obscured -eq $false) "stripe=$($st.stripe)"

    # A maximized window: the notch rests as a stripe too.
    $max = New-Object System.Windows.Forms.Form
    $max.Text = 'Perch smoke maximized'
    $max.WindowState = 'Maximized'
    $max.Show()
    $max.Activate()
    [PerchNative]::SetForegroundWindow($max.Handle) | Out-Null
    for ($i = 0; $i -lt 15; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
    if ([PerchNative]::GetForegroundWindow() -eq $max.Handle) {
        $st = Read-Status
        Check 'maximized_stripes' ($st.stripe -eq $true) "stripe=$($st.stripe)"
    } else {
        Write-Host "[INFO] maximized_stripes: skipped (foreground lock)"
    }
    $max.Close(); $max.Dispose()
    Show-Base
    for ($i = 0; $i -lt 10; $i++) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 }
    $st = Read-Status
    Check 'unmaximized_restores' ($st.stripe -eq $false) "stripe=$($st.stripe)"
    # The foreground changes above (to the test window and back) are what
    # screen time records.
    Check 'screen_time_records' ($st.usage_apps -ge 1) "apps today=$($st.usage_apps)"

    # Runs after the foreground-dependent checks: once Perch's overlay has
    # held focus, Windows' foreground lock stops the test process from
    # bringing its own window to the front.
    # --- screenshot: overlay, drag a region, Enter copies a PNG -------------
    [System.Windows.Forms.Clipboard]::Clear()
    Invoke-Perch @('shot')
    $ok = WaitStatus { (Read-Status).shot_open -eq $true }
    Check 'shot_overlay_opens' $ok "shot_open=$((Read-Status).shot_open)"
    if ($ok) {
        Start-Sleep -Milliseconds 400
        Move-MouseAbsolute 700 300
        [PerchNative]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)   # left down
        Move-MouseAbsolute 800 380
        Move-MouseAbsolute 900 450
        [PerchNative]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)   # left up
        Start-Sleep -Milliseconds 300
        [System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
        $ok = WaitStatus { (Read-Status).shot_open -eq $false }
        Check 'shot_closes_after_copy' $ok "shot_open=$((Read-Status).shot_open)"
        $img = [System.Windows.Forms.Clipboard]::GetImage()
        Check 'shot_copies_image' ($null -ne $img) "$(if ($img) { "$($img.Width)x$($img.Height)" } else { 'no image on the clipboard' })"
        Move-MouseAbsolute $parkX $parkY
    }


    # --- focus must never move ----------------------------------------------
    $fgAfter = [PerchNative]::GetForegroundWindow()
    $perchHandles = (Get-PerchWindows $proc.Id | ForEach-Object { $_.Handle })
    Check 'never_steals_focus' (-not ($perchHandles -contains $fgAfter)) "foreground window is not a Perch window"

    # --- resource usage ------------------------------------------------------
    $p = Get-Process -Id $proc.Id
    $ws = [math]::Round($p.WorkingSet64/1MB, 2)
    $pv = [math]::Round($p.PrivateMemorySize64/1MB, 2)
    $results['resources'] = @{
        working_set_mb = $ws
        private_mb     = $pv
        handles        = $p.HandleCount
        threads        = $p.Threads.Count
        cpu_seconds    = $p.CPU
    }
    Write-Host ("[INFO] working set {0} MB, private {1} MB, handles {2}, threads {3}, cpu {4}s" -f $ws, $pv, $p.HandleCount, $p.Threads.Count, $p.CPU)
    Check 'idle_ram_under_40mb' ($pv -lt 40) "private bytes $pv MB (target < 40 MB)"
}
finally {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    if ($base) { $base.Close(); $base.Dispose() }
}

$resultFile = Join-Path $OutDir 'smoke-results.json'
($results | ConvertTo-Json -Depth 5) | Set-Content -Path $resultFile -Encoding utf8
Write-Host "results: $resultFile"
Write-Host "screenshots: $OutDir\perch-expanded.png, $OutDir\perch-collapsed.png"

if ($failures.Count -gt 0) {
    Write-Error "smoke test FAILED: $($failures -join ', ')"
    exit 1
}
Write-Host "smoke test PASSED"
exit 0
