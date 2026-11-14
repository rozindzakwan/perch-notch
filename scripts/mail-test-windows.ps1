#Requires -Version 5.1
<#
.SYNOPSIS
  Windows end-to-end test of the read-only Mail feature.

.DESCRIPTION
  Needs the throwaway Dovecot from scripts/mail-test.sh running in the Linux CI
  container and published on localhost:1993:

    docker run -d --name perch-dovecot -p 127.0.0.1:1993:1993 -v "$PWD:/src" `
      perch-linux-ci scripts/mail-test.sh --serve --out /src/artifacts/linux

  Stores the test password in Windows Credential Manager with
  `perch mail-password`, starts perch.exe, checks sign-in over TLS (with the
  test CA), the unread list, a pushed new message, that nothing was marked
  read on the server, and removes the test credential afterwards.
#>
[CmdletBinding()]
param(
    [string]$Exe,
    [string]$Container = 'perch-dovecot',
    [string]$OutDir
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $repo 'target\release\perch.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'artifacts\windows' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$ca = Join-Path $repo 'artifacts\linux\mail-ca.pem'
if (-not (Test-Path $ca)) { throw "no $ca; start the Dovecot container first (see the help)" }

$results = [ordered]@{}
function Check($name, $ok, $detail) {
    $results[$name] = "$(if ($ok) {'PASS'} else {'FAIL'}) $detail"
    Write-Host ("[{0}] {1}: {2}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $detail)
}

$tmp = Join-Path $env:TEMP 'perch-mail-test'
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $tmp | Out-Null
$cfg = Join-Path $tmp 'config.toml'
@"
[features]
mail = true
[mail]
notify = false
[[mail.accounts]]
name = "TLS"
username = "test@example.com"
host = "localhost"
port = 1993
security = "tls"
ca_file = '$ca'
"@ | Set-Content $cfg -Encoding ascii
$env:PERCH_STATUS_FILE = Join-Path $tmp 'status.json'
$env:PERCH_DATA_DIR = Join-Path $tmp 'data'
function Status { Get-Content $env:PERCH_STATUS_FILE -Raw -Encoding UTF8 -ErrorAction SilentlyContinue | ConvertFrom-Json }
function WaitFor([scriptblock]$cond, [int]$ms = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}
$target = 'Perch/mail:test@example.com@localhost'

Get-Process perch -ErrorAction SilentlyContinue | Stop-Process -Force
try {
    'correct horse' | & $Exe --config $cfg mail-password TLS | Out-Null
    $listed = (cmdkey /list | Out-String) -match ("target=" + [regex]::Escape($target))
    Check 'password_in_credential_manager' $listed $target
    Check 'password_not_in_config' (-not (Select-String -Path $cfg -Pattern 'correct horse' -Quiet)) 'config.toml has no password'

    $p = Start-Process -FilePath $Exe -ArgumentList @('--config', $cfg) -PassThru
    $ok = WaitFor { (Status).mail_unread -eq 2 }
    $s = Status
    Check 'signs_in_over_tls' (($s.mail_accounts -join ',') -eq 'TLS: online') ($s.mail_accounts -join ', ')
    Check 'unread_listed' $ok "mail_unread=$($s.mail_unread)"
    Check 'subjects_decoded' ($s.mail_latest -eq "J$([char]0xFC)rgen - Halo $([char]::ConvertFromUtf32(0x1F600))") "latest: $($s.mail_latest)"

    $sw = [Diagnostics.Stopwatch]::StartNew()
    $f = "/home/mailtest/Maildir/new/$([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()).win.perchtest"
    docker exec $Container bash -c "printf 'From: Build Bot <ci@example.com>\r\nSubject: Pushed to Windows\r\nMessage-ID: <win@perch.test>\r\n\r\nhi\r\n' > $f && chown mailtest:mailtest $f" | Out-Null
    $pushed = WaitFor { (Status).mail_unread -eq 3 } 10000
    Check 'new_mail_pushed' $pushed ("visible after {0} ms: {1}" -f $sw.ElapsedMilliseconds, (Status).mail_latest)

    $seen = (docker exec $Container bash -c 'doveadm -c /tmp/perch-mail.*/dovecot.conf search -u test@example.com mailbox INBOX SEEN 2>/dev/null | wc -l').Trim()
    $all = (docker exec $Container bash -c 'doveadm -c /tmp/perch-mail.*/dovecot.conf search -u test@example.com mailbox INBOX ALL 2>/dev/null | wc -l').Trim()
    Check 'nothing_marked_read' (($seen -eq '0') -and ($all -eq '3')) "$seen of $all messages are \Seen on the server"

    Start-Sleep -Seconds 2
    $p.Refresh()
    Check 'idle_private_memory' ($p.PrivateMemorySize64 / 1MB -lt 40) ("{0:N1} MB private" -f ($p.PrivateMemorySize64 / 1MB))
    & $Exe quit | Out-Null
    $gone = WaitFor { $p.HasExited } 5000
    Check 'quits_cleanly' $gone 'perch quit'
}
finally {
    Get-Process perch -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    cmdkey /delete:$target | Out-Null
    Check 'test_credential_removed' (-not ((cmdkey /list | Out-String) -match ("target=" + [regex]::Escape($target)))) "deleted $target"
}
$results | ConvertTo-Json | Set-Content (Join-Path $OutDir 'mail-results.json') -Encoding utf8
if ($results.Values -match '^FAIL') { exit 1 }
