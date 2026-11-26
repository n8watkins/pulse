; Pulse installer (NSIS 3, Modern UI 2). Per-user, no administrator rights, unsigned.
;
; Built by scripts/build-installer.ps1, which passes the defines below with /D.
; Everything that names the product comes from this block, so a test build can use another name:
;   PRODUCT_NAME   shown name, install folder, shortcut, uninstall key, Run-key value ("Pulse")
;   SETTINGS_DIR   folder under %APPDATA% that holds settings (defaults to the product name)
;   VERSION        x.y.z
;   VERSION_4      x.y.z.0, for the installer's own file version
;   EXE_SRC        the built pulse-native.exe
;   OUT_FILE       path of the installer to write
;   REPO_ROOT      the repository root (notices and font licences are read from there)

!ifndef PRODUCT_NAME
  !define PRODUCT_NAME "Pulse"
!endif
!ifndef SETTINGS_DIR
  !define SETTINGS_DIR "${PRODUCT_NAME}"
!endif
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef VERSION_4
  !define VERSION_4 "0.0.0.0"
!endif
!ifndef REPO_ROOT
  !define REPO_ROOT ".."
!endif
!ifndef OUT_FILE
  !define OUT_FILE "${PRODUCT_NAME}-${VERSION}-setup.exe"
!endif
!ifndef EXE_SRC
  !error "pass /DEXE_SRC=<path to pulse-native.exe>"
!endif

!define PUBLISHER "n8builds.dev"
!define EXE_NAME "${PRODUCT_NAME}.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define ICON_FILE "${REPO_ROOT}\apps\native\assets\pulse.ico"

Unicode true
Name "${PRODUCT_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\${PRODUCT_NAME}"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
RequestExecutionLevel user
ManifestDPIAware true
ManifestSupportedOS all
SetCompressor /SOLID lzma
BrandingText "${PRODUCT_NAME} ${VERSION}"
ShowInstDetails nevershow
ShowUninstDetails nevershow

VIProductVersion "${VERSION_4}"
VIAddVersionKey "ProductName" "${PRODUCT_NAME} Setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "FileDescription" "${PRODUCT_NAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 ${PUBLISHER}"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Start ${PRODUCT_NAME}"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApp

!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Writes $PLUGINSDIR\close.ps1, which stops the copy of the app that runs from -Dir:
;  1. if the installed exe is version 0.1.0 or later, run it with --quit (hidden) and wait up to 10 s;
;     if that process is still running after 10 s it is ended by its PID (an exe that does not
;     understand --quit would have started normally);
;  2. then, only if a process from that exact folder is still running, end it as a last resort.
; Processes are matched by full executable path, never by image name alone, so a copy running from
; another folder (a dev build, another install) is never touched.
!macro WriteCloseScript
  InitPluginsDir
  FileOpen $0 "$PLUGINSDIR\close.ps1" w
  FileWrite $0 `param([string]$$Dir)$\r$\n`
  FileWrite $0 `$$exe = Join-Path $$Dir '${EXE_NAME}'$\r$\n`
  FileWrite $0 `if (Test-Path -LiteralPath $$exe) {$\r$\n`
  FileWrite $0 `  $$vi = (Get-Item -LiteralPath $$exe).VersionInfo; $$v = $$null$\r$\n`
  FileWrite $0 `  try { $$v = [version]('{0}.{1}.{2}' -f $$vi.FileMajorPart, $$vi.FileMinorPart, $$vi.FileBuildPart) } catch {}$\r$\n`
  FileWrite $0 `  if ($$v -and $$v -ge [version]'0.1.0') {$\r$\n`
  FileWrite $0 `    $$q = Start-Process -FilePath $$exe -ArgumentList '--quit' -WindowStyle Hidden -PassThru$\r$\n`
  FileWrite $0 `    if (-not $$q.WaitForExit(10000)) { Stop-Process -Id $$q.Id -Force -ErrorAction SilentlyContinue }$\r$\n`
  FileWrite $0 `  }$\r$\n`
  FileWrite $0 `}$\r$\n`
  FileWrite $0 `$$mine = @(Get-CimInstance Win32_Process | Where-Object { $$_.ExecutablePath -and ($$_.ExecutablePath -ieq $$exe) })$\r$\n`
  FileWrite $0 `foreach ($$m in $$mine) { Stop-Process -Id $$m.ProcessId -Force -ErrorAction SilentlyContinue; Wait-Process -Id $$m.ProcessId -Timeout 5 -ErrorAction SilentlyContinue }$\r$\n`
  FileClose $0
  nsExec::Exec `powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\close.ps1" -Dir "$INSTDIR"`
  Pop $0
!macroend

Function LaunchApp
  Exec '"$INSTDIR\${EXE_NAME}"'
FunctionEnd

Function .onInstSuccess
  ; Silent installs start the app too, unless /NORUN is given.
  ${If} ${Silent}
    ${GetParameters} $0
    ${GetOptions} $0 "/NORUN" $1
    ${If} ${Errors}
      Call LaunchApp
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Install" SEC_MAIN
  SetShellVarContext current
  !insertmacro WriteCloseScript

  SetOutPath "$INSTDIR"
  File "/oname=${EXE_NAME}" "${EXE_SRC}"
  File "${REPO_ROOT}\THIRD-PARTY-NOTICES.md"
  File "${REPO_ROOT}\apps\native\fonts\OFL-Inter.txt"
  File "${REPO_ROOT}\apps\native\fonts\OFL-SpaceGrotesk.txt"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}.lnk" "$INSTDIR\${EXE_NAME}" "" "$INSTDIR\${EXE_NAME}" 0

  SectionGetSize ${SEC_MAIN} $0
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${EXE_NAME},0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  !insertmacro WriteCloseScript

  Delete "$INSTDIR\${EXE_NAME}"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.md"
  Delete "$INSTDIR\OFL-Inter.txt"
  Delete "$INSTDIR\OFL-SpaceGrotesk.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"

  ; "Start with Windows" belongs to the app; remove its value only if it points into this folder.
  ReadRegStr $0 HKCU "${RUN_KEY}" "${PRODUCT_NAME}"
  ${If} $0 != ""
    System::Call 'shlwapi::StrStrIW(w r0, w "$INSTDIR") p .r1'
    ${If} $1 != 0
      DeleteRegValue HKCU "${RUN_KEY}" "${PRODUCT_NAME}"
    ${EndIf}
  ${EndIf}

  DeleteRegKey HKCU "${UNINSTALL_KEY}"

  ; Settings stay unless asked for; a silent uninstall keeps them.
  IfFileExists "$APPDATA\${SETTINGS_DIR}\*.*" 0 done
  MessageBox MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION "Also remove your ${PRODUCT_NAME} settings?" /SD IDNO IDNO done
  RMDir /r "$APPDATA\${SETTINGS_DIR}"
  done:
SectionEnd
