# Shared by the Windows smoke scripts that move the mouse, open windows or
# use the clipboard: wait until nobody has touched the keyboard or mouse for
# a while, so a person working at the machine is never interrupted.
#
# Dot-source it, then:  if (-not (Wait-UserIdle -Seconds 60 -MaxMinutes 20)) { exit 3 }

if (-not ('PerchIdle' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class PerchIdle {
    [StructLayout(LayoutKind.Sequential)]
    struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    [DllImport("user32.dll")] static extern bool GetLastInputInfo(ref LASTINPUTINFO info);
    public static double Seconds() {
        var info = new LASTINPUTINFO();
        info.cbSize = (uint)Marshal.SizeOf(info);
        if (!GetLastInputInfo(ref info)) return 0;
        return ((uint)Environment.TickCount - info.dwTime) / 1000.0;
    }
}
'@
}

# True once input has been idle for $Seconds; false if that did not happen
# within $MaxMinutes (the caller should skip, not fail).
function Wait-UserIdle([int]$Seconds = 60, [int]$MaxMinutes = 20) {
    $deadline = (Get-Date).AddMinutes($MaxMinutes)
    $said = $false
    while ((Get-Date) -lt $deadline) {
        $idle = [PerchIdle]::Seconds()
        if ($idle -ge $Seconds) { return $true }
        if (-not $said) {
            Write-Host ("waiting for {0} s without keyboard or mouse input (idle now: {1:N0} s)" -f $Seconds, $idle)
            $said = $true
        }
        Start-Sleep -Seconds ([math]::Max(1, [math]::Ceiling($Seconds - $idle)))
    }
    Write-Host "the machine stayed in use for $MaxMinutes min; skipping the input-driven checks"
    return $false
}
