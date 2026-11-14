#Requires -Version 5.1
<#
.SYNOPSIS
  Long-running resource soak test for Perch on Windows.

.DESCRIPTION
  Starts Perch with the everyday features on (media, to-do, Pomodoro,
  clipboard, shelf, reminders, screen time), then for -Minutes samples every
  -IntervalSeconds: working set, private bytes, CPU time, handles, threads,
  GDI and USER objects. Runs fine on a machine that is in use: the first
  -WarmupSeconds are excluded and growth compares 5-minute medians. Every 5 minutes it opens and closes the notch through
  the CLI (exercising animation, drawing and IPC) without touching the mouse.

  Writes soak-windows.csv and soak-windows.md (summary) to -OutDir and fails
  when private bytes, handles or GDI/USER objects grow beyond the leak
  thresholds, the median 10-second CPU exceeds 0.5 % of a core, or the average
  exceeds 2 %.
#>
[CmdletBinding()]
param(
    [int]$Minutes = 30,
    [int]$IntervalSeconds = 10,
    [string]$Exe,
    [string]$OutDir,
    [int]$WarmupSeconds = 120,
    # Re-score an existing CSV instead of running (e.g. after changing thresholds).
    [string]$SummarizeFrom
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $repo 'target\release\perch.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'artifacts\windows' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$cores = [Environment]::ProcessorCount

if ($SummarizeFrom) {
    $rows = @(Import-Csv $SummarizeFrom | ForEach-Object {
        [pscustomobject]@{ t = [int]$_.t_s; ws = [double]$_.working_set_mb; priv = [double]$_.private_mb
            cpu = [double]$_.cpu_s; handles = [int]$_.handles; threads = [int]$_.threads
            gdi = [int]$_.gdi; user = [int]$_.user }
    })
} else {

if (-not ([System.Management.Automation.PSTypeName]'PerchSoak').Type) {
Add-Type -Namespace '' -Name PerchSoak -MemberDefinition @'
    [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr hProcess, uint uiFlags);
'@
}

$cfg = Join-Path $env:TEMP 'perch-soak-config.toml'
@"
[features]
media = true
todo = true
pomodoro = true
clipboard = true
shelf = true
reminders = true
screen_time = true
"@ | Set-Content -Path $cfg -Encoding ascii
$env:PERCH_DATA_DIR = Join-Path $env:TEMP 'perch-soak-data'
Remove-Item Env:PERCH_STATUS_FILE -ErrorAction SilentlyContinue
Get-Process perch -ErrorAction SilentlyContinue | Stop-Process -Force
$proc = Start-Process -FilePath $Exe -ArgumentList @('--config', $cfg) -PassThru
Start-Sleep -Seconds 5

$csv = Join-Path $OutDir 'soak-windows.csv'
"t_s,working_set_mb,private_mb,cpu_s,handles,threads,gdi,user" | Set-Content $csv -Encoding ascii
$rows = New-Object System.Collections.ArrayList
$sw = [Diagnostics.Stopwatch]::StartNew()
$nextToggle = 300
try {
    while ($sw.Elapsed.TotalMinutes -lt $Minutes) {
        $p = Get-Process -Id $proc.Id -ErrorAction Stop
        $p.Refresh()
        $row = [pscustomobject]@{
            t = [int]$sw.Elapsed.TotalSeconds
            ws = [math]::Round($p.WorkingSet64 / 1MB, 2)
            priv = [math]::Round($p.PrivateMemorySize64 / 1MB, 2)
            cpu = [math]::Round($p.TotalProcessorTime.TotalSeconds, 3)
            handles = $p.HandleCount
            threads = $p.Threads.Count
            gdi = [PerchSoak]::GetGuiResources($p.Handle, 0)
            user = [PerchSoak]::GetGuiResources($p.Handle, 1)
        }
        [void]$rows.Add($row)
        "{0},{1},{2},{3},{4},{5},{6},{7}" -f $row.t, $row.ws, $row.priv, $row.cpu, $row.handles, $row.threads, $row.gdi, $row.user | Add-Content $csv
        if ($row.t -ge $nextToggle) {
            # Open, hold, close: animation + redraws + IPC, no mouse.
            Start-Process -FilePath $Exe -ArgumentList 'toggle-notch' -Wait -WindowStyle Hidden
            Start-Sleep -Seconds 2
            Start-Process -FilePath $Exe -ArgumentList 'toggle-notch' -Wait -WindowStyle Hidden
            $nextToggle += 300
        }
        Start-Sleep -Seconds $IntervalSeconds
    }
}
finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}
}

