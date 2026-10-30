; Per-user installer for 墨笺 (md-work).
;
; Build with:
;   cargo run -p xtask -- package
;   cargo run -p xtask -- installer
; or manually:
;   makensis /DVERSION=1.0.0 /DDIST=<workspace>\dist\md-work-1.0.0-win64 md-work.nsi
;
; Only HKCU is written and nothing is installed machine-wide, so no elevation
; is required. The app's own "用墨笺打开" verb (HKCU\Software\Classes) is left
; untouched here and can be (re)registered from Settings -> Windows.

!include "MUI2.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "1.0.0"
!endif
!ifndef DIST
  !error "Pass /DDIST=<path to dist\md-work-<ver>-win64>"
!endif

!define APP_NAME "墨笺"
!define APP_SLUG "md-work"
!define APP_PUBLISHER "lidawei"
!define APP_EXE "md-work.exe"
!define APP_CLI "md-work-cli.exe"
!define APP_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\md-work"

Name "${APP_NAME} ${VERSION}"
OutFile "..\..\dist\md-work-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\${APP_SLUG}"
InstallDirRegKey HKCU "Software\${APP_SLUG}" "InstallDir"
RequestExecutionLevel user
Unicode true
SetCompressor /SOLID lzma
BrandingText "${APP_NAME} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=2052 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=2052 "FileDescription" "${APP_NAME} 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "Copyright © 2026 ${APP_PUBLISHER}. MIT License."

!define MUI_ABORTWARNING
!define MUI_ICON "..\..\assets\icon.ico"
!define MUI_UNICON "..\..\assets\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "启动${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetShellVarContext current
FunctionEnd

Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "主程序" SEC_MAIN
  SetOutPath "$INSTDIR"
  File "${DIST}\${APP_EXE}"
  File "${DIST}\${APP_CLI}"
  SetOutPath "$INSTDIR\licenses"
  File "${DIST}\LICENSE"
  File "${DIST}\THIRD-PARTY-NOTICES.md"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "Software\${APP_SLUG}" "InstallDir" "$INSTDIR"

  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"

  WriteRegStr HKCU "${APP_KEY}" "DisplayName" "${APP_NAME} (${APP_SLUG})"
  WriteRegStr HKCU "${APP_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${APP_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${APP_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${APP_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${APP_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKCU "${APP_KEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegDWORD HKCU "${APP_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${APP_KEY}" "NoRepair" 1

  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${APP_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${APP_CLI}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\licenses"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${APP_KEY}"
  DeleteRegKey HKCU "Software\${APP_SLUG}"
  ; Notes, settings and the per-user file association stay in place on purpose:
  ; reinstalling picks them up again.
SectionEnd
