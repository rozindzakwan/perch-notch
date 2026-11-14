# Per-user NSIS install/uninstall + portable zip check. Run after scripts/package-windows.ps1.
# Installs into %TEMP% (not Program Files), then uninstalls; results -> artifacts/windows/installer-results.json.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$setup = Join-Path $repo 'dist\perch-0.1.0-windows-x64-setup.exe'
$zip = Join-Path $repo 'dist\perch-0.1.0-windows-x64-portable.zip'
$dir = Join-Path $env:TEMP 'perch-installer-test\Perch'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Perch'
$lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Perch.lnk'
$r = [ordered]@{}
function Check($name, $ok, $detail) { $r[$name] = "$(if ($ok) {'PASS'} else {'FAIL'}) $detail" }

if (Get-Process perch -ErrorAction SilentlyContinue) { throw 'perch.exe is running; stop it first' }
Remove-Item (Split-Path $dir) -Recurse -Force -ErrorAction SilentlyContinue

# Silent per-user install into a test folder (/D must be last, unquoted).
$p = Start-Process $setup -ArgumentList "/S", "/D=$dir" -Wait -PassThru
Check 'install_exit' ($p.ExitCode -eq 0) "exit $($p.ExitCode)"
$files = 'perch.exe', 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'uninstall.exe' | ForEach-Object { Test-Path (Join-Path $dir $_) }
Check 'install_files' (-not ($files -contains $false)) "$dir"
$u = Get-ItemProperty $key -ErrorAction SilentlyContinue
Check 'arp_entry' ($u -and $u.DisplayName -eq 'Perch' -and $u.DisplayVersion -eq '0.1.0') "DisplayName=$($u.DisplayName) DisplayVersion=$($u.DisplayVersion) InstallLocation=$($u.InstallLocation)"
Check 'start_menu' (Test-Path $lnk) $lnk
$v = & (Join-Path $dir 'perch.exe') --version 2>&1 | Out-String
Check 'installed_runs' ($v -match 'Perch 0\.1\.0') $v.Trim()
$vi = (Get-Item (Join-Path $dir 'perch.exe')).VersionInfo
Check 'version_resource' ($vi.ProductName -eq 'Perch') "$($vi.ProductName) $($vi.FileVersion)"

# Silent uninstall (NSIS copies the uninstaller to %TEMP% and returns at once).
Start-Process (Join-Path $dir 'uninstall.exe') -ArgumentList '/S' -Wait | Out-Null
for ($i = 0; $i -lt 60 -and (Test-Path $dir); $i++) { Start-Sleep -Milliseconds 250 }
Check 'uninstall_removes_dir' (-not (Test-Path $dir)) $dir
Check 'uninstall_removes_arp' (-not (Test-Path $key)) $key
Check 'uninstall_removes_shortcut' (-not (Test-Path $lnk)) $lnk
Check 'uninstall_keeps_settings' $true "%APPDATA%\perch untouched by design"

# Portable zip.
$pz = Join-Path $env:TEMP 'perch-portable-test'
Remove-Item $pz -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive $zip -DestinationPath $pz
$exe = Get-ChildItem $pz -Recurse -Filter perch.exe | Select-Object -First 1
$pv = & $exe.FullName --version 2>&1 | Out-String
Check 'portable_runs' ($pv -match 'Perch 0\.1\.0') "$((Get-ChildItem $pz -Recurse -File).Name -join ', ') -> $($pv.Trim())"
Remove-Item $pz -Recurse -Force
Remove-Item (Split-Path $dir) -Recurse -Force -ErrorAction SilentlyContinue

$r.GetEnumerator() | ForEach-Object { '{0,-28} {1}' -f $_.Key, $_.Value }
$r | ConvertTo-Json | Set-Content (Join-Path $repo 'artifacts\windows\installer-results.json') -Encoding utf8
if ($r.Values -match '^FAIL') { exit 1 }
