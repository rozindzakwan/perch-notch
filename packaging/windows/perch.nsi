; Perch per-user installer (no administrator rights needed).
; Built by scripts/package-windows.ps1:  makensis /DVERSION=x.y.z /DSRC=<dir> perch.nsi

!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef SRC
  !define SRC "..\..\target\release"
!endif
!ifndef OUTFILE
  !define OUTFILE "perch-${VERSION}-windows-x64-setup.exe"
!endif

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Perch"

Name "Perch"
OutFile "${OUTFILE}"
Unicode true
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Perch"
InstallDirRegKey HKCU "Software\Perch" "InstallDir"
SetCompressor /SOLID lzma
BrandingText "Perch ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Perch"
VIAddVersionKey "FileDescription" "Perch installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT OR Apache-2.0"
VIAddVersionKey "CompanyName" "Perch contributors"

!define MUI_ICON "..\..\assets\perch.ico"
!define MUI_UNICON "..\..\assets\perch.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\perch.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Perch now"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE-MIT"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Perch" SecMain
  SetShellVarContext current
  ; An upgrade must not fail because the old version is running.
  nsExec::Exec 'taskkill /IM perch.exe /F'
  Sleep 300
  SetOutPath "$INSTDIR"
  File "${SRC}\perch.exe"
  File "..\..\README.md"
  File "..\..\LICENSE-MIT"
  File "..\..\LICENSE-APACHE"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Perch.lnk" "$INSTDIR\perch.exe" "" "$INSTDIR\perch.exe" 0

  WriteRegStr HKCU "Software\Perch" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "Perch"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Perch contributors"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\perch.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" 9000
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  nsExec::Exec 'taskkill /IM perch.exe /F'
  Sleep 300
  Delete "$INSTDIR\perch.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE-MIT"
  Delete "$INSTDIR\LICENSE-APACHE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Perch.lnk"
  ; Undo what Perch registered at runtime: start-at-login and the toast id.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Perch"
  DeleteRegKey HKCU "Software\Classes\AppUserModelId\Perch.Notch"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\Perch"
  ; Settings and data (%APPDATA%\perch) are kept, like most apps do; remove
  ; that folder by hand for a completely clean slate.
SectionEnd