# --- summary -------------------------------------------------------------
# Leak check on medians of 5-minute windows (after a warm-up, during which
# features fill their caches: clipboard history, screen-time rows, fonts),
# so a single busy sample cannot pass or fail the run on its own.
function Median($xs) {
    $s = @($xs | Sort-Object)
    if ($s.Count -eq 0) { return 0 }
    return $s[[int][math]::Floor(($s.Count - 1) / 2)]
}
$last = $rows[$rows.Count - 1]
$post = @($rows | Where-Object { $_.t -ge $WarmupSeconds })
if ($post.Count -lt 2) { $post = @($rows | Select-Object -Skip 1) }
$first = $post[0]
$t0 = $first.t
$startWin = @($post | Where-Object { $_.t -lt $t0 + 300 })
$endWin = @($post | Where-Object { $_.t -ge $last.t - 300 })
$elapsed = $last.t - $first.t
$cpuPct = if ($elapsed -gt 0) { 100.0 * ($last.cpu - $first.cpu) / $elapsed / $cores } else { 0 }
$cpuPctOneCore = if ($elapsed -gt 0) { 100.0 * ($last.cpu - $first.cpu) / $elapsed } else { 0 }
# Per-interval CPU: the median is the "at rest" figure (a busy loop raises
# every interval); the average also includes reacting to real use of the PC.
$intervals = for ($i = 1; $i -lt $post.Count; $i++) {
    $dt = $post[$i].t - $post[$i - 1].t
    if ($dt -gt 0) { 100.0 * ($post[$i].cpu - $post[$i - 1].cpu) / $dt }
}
$cpuMedian = Median $intervals
$m = $post | Measure-Object -Property priv -Maximum -Minimum
$wsMax = ($rows | Measure-Object -Property ws -Maximum).Maximum
$privStart = Median ($startWin | ForEach-Object { $_.priv })
$privEnd = Median ($endWin | ForEach-Object { $_.priv })
$hStart = Median ($startWin | ForEach-Object { $_.handles })
$hEnd = Median ($endWin | ForEach-Object { $_.handles })
$gStart = Median ($startWin | ForEach-Object { $_.gdi }); $gEnd = Median ($endWin | ForEach-Object { $_.gdi })
$uStart = Median ($startWin | ForEach-Object { $_.user }); $uEnd = Median ($endWin | ForEach-Object { $_.user })
$privGrowth = [math]::Round($privEnd - $privStart, 2)
$handleGrowth = $hEnd - $hStart
$gdiGrowth = $gEnd - $gStart
$userGrowth = $uEnd - $uStart
$fail = @()
if ($privGrowth -gt 5) { $fail += "private bytes grew $privGrowth MB" }
if ($handleGrowth -gt 50) { $fail += "handles grew by $handleGrowth" }
if ($gdiGrowth -gt 20 -or $userGrowth -gt 20) { $fail += "GDI/USER objects grew ($gdiGrowth / $userGrowth)" }
if ($cpuMedian -gt 0.5) { $fail += ("median CPU {0:N3}% of one core" -f $cpuMedian) }
if ($cpuPctOneCore -gt 2) { $fail += ("average CPU {0:N3}% of one core" -f $cpuPctOneCore) }
$durationMin = [math]::Round($last.t / 60.0, 1)

$md = @"
# Perch soak test (Windows)

- Duration: $durationMin min, $($rows.Count) samples, notch toggled via CLI every 5 min; warm-up $WarmupSeconds s excluded
- Start/End = median of the first/last 5 minutes after warm-up
- Machine: $((Get-CimInstance Win32_Processor).Name.Trim()), $cores logical CPUs, Windows $([Environment]::OSVersion.Version)

| Metric | Start | End | Min | Max | Growth |
|---|---|---|---|---|---|
| Private bytes (MB) | $privStart | $privEnd | $([math]::Round($m.Minimum,2)) | $([math]::Round($m.Maximum,2)) | $privGrowth |
| Working set (MB) | $($first.ws) | $($last.ws) | | $wsMax | |
| Handles | $hStart | $hEnd | | | $handleGrowth |
| GDI objects | $gStart | $gEnd | | | $gdiGrowth |
| USER objects | $uStart | $uEnd | | | $userGrowth |
| Threads | $($first.threads) | $($last.threads) | | | |

- CPU at rest (median of 10 s intervals): $("{0:N3}" -f $cpuMedian)% of one core
- CPU average incl. reacting to use of the PC: $([math]::Round($last.cpu - $first.cpu, 2)) s over $elapsed s = $("{0:N3}" -f $cpuPctOneCore)% of one core ($("{0:N4}" -f $cpuPct)% of the machine)
- Result: $(if ($fail.Count) { 'FAIL: ' + ($fail -join '; ') } else { 'PASS (no growth beyond thresholds: private +5 MB, handles +50, GDI/USER +20, median CPU 0.5% and average CPU 2% of a core)' })
"@
$md | Set-Content (Join-Path $OutDir 'soak-windows.md') -Encoding utf8
Write-Host $md
if ($fail.Count) { exit 1 }
